//! The running side of a meeting: two audio sources, a segmenter per track,
//! and one transcriber thread that turns utterances into stored segments.
//!
//! ```text
//! microphone ─▶ mic supervisor ─▶ track "Me"     ─┐
//!                                                  ├─▶ transcriber ─▶ store + events
//! system tap ──────────────────▶ track "Others" ─┘
//! ```
//!
//! Every thread ends when its input closes, so stopping is just closing the
//! sources and joining in order: tracks flush their last utterance, then the
//! transcriber drains the queue.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use handy_recorder::{AudioChunk, Channels, ErrorKind, Recorder, RecorderConfig, Sink};
use log::{debug, info, warn};
use tauri::{AppHandle, Manager};
use tauri_specta::Event;

use super::segmenter::{energy_probability, Segmenter, SegmenterConfig, Utterance};
use super::store::{MeetingStore, SystemAudioState};
use super::system_audio::{self, SystemAudioCapture, SystemAudioError, SystemAudioPermission};
use super::transcript::{is_echo, is_noise, MeetingSegment, Speaker};
use super::{MeetingClock, MeetingError, MeetingSegmentEvent, MeetingSegmentRemovedEvent};
use crate::managers::transcription::TranscriptionManager;
use crate::settings::AppSettings;

const RATE: u64 = 16_000;
/// Up to ~30 s of audio may queue per track before chunks are dropped.
const TRACK_QUEUE_CHUNKS: usize = 1_000;
/// Audio missing for longer than this (a device switch, a stalled tap) is a
/// gap: the timeline jumps instead of squeezing later speech earlier.
const GAP_TOLERANCE_MS: u64 = 1_500;

pub(super) struct CaptureStart<'a> {
    pub app: AppHandle,
    pub store: Arc<MeetingStore>,
    pub meeting_id: i64,
    pub settings: &'a AppSettings,
    pub clock: Arc<Mutex<MeetingClock>>,
    pub paused: Arc<AtomicBool>,
    pub pending: Arc<AtomicU32>,
}

/// Something the transcriber should turn into text.
struct TrackUtterance {
    speaker: Speaker,
    /// Meeting time, ms.
    start_ms: u64,
    end_ms: u64,
    samples: Vec<f32>,
}

pub(super) struct CaptureSession {
    mic_control: Option<Sender<MicCommand>>,
    mic_thread: Option<JoinHandle<()>>,
    system: Option<SystemAudioCapture>,
    tracks: Vec<JoinHandle<()>>,
    transcriber: Option<JoinHandle<()>>,
    pending: Arc<AtomicU32>,
}

impl CaptureSession {
    /// Opens the microphone (required) and the system tap (best effort) and
    /// starts every thread. Returns the session and whether system audio is
    /// being captured.
    pub fn start(args: CaptureStart<'_>) -> Result<(Self, SystemAudioState), MeetingError> {
        let (utterance_tx, utterance_rx) = mpsc::channel::<TrackUtterance>();

        // Microphone first: without it there is no meeting.
        let (mic_tx, mic_rx) = mpsc::sync_channel::<Vec<f32>>(TRACK_QUEUE_CHUNKS);
        let mic_config = microphone_config(args.settings);
        let (mic_control_tx, mic_control_rx) = mpsc::channel::<MicCommand>();
        let recorder = open_mic(mic_config.clone(), mic_tx.clone(), mic_control_tx.clone())
            .map_err(|e| match e.kind() {
                ErrorKind::PermissionDenied => MeetingError::MicrophonePermissionDenied,
                ErrorKind::DeviceUnavailable => MeetingError::NoInputDevice,
                _ => MeetingError::Other(e.to_string()),
            })?;
        info!(
            "Meeting microphone: {} ({} Hz)",
            recorder.info().device.name,
            recorder.info().device_format.sample_rate
        );
        let mic_thread = {
            let control_tx = mic_control_tx.clone();
            std::thread::Builder::new()
                .name("meeting-mic".into())
                .spawn(move || {
                    supervise_mic(recorder, mic_config, mic_tx, control_tx, mic_control_rx)
                })
                .map_err(|e| MeetingError::Other(e.to_string()))?
        };

        // System audio: the other participants. A refusal or an old macOS
        // leaves a microphone-only meeting, which the UI explains.
        let (system_tx, system_rx) = mpsc::sync_channel::<Vec<f32>>(TRACK_QUEUE_CHUNKS);
        let (system, system_state) = if !system_audio::is_supported() {
            (None, SystemAudioState::Unsupported)
        } else if system_audio::permission() == SystemAudioPermission::Denied {
            (None, SystemAudioState::Denied)
        } else {
            match SystemAudioCapture::start(system_tx) {
                Ok(capture) => (Some(capture), SystemAudioState::Captured),
                Err(SystemAudioError::Unsupported) => (None, SystemAudioState::Unsupported),
                Err(SystemAudioError::Failed(code)) => {
                    warn!("System audio capture failed to start (code {code})");
                    (None, SystemAudioState::Failed)
                }
            }
        };

        let mut tracks = Vec::new();
        for (speaker, rx) in [(Speaker::Me, mic_rx), (Speaker::Others, system_rx)] {
            let track = TrackWorker {
                speaker,
                rx,
                tx: utterance_tx.clone(),
                clock: args.clock.clone(),
                paused: args.paused.clone(),
                pending: args.pending.clone(),
                app: args.app.clone(),
                backend: args.settings.vad_backend,
            };
            let handle = std::thread::Builder::new()
                .name(format!("meeting-track-{}", speaker.as_str()))
                .spawn(move || track.run())
                .map_err(|e| MeetingError::Other(e.to_string()))?;
            tracks.push(handle);
        }
        drop(utterance_tx);

        let transcriber = {
            let worker = Transcriber {
                app: args.app.clone(),
                store: args.store.clone(),
                meeting_id: args.meeting_id,
                rx: utterance_rx,
                pending: args.pending.clone(),
            };
            std::thread::Builder::new()
                .name("meeting-transcriber".into())
                .spawn(move || worker.run())
                .map_err(|e| MeetingError::Other(e.to_string()))?
        };

        Ok((
            Self {
                mic_control: Some(mic_control_tx),
                mic_thread: Some(mic_thread),
                system,
                tracks,
                transcriber: Some(transcriber),
                pending: args.pending,
            },
            system_state,
        ))
    }

    /// The permission prompt appears when the tap first starts, and a refusal
    /// only shows as silence. Re-reads the permission a little later and
    /// reports a refusal through `on_change`.
    pub fn watch_system_permission(
        &self,
        state: Arc<Mutex<SystemAudioState>>,
        on_change: impl Fn(SystemAudioState) + Send + 'static,
    ) {
        if self.system.is_none() {
            return;
        }
        std::thread::spawn(move || {
            for _ in 0..12 {
                std::thread::sleep(Duration::from_secs(5));
                if system_audio::permission() == SystemAudioPermission::Denied {
                    let mut current = state.lock().unwrap_or_else(|p| p.into_inner());
                    if *current == SystemAudioState::Captured {
                        *current = SystemAudioState::Denied;
                        drop(current);
                        on_change(SystemAudioState::Denied);
                    }
                    return;
                }
            }
        });
    }

    /// Stops capture and blocks until every utterance is transcribed.
    /// `on_progress` runs as the queue drains, for the UI.
    pub fn finish(mut self, on_progress: impl Fn()) {
        if let Some(control) = self.mic_control.take() {
            let _ = control.send(MicCommand::Stop);
        }
        if let Some(handle) = self.mic_thread.take() {
            let _ = handle.join();
        }
        // Dropping the capture closes the system track's channel.
        self.system.take();
        for handle in self.tracks.drain(..) {
            let _ = handle.join();
        }
        on_progress();
        if let Some(handle) = self.transcriber.take() {
            let pending = self.pending.clone();
            let mut last = u32::MAX;
            while !handle.is_finished() {
                let now = pending.load(Ordering::Relaxed);
                if now != last {
                    last = now;
                    on_progress();
                }
                std::thread::sleep(Duration::from_millis(250));
            }
            let _ = handle.join();
        }
        debug!("Meeting capture finished");
    }
}

/* ───────────────────────────── microphone ───────────────────────────── */

enum MicCommand {
    Stop,
    Failed,
}

struct MicSink {
    tx: SyncSender<Vec<f32>>,
}

impl Sink for MicSink {
    fn process_chunk(&mut self, chunk: AudioChunk<'_>) {
        let real = chunk.valid_frames * chunk.channels as usize;
        let _ = self.tx.try_send(chunk.samples[..real].to_vec());
    }
}

fn microphone_config(settings: &AppSettings) -> RecorderConfig {
    let device = settings.selected_microphone.as_ref().and_then(|name| {
        handy_recorder::list_input_devices()
            .ok()?
            .into_iter()
            .find(|d| &d.name == name)
            .map(|d| d.id)
    });
    RecorderConfig {
        device,
        channels: settings
            .selected_channel
            .map_or(Channels::MixToMono, Channels::Only),
        ..RecorderConfig::speech()
    }
}

fn open_mic(
    config: RecorderConfig,
    tx: SyncSender<Vec<f32>>,
    control: Sender<MicCommand>,
) -> Result<Recorder<MicSink>, handy_recorder::Error> {
    let recorder = Recorder::open_with_failure_handler(config, move |error| {
        warn!("Meeting microphone failed: {error}");
        let _ = control.send(MicCommand::Failed);
    })?;
    recorder.start(MicSink { tx }).map_err(|e| e.error)?;
    Ok(recorder)
}

/// Keeps the microphone recording for the whole meeting: a device that
/// disappears (headset unplugged) is replaced by the selected one or the
/// system default.
fn supervise_mic(
    recorder: Recorder<MicSink>,
    config: RecorderConfig,
    tx: SyncSender<Vec<f32>>,
    control_tx: Sender<MicCommand>,
    control: Receiver<MicCommand>,
) {
    let mut recorder = Some(recorder);
    loop {
        match control.recv() {
            Ok(MicCommand::Stop) | Err(_) => break,
            Ok(MicCommand::Failed) => {
                if let Some(old) = recorder.take() {
                    let _ = old.stop();
                }
                // Retry until it works or the meeting stops.
                loop {
                    match control.recv_timeout(Duration::from_secs(1)) {
                        Ok(MicCommand::Stop) | Err(RecvTimeoutError::Disconnected) => {
                            return;
                        }
                        Ok(MicCommand::Failed) | Err(RecvTimeoutError::Timeout) => {}
                    }
                    let attempt =
                        open_mic(config.clone(), tx.clone(), control_tx.clone()).or_else(|_| {
                            let fallback = RecorderConfig {
                                device: None,
                                ..config.clone()
                            };
                            open_mic(fallback, tx.clone(), control_tx.clone())
                        });
                    match attempt {
                        Ok(new) => {
                            info!("Meeting microphone reopened: {}", new.info().device.name);
                            recorder = Some(new);
                            break;
                        }
                        Err(e) => debug!("Meeting microphone still unavailable: {e}"),
                    }
                }
            }
        }
    }
    if let Some(recorder) = recorder {
        // The returned sink holds a sender; dropping it here (with `tx`)
        // closes the track once this thread exits.
        let _ = recorder.stop();
    }
}

/* ───────────────────────────── tracks ───────────────────────────── */

struct TrackWorker {
    speaker: Speaker,
    rx: Receiver<Vec<f32>>,
    tx: Sender<TrackUtterance>,
    clock: Arc<Mutex<MeetingClock>>,
    paused: Arc<AtomicBool>,
    pending: Arc<AtomicU32>,
    app: AppHandle,
    backend: crate::settings::VadBackend,
}

impl TrackWorker {
    fn run(self) {
        let mut scorer = match crate::managers::audio::create_detector(self.backend, &self.app) {
            Ok(scorer) => Some(scorer),
            Err(e) => {
                warn!("Meeting VAD unavailable, falling back to loudness: {e}");
                None
            }
        };
        let frame = scorer.as_ref().map_or(512, |s| s.frame_samples());
        let mut segmenter = Segmenter::new(SegmenterConfig::for_frame(frame));
        let mut pending_audio: Vec<f32> = Vec::new();
        let mut was_paused = false;

        loop {
            let chunk = match self.rx.recv_timeout(Duration::from_millis(500)) {
                Ok(chunk) => chunk,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => break,
            };

            let paused = self.paused.load(Ordering::Acquire);
            if paused {
                if !was_paused {
                    pending_audio.clear();
                    if let Some(u) = segmenter.flush() {
                        self.send(u);
                    }
                }
                was_paused = true;
                continue;
            }
            was_paused = false;

            // Keep the track on the meeting clock across gaps in the audio.
            let now_ms = self
                .clock
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .elapsed()
                .as_millis() as u64;
            let position_ms =
                (segmenter.position_frames() * frame as u64 + pending_audio.len() as u64) * 1000
                    / RATE;
            let chunk_ms = chunk.len() as u64 * 1000 / RATE;
            if now_ms > position_ms + chunk_ms + GAP_TOLERANCE_MS {
                let gap_ms = now_ms - chunk_ms - position_ms;
                let gap_frames = gap_ms * RATE / 1000 / frame as u64;
                pending_audio.clear();
                if let Some(u) = segmenter.skip(gap_frames) {
                    self.send(u);
                }
            }

            pending_audio.extend_from_slice(&chunk);
            let whole = pending_audio.len() / frame * frame;
            if whole == 0 {
                continue;
            }
            let frames: Vec<f32> = pending_audio.drain(..whole).collect();
            let probs = match scorer.as_mut().map(|s| s.score(&frames)) {
                Some(Ok(probs)) => probs,
                Some(Err(e)) => {
                    warn!("Meeting VAD failed, falling back to loudness: {e}");
                    scorer = None;
                    frames.chunks(frame).map(energy_probability).collect()
                }
                None => frames.chunks(frame).map(energy_probability).collect(),
            };
            for utterance in segmenter.push(&frames, &probs) {
                self.send(utterance);
            }
        }

        if let Some(u) = segmenter.flush() {
            self.send(u);
        }
    }

    fn send(&self, utterance: Utterance) {
        self.pending.fetch_add(1, Ordering::Relaxed);
        let item = TrackUtterance {
            speaker: self.speaker,
            start_ms: utterance.start_ms(),
            end_ms: utterance.end_ms(),
            samples: utterance.samples,
        };
        if self.tx.send(item).is_err() {
            self.pending.fetch_sub(1, Ordering::Relaxed);
        }
    }
}

/* ───────────────────────────── transcriber ───────────────────────────── */

struct Transcriber {
    app: AppHandle,
    store: Arc<MeetingStore>,
    meeting_id: i64,
    rx: Receiver<TrackUtterance>,
    pending: Arc<AtomicU32>,
}

/// Recent segments kept to spot the microphone echoing the speakers.
const ECHO_MEMORY_MS: i64 = 60_000;

impl Transcriber {
    fn run(self) {
        let mut recent: Vec<MeetingSegment> = Vec::new();
        while let Ok(utterance) = self.rx.recv() {
            let text = self.transcribe(utterance.samples);
            self.pending.fetch_sub(1, Ordering::Relaxed);
            let Some(text) = text else { continue };
            let text = text.trim().to_string();
            if is_noise(&text) {
                debug!("Dropped a noise segment from the meeting transcript");
                continue;
            }

            let candidate = MeetingSegment {
                id: 0,
                speaker: utterance.speaker,
                start_ms: utterance.start_ms as i64,
                end_ms: utterance.end_ms as i64,
                text,
            };

            match candidate.speaker {
                Speaker::Me => {
                    if recent
                        .iter()
                        .any(|o| o.speaker == Speaker::Others && is_echo(&candidate, o))
                    {
                        debug!("Dropped a microphone segment echoing the speakers");
                        continue;
                    }
                }
                Speaker::Others => {
                    let echoes: Vec<i64> = recent
                        .iter()
                        .filter(|m| m.speaker == Speaker::Me && is_echo(m, &candidate))
                        .map(|m| m.id)
                        .collect();
                    for id in echoes {
                        if self.store.delete_segment(id).is_ok() {
                            recent.retain(|s| s.id != id);
                            let _ = MeetingSegmentRemovedEvent {
                                meeting_id: self.meeting_id,
                                segment_id: id,
                            }
                            .emit(&self.app);
                        }
                    }
                }
            }

            match self.store.add_segment(
                self.meeting_id,
                candidate.speaker,
                candidate.start_ms,
                candidate.end_ms,
                &candidate.text,
            ) {
                Ok(segment) => {
                    let _ = MeetingSegmentEvent {
                        meeting_id: self.meeting_id,
                        segment: segment.clone(),
                    }
                    .emit(&self.app);
                    let horizon = segment.end_ms - ECHO_MEMORY_MS;
                    recent.retain(|s| s.end_ms >= horizon);
                    recent.push(segment);
                }
                Err(e) => warn!("Failed to store a meeting segment: {e}"),
            }
        }
    }

    /// The utterance's text, retrying while the model (re)loads.
    fn transcribe(&self, samples: Vec<f32>) -> Option<String> {
        let tm = self.app.try_state::<Arc<TranscriptionManager>>()?;
        let started = Instant::now();
        for attempt in 0..4 {
            if !tm.is_model_loaded() {
                tm.initiate_model_load();
            }
            match tm.transcribe(samples.clone()) {
                Ok(text) => {
                    debug!(
                        "Meeting utterance ({:.1}s) transcribed in {:?}",
                        samples.len() as f64 / RATE as f64,
                        started.elapsed()
                    );
                    return Some(text);
                }
                Err(e) => {
                    warn!(
                        "Meeting utterance transcription failed (attempt {}): {e}",
                        attempt + 1
                    );
                    std::thread::sleep(Duration::from_secs(2));
                }
            }
        }
        None
    }
}
