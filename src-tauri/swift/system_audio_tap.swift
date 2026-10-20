// System audio capture for meeting mode, via Core Audio process taps
// (macOS 14.2+). Everything the Mac plays (the other participants of a call)
// is tapped, mixed to mono and converted to 16 kHz Float32, then handed to
// Rust through a C callback. Handy's own process is excluded so its feedback
// sounds never end up in a transcript.
//
// The tap is attached to a private aggregate device whose clock comes from
// the current default output device. When the default output changes (AirPods
// connected mid-call), the aggregate is rebuilt on the new device so capture
// keeps going.
//
// Compiled by build.rs into a static library on macOS. Deployment target is
// 11.0, so every tap API is guarded with `#available`.

import AVFoundation
import CoreAudio
import Darwin
import Foundation

public typealias HandySystemAudioCallback = @convention(c) (
    UnsafeMutableRawPointer?, UnsafePointer<Float>?, Int32
) -> Void

private let targetSampleRate: Double = 16_000

// Error codes returned through `errorOut`. Mirrored in system_audio.rs.
private let errUnsupportedOS: Int32 = 1
private let errTapCreation: Int32 = 2
private let errAggregateCreation: Int32 = 3
private let errIOProc: Int32 = 4
private let errDeviceStart: Int32 = 5
private let errFormat: Int32 = 6

@available(macOS 14.2, *)
private final class SystemAudioTap {
    private let context: UnsafeMutableRawPointer?
    private let callback: HandySystemAudioCallback
    /// IO blocks run here. Never block it on `stateLock` or call
    /// AudioDeviceStop from it: the HAL waits on this queue while stopping.
    private let ioQueue = DispatchQueue(label: "computer.handy.meeting.system-audio.io")
    /// Default-output-change notifications run here.
    private let controlQueue = DispatchQueue(label: "computer.handy.meeting.system-audio.control")
    /// Serializes start / stop / rebuild.
    private let stateLock = NSLock()
    /// Guards `converter` and `inputFormat`, read by every IO block.
    private let formatLock = NSLock()

    private var tapID = AudioObjectID(kAudioObjectUnknown)
    private var aggregateID = AudioObjectID(kAudioObjectUnknown)
    private var ioProcID: AudioDeviceIOProcID?
    private var converter: AVAudioConverter?
    private var inputFormat: AVAudioFormat?
    private let outputFormat = AVAudioFormat(
        commonFormat: .pcmFormatFloat32,
        sampleRate: targetSampleRate,
        channels: 1,
        interleaved: false
    )!
    private var listenerBlock: AudioObjectPropertyListenerBlock?
    private var stopped = false

    init(context: UnsafeMutableRawPointer?, callback: @escaping HandySystemAudioCallback) {
        self.context = context
        self.callback = callback
    }

    /// Builds the tap and aggregate and starts delivering audio. Returns 0 or
    /// one of the error codes above.
    func start() -> Int32 {
        stateLock.lock()
        defer { stateLock.unlock() }
        let code = buildAndStart()
        if code != 0 {
            teardown()
            return code
        }
        installDefaultOutputListener()
        return 0
    }

    func stop() {
        removeDefaultOutputListener()
        stateLock.lock()
        stopped = true
        teardown()
        stateLock.unlock()
        // Let any IO block already queued finish before the context is freed
        // by the caller.
        ioQueue.sync {}
    }

    // MARK: - Setup

    private func buildAndStart() -> Int32 {
        let description = CATapDescription(stereoGlobalTapButExcludeProcesses: ownProcessObjects())
        description.uuid = UUID()
        description.name = "Handy meeting tap"
        description.isPrivate = true
        description.muteBehavior = .unmuted

        var newTap = AudioObjectID(kAudioObjectUnknown)
        guard AudioHardwareCreateProcessTap(description, &newTap) == noErr,
            newTap != kAudioObjectUnknown
        else {
            return errTapCreation
        }
        tapID = newTap

        guard var streamDescription = readTapFormat(newTap),
            let format = AVAudioFormat(streamDescription: &streamDescription),
            let newConverter = AVAudioConverter(from: format, to: outputFormat)
        else {
            return errFormat
        }
        formatLock.lock()
        inputFormat = format
        converter = newConverter
        formatLock.unlock()

        guard let outputUID = defaultOutputDeviceUID() else {
            return errAggregateCreation
        }
        let aggregateDescription: [String: Any] = [
            kAudioAggregateDeviceNameKey: "Handy meeting capture",
            kAudioAggregateDeviceUIDKey: UUID().uuidString,
            kAudioAggregateDeviceMainSubDeviceKey: outputUID,
            kAudioAggregateDeviceIsPrivateKey: true,
            kAudioAggregateDeviceIsStackedKey: false,
            kAudioAggregateDeviceTapAutoStartKey: true,
            kAudioAggregateDeviceSubDeviceListKey: [[kAudioSubDeviceUIDKey: outputUID]],
            kAudioAggregateDeviceTapListKey: [
                [
                    kAudioSubTapDriftCompensationKey: true,
                    kAudioSubTapUIDKey: description.uuid.uuidString,
                ]
            ],
        ]
        var newAggregate = AudioObjectID(kAudioObjectUnknown)
        guard
            AudioHardwareCreateAggregateDevice(aggregateDescription as CFDictionary, &newAggregate)
                == noErr
        else {
            return errAggregateCreation
        }
        aggregateID = newAggregate

        var procID: AudioDeviceIOProcID?
        let status = AudioDeviceCreateIOProcIDWithBlock(&procID, newAggregate, ioQueue) {
            [weak self] _, inputData, _, _, _ in
            self?.deliver(inputData)
        }
        guard status == noErr, let procID else {
            return errIOProc
        }
        ioProcID = procID

        guard AudioDeviceStart(newAggregate, procID) == noErr else {
            return errDeviceStart
        }
        return 0
    }

    private func teardown() {
        if aggregateID != kAudioObjectUnknown {
            if let procID = ioProcID {
                AudioDeviceStop(aggregateID, procID)
                AudioDeviceDestroyIOProcID(aggregateID, procID)
            }
            AudioHardwareDestroyAggregateDevice(aggregateID)
        }
        ioProcID = nil
        aggregateID = AudioObjectID(kAudioObjectUnknown)
        if tapID != kAudioObjectUnknown {
            AudioHardwareDestroyProcessTap(tapID)
        }
        tapID = AudioObjectID(kAudioObjectUnknown)
        formatLock.lock()
        converter = nil
        inputFormat = nil
        formatLock.unlock()
    }

    // MARK: - Audio delivery (runs on `ioQueue`)

    private func deliver(_ inputData: UnsafePointer<AudioBufferList>) {
        formatLock.lock()
        defer { formatLock.unlock() }
        guard let format = inputFormat, let converter else { return }
        guard
            let input = AVAudioPCMBuffer(
                pcmFormat: format, bufferListNoCopy: inputData, deallocator: nil)
        else { return }
        let frames = input.frameLength
        if frames == 0 { return }

        let ratio = targetSampleRate / format.sampleRate
        let capacity = AVAudioFrameCount(Double(frames) * ratio) + 64
        guard let output = AVAudioPCMBuffer(pcmFormat: outputFormat, frameCapacity: capacity)
        else { return }

        var consumed = false
        var error: NSError?
        let status = converter.convert(to: output, error: &error) { _, inputStatus in
            if consumed {
                inputStatus.pointee = .noDataNow
                return nil
            }
            consumed = true
            inputStatus.pointee = .haveData
            return input
        }
        guard status != .error, output.frameLength > 0,
            let channel = output.floatChannelData?[0]
        else { return }
        callback(context, UnsafePointer(channel), Int32(output.frameLength))
    }

    // MARK: - Default output changes

    private func installDefaultOutputListener() {
        var address = AudioObjectPropertyAddress(
            mSelector: kAudioHardwarePropertyDefaultOutputDevice,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain
        )
        let block: AudioObjectPropertyListenerBlock = { [weak self] _, _ in
            self?.rebuildAfterOutputChange()
        }
        if AudioObjectAddPropertyListenerBlock(
            AudioObjectID(kAudioObjectSystemObject), &address, controlQueue, block) == noErr
        {
            listenerBlock = block
        }
    }

    private func removeDefaultOutputListener() {
        guard let block = listenerBlock else { return }
        var address = AudioObjectPropertyAddress(
            mSelector: kAudioHardwarePropertyDefaultOutputDevice,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain
        )
        AudioObjectRemovePropertyListenerBlock(
            AudioObjectID(kAudioObjectSystemObject), &address, controlQueue, block)
        listenerBlock = nil
    }

    private func rebuildAfterOutputChange() {
        stateLock.lock()
        defer { stateLock.unlock() }
        if stopped { return }
        teardown()
        let code = buildAndStart()
        if code != 0 {
            // Leave it torn down; Rust notices the silence and the meeting
            // keeps the microphone track.
            teardown()
        }
    }

    // MARK: - Helpers

    private func readTapFormat(_ tap: AudioObjectID) -> AudioStreamBasicDescription? {
        var address = AudioObjectPropertyAddress(
            mSelector: kAudioTapPropertyFormat,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain
        )
        var description = AudioStreamBasicDescription()
        var size = UInt32(MemoryLayout<AudioStreamBasicDescription>.size)
        guard AudioObjectGetPropertyData(tap, &address, 0, nil, &size, &description) == noErr,
            description.mSampleRate > 0
        else {
            return nil
        }
        return description
    }

    private func defaultOutputDeviceUID() -> String? {
        var address = AudioObjectPropertyAddress(
            mSelector: kAudioHardwarePropertyDefaultSystemOutputDevice,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain
        )
        var deviceID = AudioObjectID(kAudioObjectUnknown)
        var size = UInt32(MemoryLayout<AudioObjectID>.size)
        guard
            AudioObjectGetPropertyData(
                AudioObjectID(kAudioObjectSystemObject), &address, 0, nil, &size, &deviceID)
                == noErr,
            deviceID != kAudioObjectUnknown
        else {
            return nil
        }

        address.mSelector = kAudioDevicePropertyDeviceUID
        var uid: Unmanaged<CFString>?
        size = UInt32(MemoryLayout<Unmanaged<CFString>?>.size)
        guard AudioObjectGetPropertyData(deviceID, &address, 0, nil, &size, &uid) == noErr,
            let uid
        else {
            return nil
        }
        return uid.takeRetainedValue() as String
    }

    /// Handy's own Core Audio process object, so the tap skips our sounds.
    private func ownProcessObjects() -> [AudioObjectID] {
        var address = AudioObjectPropertyAddress(
            mSelector: kAudioHardwarePropertyTranslatePIDToProcessObject,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain
        )
        var pid = getpid()
        var processObject = AudioObjectID(kAudioObjectUnknown)
        var size = UInt32(MemoryLayout<AudioObjectID>.size)
        let status = AudioObjectGetPropertyData(
            AudioObjectID(kAudioObjectSystemObject),
            &address,
            UInt32(MemoryLayout<pid_t>.size),
            &pid,
            &size,
            &processObject
        )
        if status == noErr, processObject != kAudioObjectUnknown {
            return [processObject]
        }
        return []
    }
}

// MARK: - C entry points

@_cdecl("handy_system_audio_is_supported")
public func handySystemAudioIsSupported() -> Int32 {
    if #available(macOS 14.2, *) {
        return 1
    }
    return 0
}

/// 0 = not determined / unknown, 1 = authorized, 2 = denied. Best effort: the
/// only way to read this TCC service is the private TCC preflight call, so a
/// missing symbol reads as unknown and the caller simply tries.
@_cdecl("handy_system_audio_permission_status")
public func handySystemAudioPermissionStatus() -> Int32 {
    typealias Preflight = @convention(c) (CFString, CFDictionary?) -> Int
    guard
        let handle = dlopen(
            "/System/Library/PrivateFrameworks/TCC.framework/Versions/A/TCC", RTLD_NOW)
    else {
        return 0
    }
    defer { dlclose(handle) }
    guard let symbol = dlsym(handle, "TCCAccessPreflight") else {
        return 0
    }
    let preflight = unsafeBitCast(symbol, to: Preflight.self)
    switch preflight("kTCCServiceAudioCapture" as CFString, nil) {
    case 0: return 1
    case 1: return 2
    default: return 0
    }
}

/// Starts capturing. Returns an opaque handle for `handy_system_audio_stop`,
/// or nil with an error code in `errorOut`.
@_cdecl("handy_system_audio_start")
public func handySystemAudioStart(
    _ context: UnsafeMutableRawPointer?,
    _ callback: HandySystemAudioCallback,
    _ errorOut: UnsafeMutablePointer<Int32>?
) -> UnsafeMutableRawPointer? {
    guard #available(macOS 14.2, *) else {
        errorOut?.pointee = errUnsupportedOS
        return nil
    }
    let tap = SystemAudioTap(context: context, callback: callback)
    let code = tap.start()
    if code != 0 {
        errorOut?.pointee = code
        return nil
    }
    errorOut?.pointee = 0
    return Unmanaged.passRetained(tap as AnyObject).toOpaque()
}

/// Stops capturing. No callback runs after this returns.
@_cdecl("handy_system_audio_stop")
public func handySystemAudioStop(_ handle: UnsafeMutableRawPointer?) {
    guard let handle else { return }
    let object = Unmanaged<AnyObject>.fromOpaque(handle).takeRetainedValue()
    if #available(macOS 14.2, *), let tap = object as? SystemAudioTap {
        tap.stop()
    }
}
