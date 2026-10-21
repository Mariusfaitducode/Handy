//! Meeting mode: record a call end to end, transcribe it locally as it goes,
//! and write its minutes when it ends.
//!
//! Two tracks are captured at once, the microphone ("Me") and the computer's
//! audio output ("Participants"), so the transcript knows who spoke without
//! any diarization model. Each track is cut into utterances at pauses
//! ([`segmenter`]) and every utterance is transcribed by the regular
//! transcription engine while the meeting goes on, so the transcript is on
//! screen live and stopping only waits for the last few sentences.
//!
//! # Internal API
//!
//! Other features (calendar auto-start) drive meetings through
//! [`start_meeting`], [`stop_meeting`], [`pause_meeting`], [`resume_meeting`]
//! and [`current_status`], and follow them through the
//! [`MeetingStatusEvent`] / [`MeetingSegmentEvent`] / [`MeetingUpdatedEvent`]
//! events. All of them are safe to call from any thread.
//!
//! # Privacy
//!
//! Audio stays in memory and is dropped once transcribed; only text is stored
//! (`meetings.db`). The transcript only leaves the machine for the summary,
//! and only to the language model the user configured for post-processing.

mod capture;
pub mod commands;
mod indicator;
mod segmenter;
mod store;
mod summary;
mod system_audio;
mod transcript;

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use log::{error, info, warn};
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{AppHandle, Manager};
use tauri_specta::Event;

use crate::settings::{get_settings, AppSettings};
use capture::{CaptureSession, CaptureStart};
pub use store::{
    MeetingRecord, MeetingRecordStatus, MeetingStore, SummaryStatus, SystemAudioState,
};
pub use system_audio::SystemAudioPermission;
pub use transcript::MeetingSegment;

/// True from the moment a meeting starts until its last utterance is
/// transcribed. The transcription manager keeps the model loaded meanwhile.
static MEETING_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Whether a meeting is recording or still transcribing its end.
pub fn is_meeting_active() -> bool {
    MEETING_ACTIVE.load(Ordering::Acquire)
}

/* ───────────────────────────── settings ───────────────────────────── */

/// How long meetings are kept before they are deleted automatically.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type, Default)]
#[serde(rename_all = "snake_case")]
pub enum MeetingRetention {
    #[default]
    Forever,
    Days30,
    Days90,
    Year,
}

impl MeetingRetention {
    fn days(self) -> Option<i64> {
        match self {
            Self::Forever => None,
            Self::Days30 => Some(30),
            Self::Days90 => Some(90),
            Self::Year => Some(365),
        }
    }
}

/// Meeting preferences, stored as one object in the app settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(default)]
pub struct MeetingSettings {
    /// Write the minutes when a meeting ends (needs a configured model).
    pub auto_summary: bool,
    /// Folder Markdown exports go to; asked for on first export.
    pub export_dir: Option<String>,
    /// Save each finished meeting as Markdown in `export_dir`.
    pub auto_export: bool,
    pub retention: MeetingRetention,
    /// The floating recording indicator. Off for people who share their
    /// screen and don't want it in the share.
    pub show_indicator: bool,
}

impl Default for MeetingSettings {
    fn default() -> Self {
        Self {
            auto_summary: true,
            export_dir: None,
            auto_export: false,
            retention: MeetingRetention::Forever,
            show_indicator: true,
        }
    }
}

/* ───────────────────────────── public types ───────────────────────────── */

/// What started a meeting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MeetingSource {
    /// The Start button in the app.
    Manual,
    Shortcut,
    Tray,
    Cli,
    /// A calendar event; `event_id` is the calendar's own identifier.
    Calendar {
        event_id: String,
    },
}

impl MeetingSource {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Shortcut => "shortcut",
            Self::Tray => "tray",
            Self::Cli => "cli",
            Self::Calendar { .. } => "calendar",
        }
    }

    fn calendar_event_id(&self) -> Option<&str> {
        match self {
            Self::Calendar { event_id } => Some(event_id),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum MeetingPhase {
    Idle,
    Recording,
    Paused,
    /// Capture stopped; the last utterances are being transcribed.
    Finishing,
}

/// Snapshot of the meeting in progress.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct MeetingStatus {
    pub phase: MeetingPhase,
    pub meeting_id: Option<i64>,
    pub title: Option<String>,
    /// Unix milliseconds.
    pub started_at: Option<i64>,
    /// Recorded time (pauses excluded) as of `as_of`. While recording, add
    /// the time since `as_of` to get the live value.
    pub elapsed_ms: i64,
    pub as_of: i64,
    pub system_audio: Option<SystemAudioState>,
    /// Utterances cut but not transcribed yet.
    pub pending_utterances: u32,
}

impl MeetingStatus {
    fn idle() -> Self {
        Self {
            phase: MeetingPhase::Idle,
            meeting_id: None,
            title: None,
            started_at: None,
            elapsed_ms: 0,
            as_of: now_ms(),
            system_audio: None,
            pending_utterances: 0,
        }
    }
}

/// Why a meeting couldn't start. Serialized as the snake_case code the UI
/// translates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MeetingError {
    AlreadyRecording,
    /// The previous meeting is still transcribing its last sentences.
    StillFinishing,
    NotRecording,
    NoModel,
    MicrophonePermissionDenied,
    NoInputDevice,
    Other(String),
}

impl MeetingError {
    pub fn code(&self) -> String {
        match self {
            Self::AlreadyRecording => "already_recording".into(),
            Self::StillFinishing => "still_finishing".into(),
            Self::NotRecording => "not_recording".into(),
            Self::NoModel => "no_model".into(),
            Self::MicrophonePermissionDenied => "microphone_permission_denied".into(),
            Self::NoInputDevice => "no_input_device".into(),
            Self::Other(detail) => format!("unknown:{detail}"),
        }
    }
}

impl std::fmt::Display for MeetingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.code())
    }
}

/* ───────────────────────────── events ───────────────────────────── */

/// Emitted whenever the meeting in progress changes phase.
#[derive(Debug, Clone, Serialize, Deserialize, Type, tauri_specta::Event)]
pub struct MeetingStatusEvent(pub MeetingStatus);

/// A segment was added to a meeting's transcript.
#[derive(Debug, Clone, Serialize, Deserialize, Type, tauri_specta::Event)]
pub struct MeetingSegmentEvent {
    pub meeting_id: i64,
    pub segment: MeetingSegment,
}

/// A segment was removed (it was the microphone echoing the speakers).
#[derive(Debug, Clone, Serialize, Deserialize, Type, tauri_specta::Event)]
pub struct MeetingSegmentRemovedEvent {
    pub meeting_id: i64,
    pub segment_id: i64,
}

/// A meeting's record changed (title, summary, status) or it was deleted:
/// refetch it.
#[derive(Debug, Clone, Serialize, Deserialize, Type, tauri_specta::Event)]
pub struct MeetingUpdatedEvent {
    pub meeting_id: i64,
    pub deleted: bool,
}

/* ───────────────────────────── manager ───────────────────────────── */

/// Recorded time, pauses excluded.
#[derive(Debug)]
pub(crate) struct MeetingClock {
    accumulated: Duration,
    running_since: Option<Instant>,
}

impl MeetingClock {
    fn started() -> Self {
        Self {
            accumulated: Duration::ZERO,
            running_since: Some(Instant::now()),
        }
    }

    pub(crate) fn elapsed(&self) -> Duration {
        self.accumulated + self.running_since.map_or(Duration::ZERO, |t| t.elapsed())
    }

    fn pause(&mut self) {
        if let Some(t) = self.running_since.take() {
            self.accumulated += t.elapsed();
        }
    }

    fn resume(&mut self) {
        if self.running_since.is_none() {
            self.running_since = Some(Instant::now());
        }
    }

    fn is_running(&self) -> bool {
        self.running_since.is_some()
    }
}

struct ActiveMeeting {
    id: i64,
    title: Option<String>,
    started_at: i64,
    clock: Arc<Mutex<MeetingClock>>,
    paused: Arc<AtomicBool>,
    system_audio: Arc<Mutex<SystemAudioState>>,
    pending: Arc<AtomicU32>,
    session: CaptureSession,
}

struct Finishing {
    id: i64,
    title: Option<String>,
    started_at: i64,
    elapsed_ms: i64,
    system_audio: SystemAudioState,
    pending: Arc<AtomicU32>,
}

enum Phase {
    Idle,
    Active(ActiveMeeting),
    Finishing(Finishing),
}

pub struct MeetingManager {
    app: AppHandle,
    store: Arc<MeetingStore>,
    phase: Mutex<Phase>,
    summaries_running: Mutex<HashSet<i64>>,
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

impl MeetingManager {
    pub fn new(app: &AppHandle) -> anyhow::Result<Self> {
        let dir = crate::portable::app_data_dir(app)?;
        std::fs::create_dir_all(&dir)?;
        let store = MeetingStore::open(&MeetingStore::default_path(&dir))?;
        match store.mark_interrupted() {
            Ok(ids) if !ids.is_empty() => {
                info!("Marked {} unfinished meeting(s) as interrupted", ids.len())
            }
            Ok(_) => {}
            Err(e) => warn!("Failed to mark interrupted meetings: {e}"),
        }
        let manager = Self {
            app: app.clone(),
            store: Arc::new(store),
            phase: Mutex::new(Phase::Idle),
            summaries_running: Mutex::new(HashSet::new()),
        };
        manager.apply_retention();
        Ok(manager)
    }

    pub fn store(&self) -> &MeetingStore {
        &self.store
    }

    fn lock_phase(&self) -> std::sync::MutexGuard<'_, Phase> {
        self.phase.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn status(&self) -> MeetingStatus {
        let phase = self.lock_phase();
        match &*phase {
            Phase::Idle => MeetingStatus::idle(),
            Phase::Active(m) => {
                let clock = m.clock.lock().unwrap_or_else(|p| p.into_inner());
                MeetingStatus {
                    phase: if clock.is_running() {
                        MeetingPhase::Recording
                    } else {
                        MeetingPhase::Paused
                    },
                    meeting_id: Some(m.id),
                    title: m.title.clone(),
                    started_at: Some(m.started_at),
                    elapsed_ms: clock.elapsed().as_millis() as i64,
                    as_of: now_ms(),
                    system_audio: Some(*m.system_audio.lock().unwrap_or_else(|p| p.into_inner())),
                    pending_utterances: m.pending.load(Ordering::Relaxed),
                }
            }
            Phase::Finishing(f) => MeetingStatus {
                phase: MeetingPhase::Finishing,
                meeting_id: Some(f.id),
                title: f.title.clone(),
                started_at: Some(f.started_at),
                elapsed_ms: f.elapsed_ms,
                as_of: now_ms(),
                system_audio: Some(f.system_audio),
                pending_utterances: f.pending.load(Ordering::Relaxed),
            },
        }
    }

    fn emit_status(&self) {
        let status = self.status();
        if let Err(e) = MeetingStatusEvent(status.clone()).emit(&self.app) {
            warn!("Failed to emit meeting status: {e}");
        }
        crate::tray::update_tray_menu(&self.app);
        indicator::sync(&self.app, &status);
    }

    fn emit_updated(&self, meeting_id: i64, deleted: bool) {
        let _ = MeetingUpdatedEvent {
            meeting_id,
            deleted,
        }
        .emit(&self.app);
    }

    fn start(
        &self,
        title: Option<String>,
        source: MeetingSource,
    ) -> Result<MeetingStatus, MeetingError> {
        let mut phase = self.lock_phase();
        match &*phase {
            Phase::Active(_) => return Err(MeetingError::AlreadyRecording),
            Phase::Finishing(_) => return Err(MeetingError::StillFinishing),
            Phase::Idle => {}
        }

        let settings = get_settings(&self.app);
        ensure_model_available(&self.app, &settings)?;

        let title = title
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty());
        let clock = Arc::new(Mutex::new(MeetingClock::started()));
        let paused = Arc::new(AtomicBool::new(false));
        let pending = Arc::new(AtomicU32::new(0));
        let started_at = now_ms();

        // The meeting row must exist before the first segment lands.
        let system_audio_guess = if system_audio::is_supported() {
            SystemAudioState::Captured
        } else {
            SystemAudioState::Unsupported
        };
        let id = self
            .store
            .create(&store::NewMeeting {
                title: title.as_deref(),
                source: source.as_str(),
                calendar_event_id: source.calendar_event_id(),
                started_at,
                system_audio: system_audio_guess,
            })
            .map_err(|e| MeetingError::Other(e.to_string()))?;

        MEETING_ACTIVE.store(true, Ordering::Release);
        let started = CaptureSession::start(CaptureStart {
            app: self.app.clone(),
            store: self.store.clone(),
            meeting_id: id,
            settings: &settings,
            clock: clock.clone(),
            paused: paused.clone(),
            pending: pending.clone(),
        });
        let (session, system_state) = match started {
            Ok(started) => started,
            Err(e) => {
                MEETING_ACTIVE.store(false, Ordering::Release);
                let _ = self.store.delete(id);
                return Err(e);
            }
        };
        if system_state != system_audio_guess {
            let _ = self.store.set_system_audio(id, system_state);
        }

        // Make sure the model is warm before the first utterance.
        if let Some(tm) = self
            .app
            .try_state::<Arc<crate::managers::transcription::TranscriptionManager>>()
        {
            tm.initiate_model_load();
        }

        info!(
            "Meeting {id} started ({}), system audio: {}",
            source.as_str(),
            system_state.as_str()
        );
        let system_audio = Arc::new(Mutex::new(system_state));
        session.watch_system_permission(system_audio.clone(), {
            let app = self.app.clone();
            move |state| {
                if let Some(manager) = app.try_state::<Arc<MeetingManager>>() {
                    let _ = manager.store.set_system_audio(id, state);
                    manager.emit_status();
                    manager.emit_updated(id, false);
                }
            }
        });
        *phase = Phase::Active(ActiveMeeting {
            id,
            title,
            started_at,
            clock,
            paused,
            system_audio,
            pending,
            session,
        });
        drop(phase);

        self.emit_updated(id, false);
        self.emit_status();
        Ok(self.status())
    }

    fn stop(self: &Arc<Self>) -> Result<i64, MeetingError> {
        let mut phase = self.lock_phase();
        let active = match std::mem::replace(&mut *phase, Phase::Idle) {
            Phase::Active(active) => active,
            other => {
                *phase = other;
                return Err(MeetingError::NotRecording);
            }
        };
        let elapsed_ms = active
            .clock
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .elapsed()
            .as_millis() as i64;
        let system_audio = *active
            .system_audio
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let id = active.id;
        *phase = Phase::Finishing(Finishing {
            id,
            title: active.title.clone(),
            started_at: active.started_at,
            elapsed_ms,
            system_audio,
            pending: active.pending.clone(),
        });
        drop(phase);

        if let Err(e) = self.store.finish_capture(id, now_ms(), elapsed_ms) {
            error!("Failed to record the end of meeting {id}: {e}");
        }
        info!("Meeting {id} stopped after {}s", elapsed_ms / 1000);
        self.emit_status();
        self.emit_updated(id, false);

        let manager = self.clone();
        std::thread::spawn(move || {
            // Stops capture, flushes both tracks and waits for every
            // utterance to be transcribed.
            active.session.finish(|| manager.emit_status());
            MEETING_ACTIVE.store(false, Ordering::Release);
            {
                let mut phase = manager.lock_phase();
                if matches!(&*phase, Phase::Finishing(f) if f.id == id) {
                    *phase = Phase::Idle;
                }
            }
            if let Some(tm) = manager
                .app
                .try_state::<Arc<crate::managers::transcription::TranscriptionManager>>()
            {
                tm.maybe_unload_immediately("meeting");
            }
            manager.emit_status();
            manager.after_capture(id);
        });
        Ok(id)
    }

    /// Summary, export and retention once a meeting's transcript is final.
    fn after_capture(self: &Arc<Self>, id: i64) {
        let settings = get_settings(&self.app);
        if settings.meeting.auto_summary {
            self.summarize_blocking(id, &settings);
        } else {
            let _ = self
                .store
                .set_summary(id, SummaryStatus::None, None, None, None);
        }
        let _ = self.store.set_status(id, MeetingRecordStatus::Done);
        self.emit_updated(id, false);

        if settings.meeting.auto_export {
            if let Some(dir) = settings.meeting.export_dir.as_deref() {
                if let Err(e) = commands::export_to_dir(&self.app, self, id, dir) {
                    warn!("Automatic export of meeting {id} failed: {e}");
                }
            }
        }
        self.apply_retention();
    }

    fn summarize_blocking(&self, id: i64, settings: &AppSettings) {
        {
            let mut running = self
                .summaries_running
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            if !running.insert(id) {
                return;
            }
        }
        let result = self.summarize_inner(id, settings);
        if let Err(e) = result {
            warn!("Summary of meeting {id} failed: {e}");
        }
        self.summaries_running
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&id);
        self.emit_updated(id, false);
    }

    fn summarize_inner(&self, id: i64, settings: &AppSettings) -> Result<(), String> {
        let record = self
            .store
            .get(id)
            .map_err(|e| e.to_string())?
            .ok_or("meeting not found")?;
        let segments = self.store.segments(id).map_err(|e| e.to_string())?;
        if !summary::has_enough_content(&segments) {
            let _ = self
                .store
                .set_summary(id, SummaryStatus::Empty, None, None, None);
            return Ok(());
        }
        let Some(model) = summary::resolve_model(settings) else {
            let _ = self
                .store
                .set_summary(id, SummaryStatus::NotConfigured, None, None, None);
            return Ok(());
        };
        let label = model.label();
        let _ = self
            .store
            .set_summary(id, SummaryStatus::Pending, None, None, None);
        self.emit_updated(id, false);

        let language = summary::SummaryLanguage::from_app_language(&settings.app_language);
        let input = summary::SummaryInput {
            title: record.title.as_deref(),
            duration_ms: record.duration_ms,
            segments: &segments,
            language,
        };
        let result = tauri::async_runtime::block_on(summary::summarize(&model, &input));
        match result {
            Ok(text) => self
                .store
                .set_summary(id, SummaryStatus::Done, Some(&text), None, Some(&label))
                .map_err(|e| e.to_string()),
            Err(e) => {
                let _ = self
                    .store
                    .set_summary(id, SummaryStatus::Failed, None, Some(&e), None);
                Err(e)
            }
        }
    }

    /// (Re)writes a meeting's summary in the background.
    pub fn regenerate_summary(self: &Arc<Self>, id: i64) -> Result<(), String> {
        if self
            .summaries_running
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .contains(&id)
        {
            return Ok(());
        }
        let record = self
            .store
            .get(id)
            .map_err(|e| e.to_string())?
            .ok_or("meeting not found")?;
        if matches!(
            record.status,
            MeetingRecordStatus::Recording | MeetingRecordStatus::Processing
        ) {
            return Err("meeting_in_progress".into());
        }
        let manager = self.clone();
        std::thread::spawn(move || {
            let settings = get_settings(&manager.app);
            manager.summarize_blocking(id, &settings);
        });
        Ok(())
    }

    fn set_paused(&self, pause: bool) -> Result<(), MeetingError> {
        {
            let phase = self.lock_phase();
            let Phase::Active(active) = &*phase else {
                return Err(MeetingError::NotRecording);
            };
            let mut clock = active.clock.lock().unwrap_or_else(|p| p.into_inner());
            if pause {
                clock.pause();
            } else {
                clock.resume();
            }
            active.paused.store(pause, Ordering::Release);
            let elapsed = clock.elapsed().as_millis() as i64;
            let _ = self.store.set_duration(active.id, elapsed);
        }
        self.emit_status();
        Ok(())
    }

    pub fn delete(&self, id: i64) -> Result<(), String> {
        let busy = match &*self.lock_phase() {
            Phase::Active(m) => m.id == id,
            Phase::Finishing(f) => f.id == id,
            Phase::Idle => false,
        };
        if busy {
            return Err("meeting_in_progress".into());
        }
        self.store.delete(id).map_err(|e| e.to_string())?;
        self.emit_updated(id, true);
        Ok(())
    }

    pub fn rename(&self, id: i64, title: Option<String>) -> Result<(), String> {
        let title = title
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty());
        self.store
            .rename(id, title.as_deref())
            .map_err(|e| e.to_string())?;
        let changed_active = {
            let mut phase = self.lock_phase();
            match &mut *phase {
                Phase::Active(m) if m.id == id => {
                    m.title = title.clone();
                    true
                }
                Phase::Finishing(f) if f.id == id => {
                    f.title = title.clone();
                    true
                }
                _ => false,
            }
        };
        if changed_active {
            self.emit_status();
        }
        self.emit_updated(id, false);
        Ok(())
    }

    fn apply_retention(&self) {
        let retention = get_settings(&self.app).meeting.retention;
        let Some(days) = retention.days() else {
            return;
        };
        let cutoff = now_ms() - days * 24 * 3600 * 1000;
        match self.store.delete_older_than(cutoff) {
            Ok(0) => {}
            Ok(n) => info!("Deleted {n} meeting(s) older than {days} days"),
            Err(e) => warn!("Meeting retention cleanup failed: {e}"),
        }
    }
}

/// A transcription model must be downloaded before a meeting can be
/// transcribed.
fn ensure_model_available(app: &AppHandle, settings: &AppSettings) -> Result<(), MeetingError> {
    if settings.selected_model.trim().is_empty() {
        return Err(MeetingError::NoModel);
    }
    let Some(models) = app.try_state::<Arc<crate::managers::model::ModelManager>>() else {
        return Err(MeetingError::NoModel);
    };
    match models.get_model_info(&settings.selected_model) {
        Some(info) if info.is_downloaded => Ok(()),
        _ => Err(MeetingError::NoModel),
    }
}

fn manager(app: &AppHandle) -> Result<Arc<MeetingManager>, MeetingError> {
    app.try_state::<Arc<MeetingManager>>()
        .map(|m| m.inner().clone())
        .ok_or_else(|| MeetingError::Other("meeting manager not initialized".into()))
}

/* ───────────────────────────── API ───────────────────────────── */

/// Starts recording a meeting. `title` may be empty; the UI then names it by
/// its date. Fails if a meeting is already recording or finishing.
pub fn start_meeting(
    app: &AppHandle,
    title: Option<String>,
    source: MeetingSource,
) -> Result<MeetingStatus, MeetingError> {
    manager(app)?.start(title, source)
}

/// Stops the meeting in progress and returns its id. Returns immediately; the
/// last utterances are transcribed and the summary written in the background
/// (follow [`MeetingStatusEvent`] and [`MeetingUpdatedEvent`]).
pub fn stop_meeting(app: &AppHandle) -> Result<i64, MeetingError> {
    manager(app)?.stop()
}

pub fn pause_meeting(app: &AppHandle) -> Result<(), MeetingError> {
    manager(app)?.set_paused(true)
}

pub fn resume_meeting(app: &AppHandle) -> Result<(), MeetingError> {
    manager(app)?.set_paused(false)
}

/// Starts a meeting if none is recording, stops it otherwise.
pub fn toggle_meeting(app: &AppHandle, source: MeetingSource) -> Result<(), MeetingError> {
    let manager = manager(app)?;
    match manager.status().phase {
        MeetingPhase::Recording | MeetingPhase::Paused => manager.stop().map(|_| ()),
        MeetingPhase::Idle => manager.start(None, source).map(|_| ()),
        MeetingPhase::Finishing => Err(MeetingError::StillFinishing),
    }
}

pub fn current_status(app: &AppHandle) -> MeetingStatus {
    manager(app)
        .map(|m| m.status())
        .unwrap_or_else(|_| MeetingStatus::idle())
}

/// Starts or toggles a meeting from outside the UI (shortcut, tray, CLI) and
/// reports a failure to the user, since there is no button to show it on.
pub fn toggle_meeting_reporting_errors(app: &AppHandle, source: MeetingSource) {
    if let Err(e) = toggle_meeting(app, source) {
        report_error(app, &e);
    }
}

pub fn start_meeting_reporting_errors(
    app: &AppHandle,
    title: Option<String>,
    source: MeetingSource,
) {
    match start_meeting(app, title, source) {
        Ok(_) | Err(MeetingError::AlreadyRecording) => {}
        Err(e) => report_error(app, &e),
    }
}

pub fn stop_meeting_reporting_errors(app: &AppHandle) {
    match stop_meeting(app) {
        Ok(_) | Err(MeetingError::NotRecording) => {}
        Err(e) => report_error(app, &e),
    }
}

/// Shown by the main window as a toast.
#[derive(Debug, Clone, Serialize, Deserialize, Type, tauri_specta::Event)]
pub struct MeetingErrorEvent {
    pub code: String,
}

fn report_error(app: &AppHandle, error: &MeetingError) {
    warn!("Meeting action failed: {error}");
    let _ = MeetingErrorEvent { code: error.code() }.emit(app);
}

/* ───────────────────────────── CLI ───────────────────────────── */

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CliMeetingAction {
    Toggle,
    Start,
    Stop,
}

/// The meeting flag in a forwarded command line, with `--meeting-title`.
pub fn parse_cli_args(args: &[String]) -> Option<(CliMeetingAction, Option<String>)> {
    let action = if args.iter().any(|a| a == "--toggle-meeting") {
        CliMeetingAction::Toggle
    } else if args.iter().any(|a| a == "--start-meeting") {
        CliMeetingAction::Start
    } else if args.iter().any(|a| a == "--stop-meeting") {
        CliMeetingAction::Stop
    } else {
        return None;
    };
    let mut title = None;
    for (i, arg) in args.iter().enumerate() {
        if let Some(value) = arg.strip_prefix("--meeting-title=") {
            title = Some(value.to_string());
        } else if arg == "--meeting-title" {
            title = args.get(i + 1).cloned();
        }
    }
    Some((action, title.filter(|t| !t.trim().is_empty())))
}

/// Runs a meeting CLI action on this (running) instance.
pub fn run_cli_action(app: &AppHandle, action: CliMeetingAction, title: Option<String>) {
    let app = app.clone();
    std::thread::spawn(move || match action {
        CliMeetingAction::Start => start_meeting_reporting_errors(&app, title, MeetingSource::Cli),
        CliMeetingAction::Stop => stop_meeting_reporting_errors(&app),
        CliMeetingAction::Toggle => match current_status(&app).phase {
            MeetingPhase::Idle => start_meeting_reporting_errors(&app, title, MeetingSource::Cli),
            _ => stop_meeting_reporting_errors(&app),
        },
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_meeting_flags_parse_with_titles() {
        let args = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(parse_cli_args(&args(&["handy", "--cancel"])), None);
        assert_eq!(
            parse_cli_args(&args(&[
                "handy",
                "--start-meeting",
                "--meeting-title",
                "Point hebdo"
            ])),
            Some((CliMeetingAction::Start, Some("Point hebdo".to_string())))
        );
        assert_eq!(
            parse_cli_args(&args(&[
                "handy",
                "--toggle-meeting",
                "--meeting-title=Démo"
            ])),
            Some((CliMeetingAction::Toggle, Some("Démo".to_string())))
        );
        assert_eq!(
            parse_cli_args(&args(&["handy", "--stop-meeting"])),
            Some((CliMeetingAction::Stop, None))
        );
    }

    #[test]
    fn clock_excludes_pauses() {
        let mut clock = MeetingClock::started();
        std::thread::sleep(Duration::from_millis(20));
        clock.pause();
        let paused_at = clock.elapsed();
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(clock.elapsed(), paused_at);
        assert!(!clock.is_running());
        clock.resume();
        assert!(clock.is_running());
        std::thread::sleep(Duration::from_millis(10));
        assert!(clock.elapsed() > paused_at);
        assert!(clock.elapsed() < paused_at + Duration::from_millis(28));
    }

    #[test]
    fn error_codes_are_stable() {
        assert_eq!(MeetingError::NoModel.code(), "no_model");
        assert_eq!(MeetingError::Other("x".into()).code(), "unknown:x");
    }

    #[test]
    fn settings_default_to_summaries_on_and_keeping_everything() {
        let s = MeetingSettings::default();
        assert!(s.auto_summary);
        assert!(s.show_indicator);
        assert!(!s.auto_export);
        assert_eq!(s.retention, MeetingRetention::Forever);
        assert_eq!(MeetingRetention::Days90.days(), Some(90));
        // A partial stored object fills in the rest.
        let partial: MeetingSettings = serde_json::from_str(r#"{"auto_export":true}"#).unwrap();
        assert!(partial.auto_export && partial.auto_summary);
    }

    #[test]
    fn calendar_source_carries_its_event() {
        let source = MeetingSource::Calendar {
            event_id: "abc".into(),
        };
        assert_eq!(source.as_str(), "calendar");
        assert_eq!(source.calendar_event_id(), Some("abc"));
        let json = serde_json::to_string(&source).unwrap();
        assert_eq!(json, r#"{"kind":"calendar","event_id":"abc"}"#);
    }
}
