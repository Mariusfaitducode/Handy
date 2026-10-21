//! The background thread that reads the calendar every few seconds, asks
//! [`schedule::Tracker`] what to do, and does it: notifications, starting
//! and stopping meetings.

use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use chrono::{Duration as ChronoDuration, Local, TimeZone};
use log::{debug, info, warn};
use tauri::{AppHandle, Emitter, Manager};
use tauri_specta::Event;

use super::bridge::{self, Notification, NotificationAction};
use super::schedule::{self, Action, Candidate, MeetingView, Tracker};
use super::strings::{self, Strings};
use super::{
    CalendarAccess, CalendarAnswer, CalendarChangedEvent, CalendarMode, CalendarPromptEvent,
    CalendarPromptKind, CalendarSettings, UpcomingMeeting,
};
use crate::meeting::{self, MeetingError, MeetingPhase, MeetingSegmentEvent, MeetingSource};
use crate::settings::{get_settings, write_settings};

/// How often the calendar is read when nothing else wakes the thread.
const TICK: Duration = Duration::from_secs(15);
/// "Upcoming meetings" shows at most this many events.
const UPCOMING_LIMIT: usize = 6;
/// Frontend event that shows the Meetings section (a notification was
/// clicked).
pub const OPEN_EVENT: &str = "calendar-open";

enum Msg {
    /// Re-read the calendar now (settings or access changed, a meeting
    /// started or stopped).
    Wake,
    /// Something was said in this meeting.
    Activity(i64),
    /// A notification button (or the notification itself) was clicked.
    Notification { action: String, id: String },
}

static SENDER: OnceLock<Mutex<Sender<Msg>>> = OnceLock::new();

fn send(msg: Msg) {
    if let Some(tx) = SENDER.get() {
        let _ = tx.lock().unwrap_or_else(|p| p.into_inner()).send(msg);
    }
}

/// Re-reads the calendar right away.
pub fn wake() {
    send(Msg::Wake);
}

/// What the commands read; written by the scheduler thread.
#[derive(Default)]
pub struct CalendarState {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    tracker: Tracker,
    candidates: Vec<Candidate>,
    upcoming: Vec<UpcomingMeeting>,
}

impl CalendarState {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn upcoming(&self) -> Vec<UpcomingMeeting> {
        self.lock().upcoming.clone()
    }
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Starts the scheduler. Safe on every platform: off macOS it never finds a
/// calendar and stays idle.
pub fn init(app: &AppHandle) {
    let (tx, rx) = mpsc::channel();
    if SENDER.set(Mutex::new(tx)).is_err() {
        return;
    }
    let state = Arc::new(CalendarState::default());
    app.manage(state.clone());

    let notifications = bridge::setup_notifications(|action, id| {
        send(Msg::Notification {
            action: action.to_string(),
            id: id.to_string(),
        })
    });
    debug!("Calendar notifications available: {notifications}");

    MeetingSegmentEvent::listen_any(app, |event| send(Msg::Activity(event.payload.meeting_id)));
    meeting::MeetingStatusEvent::listen_any(app, |_| wake());

    let app = app.clone();
    std::thread::Builder::new()
        .name("calendar".into())
        .spawn(move || loop {
            match rx.recv_timeout(TICK) {
                Ok(Msg::Activity(id)) => state.lock().tracker.activity(id, now_ms()),
                Ok(Msg::Notification { action, id }) => {
                    on_notification(&app, &state, &action, &id);
                    tick(&app, &state);
                }
                Ok(Msg::Wake) | Err(RecvTimeoutError::Timeout) => tick(&app, &state),
                Err(RecvTimeoutError::Disconnected) => break,
            }
        })
        .map(|_| ())
        .unwrap_or_else(|e| warn!("Failed to start the calendar thread: {e}"));
}

fn meeting_view(app: &AppHandle) -> MeetingView {
    let status = meeting::current_status(app);
    MeetingView {
        busy: status.phase != MeetingPhase::Idle,
        recording: status.phase == MeetingPhase::Recording,
        meeting_id: status.meeting_id,
    }
}

/// End of tomorrow, local time: "Upcoming meetings" stops there.
fn upcoming_until(now: i64) -> i64 {
    Local
        .timestamp_millis_opt(now)
        .single()
        .and_then(|t| (t.date_naive() + ChronoDuration::days(2)).and_hms_opt(0, 0, 0))
        .and_then(|t| Local.from_local_datetime(&t).earliest())
        .map(|t| t.timestamp_millis())
        .unwrap_or(now + 36 * 3_600_000)
}

fn tick(app: &AppHandle, state: &CalendarState) {
    let settings = get_settings(app).calendar;
    let now = now_ms();
    let candidates =
        if settings.mode != CalendarMode::Off && bridge::access() == CalendarAccess::Granted {
            match bridge::events(now - 12 * 3_600_000, upcoming_until(now)) {
                Ok(events) => schedule::candidates(&events, &settings, now),
                Err(e) => {
                    warn!("Failed to read the calendar: {e}");
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };
    let view = meeting_view(app);

    let (actions, changed) = {
        let mut inner = state.lock();
        let actions = inner.tracker.plan(now, &candidates, settings.mode, view);
        let upcoming = schedule::upcoming(
            &candidates,
            upcoming_until(now),
            UPCOMING_LIMIT,
            inner.tracker.recording_key(),
        );
        let changed = upcoming != inner.upcoming;
        inner.upcoming = upcoming;
        inner.candidates = candidates;
        (actions, changed)
    };
    for action in actions {
        run(app, state, action);
    }
    if changed {
        let _ = CalendarChangedEvent.emit(app);
    }
}

fn texts(app: &AppHandle) -> &'static Strings {
    strings::for_language(&get_settings(app).app_language)
}

fn display_title<'a>(title: &'a str, s: &'static Strings) -> &'a str {
    if title.trim().is_empty() {
        s.untitled
    } else {
        title
    }
}

fn soon_id(key: &str) -> String {
    format!("soon:{key}")
}

fn ended_id(key: &str) -> String {
    format!("ended:{key}")
}

/// "10:00–10:30 · Google Meet"
fn when_and_where(c: &Candidate) -> String {
    let time = |ms| {
        Local
            .timestamp_millis_opt(ms)
            .single()
            .map(|t| t.format("%H:%M").to_string())
            .unwrap_or_default()
    };
    let mut out = format!("{}–{}", time(c.start_ms), time(c.end_ms));
    if let Some(video) = &c.video {
        out.push_str(" · ");
        out.push_str(provider_name(&video.provider));
    }
    out
}

fn provider_name(id: &str) -> &'static str {
    match id {
        "meet" => "Google Meet",
        "zoom" => "Zoom",
        "teams" => "Microsoft Teams",
        "webex" => "Webex",
        "facetime" => "FaceTime",
        "whereby" => "Whereby",
        "jitsi" => "Jitsi",
        "around" => "Around",
        "slack" => "Slack",
        _ => "Visio",
    }
}

/// Shows a notification, or the same prompt in the window when
/// notifications aren't available.
fn prompt(
    app: &AppHandle,
    notification: &Notification<'_>,
    fallback: Option<(CalendarPromptKind, &str, &str)>,
) {
    let allowed = bridge::notification_access() == super::NotificationAccess::Granted;
    if allowed && bridge::post(notification) {
        return;
    }
    if let Some((kind, key, title)) = fallback {
        let _ = CalendarPromptEvent {
            kind,
            key: key.to_string(),
            title: title.to_string(),
        }
        .emit(app);
    }
}

fn run(app: &AppHandle, state: &CalendarState, action: Action) {
    let s = texts(app);
    match action {
        Action::PromptSoon(c) => {
            let title = display_title(&c.title, s);
            let body = format!("{}\n{}", when_and_where(&c), s.soon_body_ask);
            info!("Calendar: offering to record \"{}\"", c.title);
            prompt(
                app,
                &Notification {
                    id: &soon_id(&c.key),
                    title: &(s.soon_title)(title),
                    body: &body,
                    category: "meeting-soon",
                    actions: &[
                        NotificationAction {
                            id: "record",
                            title: s.record,
                            destructive: false,
                        },
                        NotificationAction {
                            id: "ignore",
                            title: s.ignore,
                            destructive: false,
                        },
                    ],
                },
                Some((CalendarPromptKind::Soon, &c.key, title)),
            );
        }
        Action::AnnounceAuto(c) => {
            let title = display_title(&c.title, s);
            let body = format!("{}\n{}", when_and_where(&c), s.soon_body_auto);
            prompt(
                app,
                &Notification {
                    id: &soon_id(&c.key),
                    title: &(s.soon_title)(title),
                    body: &body,
                    category: "meeting-auto",
                    actions: &[NotificationAction {
                        id: "ignore",
                        title: s.dont_record,
                        destructive: false,
                    }],
                },
                Some((CalendarPromptKind::AutoSoon, &c.key, title)),
            );
        }
        Action::Start(c) => start(app, state, &c),
        Action::Withdraw(key) => bridge::remove(&soon_id(&key)),
        Action::PromptEnded { key, title } => {
            let title = display_title(&title, s);
            info!("Calendar: \"{title}\" seems over, asking to stop");
            prompt(
                app,
                &Notification {
                    id: &ended_id(&key),
                    title: &(s.ended_title)(title),
                    body: s.ended_body,
                    category: "meeting-ended",
                    actions: &[
                        NotificationAction {
                            id: "stop",
                            title: s.stop,
                            destructive: true,
                        },
                        NotificationAction {
                            id: "keep",
                            title: s.keep,
                            destructive: false,
                        },
                    ],
                },
                Some((CalendarPromptKind::Ended, &key, title)),
            );
        }
        Action::WithdrawEnded(key) => bridge::remove(&ended_id(&key)),
        Action::Stop { key, title } => {
            info!("Calendar: stopping \"{title}\" after a long silence");
            meeting::stop_meeting_reporting_errors(app);
            let title = display_title(&title, s);
            prompt(
                app,
                &Notification {
                    id: &format!("stopped:{key}"),
                    title: &(s.stopped_title)(title),
                    body: s.stopped_body,
                    category: "meeting-info",
                    actions: &[],
                },
                None,
            );
        }
    }
}

fn start(app: &AppHandle, state: &CalendarState, c: &Candidate) {
    let s = texts(app);
    bridge::remove(&soon_id(&c.key));
    let title = (!c.title.is_empty()).then(|| c.title.clone());
    let source = MeetingSource::Calendar {
        event_id: c.key.clone(),
    };
    match meeting::start_meeting(app, title, source) {
        Ok(status) => {
            let Some(id) = status.meeting_id else {
                return;
            };
            info!("Calendar: recording \"{}\" as meeting {id}", c.title);
            state.lock().tracker.started(c, id, now_ms());
            if !c.participants.is_empty() {
                if let Err(e) = meeting::set_meeting_participants(app, id, &c.participants) {
                    warn!("Failed to save the participants of meeting {id}: {e}");
                }
            }
        }
        Err(e @ (MeetingError::AlreadyRecording | MeetingError::StillFinishing)) => {
            debug!("Calendar: not starting \"{}\": {e}", c.title);
            state.lock().tracker.start_failed(&c.key);
            prompt(
                app,
                &Notification {
                    id: &format!("busy:{}", c.key),
                    title: &(s.soon_title)(display_title(&c.title, s)),
                    body: s.busy_body,
                    category: "meeting-info",
                    actions: &[],
                },
                None,
            );
        }
        Err(e) => {
            state.lock().tracker.start_failed(&c.key);
            meeting::report_error(app, &e);
        }
    }
}

fn on_notification(app: &AppHandle, state: &CalendarState, action: &str, id: &str) {
    let Some((kind, key)) = id.split_once(':') else {
        return;
    };
    if action == "open" {
        crate::show_main_window(app);
        let _ = app.emit(OPEN_EVENT, ());
        return;
    }
    let Some(answer) = CalendarAnswer::parse(action) else {
        return;
    };
    match kind {
        "soon" | "ended" => answer_prompt(app, state, key, answer),
        _ => {}
    }
}

/// Carries out the user's answer to a prompt (from a notification or from
/// the window).
pub(super) fn answer_prompt(
    app: &AppHandle,
    state: &CalendarState,
    key: &str,
    answer: CalendarAnswer,
) {
    match answer {
        CalendarAnswer::Record => {
            let candidate = state
                .lock()
                .candidates
                .iter()
                .find(|c| c.key == key)
                .cloned();
            match candidate {
                Some(c) => start(app, state, &c),
                None => warn!("Calendar: the event to record is gone ({key})"),
            }
        }
        CalendarAnswer::Ignore => {
            bridge::remove(&soon_id(key));
            state.lock().tracker.dismissed(key);
            let mut settings = get_settings(app);
            settings.calendar.set_choice(key, Some(false), now_ms());
            write_settings(app, settings);
            let _ = CalendarChangedEvent.emit(app);
        }
        CalendarAnswer::Stop => {
            bridge::remove(&ended_id(key));
            meeting::stop_meeting_reporting_errors(app);
        }
        CalendarAnswer::Keep => {
            bridge::remove(&ended_id(key));
            state.lock().tracker.keep(now_ms());
        }
    }
}

/// Applies the user's choice for one occurrence from "Upcoming meetings".
pub(super) fn set_choice(app: &AppHandle, key: &str, record: Option<bool>) {
    if record != Some(false) {
        // Changing their mind after "Ignore": offer it again.
        if let Some(state) = app.try_state::<Arc<CalendarState>>() {
            state.lock().tracker.forget(key);
        }
    }
    let mut settings = get_settings(app);
    settings.calendar.set_choice(key, record, now_ms());
    write_settings(app, settings);
    let _ = CalendarChangedEvent.emit(app);
    wake();
}

/// Keeps the per-event choices the scheduler wrote; takes everything else
/// from the UI.
pub(super) fn update_settings(app: &AppHandle, new: CalendarSettings) {
    let mut settings = get_settings(app);
    let lists = (
        std::mem::take(&mut settings.calendar.skipped_events),
        std::mem::take(&mut settings.calendar.included_events),
    );
    settings.calendar = CalendarSettings {
        skipped_events: lists.0,
        included_events: lists.1,
        ..new
    };
    write_settings(app, settings);
    let _ = CalendarChangedEvent.emit(app);
    wake();
}
