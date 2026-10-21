//! The computer's audio output (the other people on a call), captured on its
//! own track. macOS 14.2+ through Core Audio process taps, implemented in
//! `swift/system_audio_tap.swift`; other platforms report it unsupported and
//! meetings record the microphone only.

use std::sync::mpsc::SyncSender;

/// Why system audio is not being captured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemAudioError {
    Unsupported,
    Failed(i32),
}

/// What the OS reports about the "System Audio Recording" permission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum SystemAudioPermission {
    Authorized,
    Denied,
    /// Not asked yet, or the OS doesn't say. Starting a meeting asks.
    Unknown,
}

pub fn is_supported() -> bool {
    imp::is_supported()
}

pub fn permission() -> SystemAudioPermission {
    imp::permission()
}

/// A running capture. 16 kHz mono Float32 chunks arrive on the channel given
/// to [`SystemAudioCapture::start`] until it is dropped.
pub struct SystemAudioCapture {
    inner: imp::Capture,
}

impl SystemAudioCapture {
    pub fn start(tx: SyncSender<Vec<f32>>) -> Result<Self, SystemAudioError> {
        imp::Capture::start(tx).map(|inner| Self { inner })
    }
}

impl Drop for SystemAudioCapture {
    fn drop(&mut self) {
        self.inner.stop();
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::{SystemAudioError, SystemAudioPermission};
    use std::os::raw::{c_int, c_void};
    use std::sync::mpsc::SyncSender;

    type Callback = extern "C" fn(*mut c_void, *const f32, c_int);

    extern "C" {
        fn handy_system_audio_is_supported() -> c_int;
        fn handy_system_audio_permission_status() -> c_int;
        fn handy_system_audio_start(
            context: *mut c_void,
            callback: Callback,
            error_out: *mut c_int,
        ) -> *mut c_void;
        fn handy_system_audio_stop(handle: *mut c_void);
    }

    pub fn is_supported() -> bool {
        unsafe { handy_system_audio_is_supported() == 1 }
    }

    pub fn permission() -> SystemAudioPermission {
        match unsafe { handy_system_audio_permission_status() } {
            1 => SystemAudioPermission::Authorized,
            2 => SystemAudioPermission::Denied,
            _ => SystemAudioPermission::Unknown,
        }
    }

    struct Context {
        tx: SyncSender<Vec<f32>>,
    }

    /// Runs on the tap's IO queue: copy and hand off, never block. A full
    /// channel means the consumer stalled; dropping audio beats stalling the
    /// HAL.
    extern "C" fn on_audio(context: *mut c_void, samples: *const f32, count: c_int) {
        if context.is_null() || samples.is_null() || count <= 0 {
            return;
        }
        let context = unsafe { &*(context as *const Context) };
        let chunk = unsafe { std::slice::from_raw_parts(samples, count as usize) }.to_vec();
        let _ = context.tx.try_send(chunk);
    }

    pub struct Capture {
        handle: *mut c_void,
        context: *mut Context,
    }

    // The handle is only used to stop the capture, which the Swift side
    // serializes; the context is only read by the callback.
    unsafe impl Send for Capture {}

    impl Capture {
        pub fn start(tx: SyncSender<Vec<f32>>) -> Result<Self, SystemAudioError> {
            if !is_supported() {
                return Err(SystemAudioError::Unsupported);
            }
            let context = Box::into_raw(Box::new(Context { tx }));
            let mut error: c_int = 0;
            let handle =
                unsafe { handy_system_audio_start(context as *mut c_void, on_audio, &mut error) };
            if handle.is_null() {
                // No callback can run: the tap never started.
                drop(unsafe { Box::from_raw(context) });
                return Err(if error == 1 {
                    SystemAudioError::Unsupported
                } else {
                    SystemAudioError::Failed(error)
                });
            }
            Ok(Self { handle, context })
        }

        pub fn stop(&mut self) {
            if !self.handle.is_null() {
                // Returns once no callback can run any more.
                unsafe { handy_system_audio_stop(self.handle) };
                self.handle = std::ptr::null_mut();
            }
            if !self.context.is_null() {
                drop(unsafe { Box::from_raw(self.context) });
                self.context = std::ptr::null_mut();
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::{SystemAudioError, SystemAudioPermission};
    use std::sync::mpsc::SyncSender;

    pub fn is_supported() -> bool {
        false
    }

    pub fn permission() -> SystemAudioPermission {
        SystemAudioPermission::Unknown
    }

    pub struct Capture;

    impl Capture {
        pub fn start(_tx: SyncSender<Vec<f32>>) -> Result<Self, SystemAudioError> {
            Err(SystemAudioError::Unsupported)
        }

        pub fn stop(&mut self) {}
    }
}

#[cfg(test)]
mod tests {
    /// Reads support and permission through the Swift bridge, which never
    /// prompts. Run with `--ignored` on a Mac.
    #[test]
    #[ignore]
    fn bridge_answers() {
        eprintln!(
            "system audio supported: {}, permission: {:?}",
            super::is_supported(),
            super::permission()
        );
    }
}
