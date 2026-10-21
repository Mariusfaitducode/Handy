//! Calendar auto-start: offers to record a video call a minute before it
//! starts, and to stop once it seems over.
//!
//! Events come from the macOS calendar database through EventKit
//! (`swift/calendar_bridge.swift`), so every account added to Calendar.app
//! (iCloud, Google, Exchange…) is covered without any sign-in in Handy. The
//! prompts are native notifications with actions, from the same bridge.
//!
//! The logic lives in pure, unit-tested modules: [`detect`] decides which
//! events are calls worth recording, [`schedule`] decides when to prompt,
//! start and propose stopping. [`scheduler`] runs it every few seconds and
//! carries out its decisions through the meeting API
//! ([`crate::meeting::start_meeting`] and friends). Handy never writes to the
//! calendar.

mod bridge;
pub mod commands;
pub mod detect;
mod schedule;
mod scheduler;
mod strings;

use serde::{Deserialize, Serialize};
use specta::Type;

pub use detect::VideoLink;
pub use scheduler::init;

/// When calendar events turn into meetings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type, Default)]
#[serde(rename_all = "snake_case")]
pub enum CalendarMode {
    /// Not set up, or turned off: the calendar isn't read at all.
    #[default]
    Off,
    /// A notification a minute before asks whether to record.
    Ask,
    /// Recording starts on its own; the notification a minute before lets
    /// the user opt out.
    Auto,
}

/// Calendar preferences, stored as one object in the app settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(default)]
pub struct CalendarSettings {
    pub mode: CalendarMode,
    /// Calendars the user unticked. New calendars are followed by default.
    pub excluded_calendars: Vec<String>,
    /// Only events with a Meet/Zoom/Teams… link are offered.
    pub only_video_calls: bool,
    /// Events with nobody else invited (focus blocks with a link…) are not
    /// offered.
    pub skip_without_attendees: bool,
    /// Occurrences (see [`detect::RawEvent::key`]) the user chose not to
    /// record.
    pub skipped_events: Vec<String>,
    /// Occurrences the user chose to record although the rules above would
    /// skip them.
    pub included_events: Vec<String>,
}

impl Default for CalendarSettings {
    fn default() -> Self {
        Self {
            mode: CalendarMode::Off,
            excluded_calendars: Vec::new(),
            only_video_calls: true,
            skip_without_attendees: true,
            skipped_events: Vec::new(),
            included_events: Vec::new(),
        }
    }
}

impl CalendarSettings {
    /// The user's own choice for an occurrence, if any.
    pub fn choice(&self, key: &str) -> Option<bool> {
        if self.skipped_events.iter().any(|k| k == key) {
            Some(false)
        } else if self.included_events.iter().any(|k| k == key) {
            Some(true)
        } else {
            None
        }
    }

    /// Records (or clears, with `None`) the user's choice for an occurrence,
    /// and forgets choices about occurrences long gone.
    pub fn set_choice(&mut self, key: &str, record: Option<bool>, now_ms: i64) {
        self.skipped_events.retain(|k| k != key);
        self.included_events.retain(|k| k != key);
        match record {
            Some(false) => self.skipped_events.push(key.to_string()),
            Some(true) => self.included_events.push(key.to_string()),
            None => {}
        }
        let keep = |k: &String| occurrence_start(k).is_none_or(|start| start > now_ms - DAY_MS * 2);
        self.skipped_events.retain(keep);
        self.included_events.retain(keep);
    }
}

const DAY_MS: i64 = 24 * 3_600_000;

/// The start (Unix ms) encoded in an occurrence key.
fn occurrence_start(key: &str) -> Option<i64> {
    key.rsplit_once('@')
        .and_then(|(_, start)| start.parse().ok())
}

/// Whether Handy may read the calendar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum CalendarAccess {
    /// Not asked yet.
    NotDetermined,
    Granted,
    /// Refused, or restricted by the administrator, or "add events only".
    Denied,
    /// Not macOS.
    Unsupported,
}

/// Whether Handy's notifications can be shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum NotificationAccess {
    NotDetermined,
    Granted,
    Denied,
    /// Not available in this build (not running from an app bundle, or not
    /// macOS). Prompts then appear in the Handy window only.
    Unavailable,
}

/// A calendar the user can follow or not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct CalendarInfo {
    pub id: String,
    pub title: String,
    /// The account it belongs to ("iCloud", "Google", an address…).
    pub source: String,
    pub color: Option<String>,
    pub followed: bool,
}

/// Where an upcoming event stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum UpcomingDecision {
    /// Will be offered (Ask) or recorded (Auto).
    Record,
    /// The user said not to record this one.
    Skipped,
    /// Nobody else is invited, so it isn't offered; the user can still ask
    /// for it.
    NoAttendees,
}

/// An event in "Upcoming meetings".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct UpcomingMeeting {
    /// Identifies this occurrence.
    pub key: String,
    pub title: String,
    /// Unix milliseconds.
    pub start_ms: i64,
    pub end_ms: i64,
    pub calendar_title: String,
    pub calendar_color: Option<String>,
    pub video: Option<VideoLink>,
    /// Other people invited, as the minutes will name them.
    pub participants: Vec<String>,
    pub decision: UpcomingDecision,
    /// Being recorded right now (started from the calendar).
    pub recording: bool,
}

/// Everything the "Upcoming meetings" section shows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct CalendarOverview {
    pub access: CalendarAccess,
    pub notifications: NotificationAccess,
    pub mode: CalendarMode,
    pub upcoming: Vec<UpcomingMeeting>,
}

/// The upcoming list or the access changed: refetch the overview.
#[derive(Debug, Clone, Serialize, Deserialize, Type, tauri_specta::Event)]
pub struct CalendarChangedEvent;

/// A prompt that couldn't be shown as a notification, shown in the window
/// instead.
#[derive(Debug, Clone, Serialize, Deserialize, Type, tauri_specta::Event)]
pub struct CalendarPromptEvent {
    pub kind: CalendarPromptKind,
    pub key: String,
    pub title: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum CalendarPromptKind {
    /// "X starts in 1 min — Record / Ignore".
    Soon,
    /// "X starts in 1 min, recording automatically — Don't record".
    AutoSoon,
    /// "X seems to be over — Stop / Keep recording".
    Ended,
}

/// What the user answered to a prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum CalendarAnswer {
    Record,
    Ignore,
    Stop,
    Keep,
}

impl CalendarAnswer {
    fn parse(action: &str) -> Option<Self> {
        match action {
            "record" => Some(Self::Record),
            "ignore" => Some(Self::Ignore),
            "stop" => Some(Self::Stop),
            "keep" => Some(Self::Keep),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_off_and_strict() {
        let s = CalendarSettings::default();
        assert_eq!(s.mode, CalendarMode::Off);
        assert!(s.only_video_calls && s.skip_without_attendees);
        let partial: CalendarSettings = serde_json::from_str(r#"{"mode":"auto"}"#).unwrap();
        assert_eq!(partial.mode, CalendarMode::Auto);
        assert!(partial.only_video_calls);
    }

    #[test]
    fn choices_replace_each_other_and_old_ones_are_forgotten() {
        let now = 10 * DAY_MS;
        let mut s = CalendarSettings::default();
        let key = format!("A@{}", now + 60_000);
        s.set_choice(&key, Some(false), now);
        assert_eq!(s.choice(&key), Some(false));
        s.set_choice(&key, Some(true), now);
        assert_eq!(s.choice(&key), Some(true));
        assert!(s.skipped_events.is_empty());
        s.set_choice(&key, None, now);
        assert_eq!(s.choice(&key), None);

        s.skipped_events.push(format!("old@{}", now - 3 * DAY_MS));
        s.skipped_events.push("odd-key".into());
        s.set_choice(&key, Some(false), now);
        assert_eq!(s.skipped_events, vec!["odd-key".to_string(), key.clone()]);
    }

    #[test]
    fn occurrence_start_reads_the_key() {
        assert_eq!(
            occurrence_start("ABC:123@1700000000000"),
            Some(1_700_000_000_000)
        );
        assert_eq!(occurrence_start("a@b@42"), Some(42));
        assert_eq!(occurrence_start("nothing"), None);
    }

    #[test]
    fn answers_parse() {
        assert_eq!(
            CalendarAnswer::parse("record"),
            Some(CalendarAnswer::Record)
        );
        assert_eq!(CalendarAnswer::parse("open"), None);
    }
}
