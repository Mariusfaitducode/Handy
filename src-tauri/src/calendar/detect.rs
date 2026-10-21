//! Pure logic over calendar events: which ones are calls worth recording,
//! which video service they use, and who was invited. No I/O, so every rule
//! is unit-tested below.

use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use specta::Type;

use super::CalendarSettings;

/* ───────────────────────────── raw events ───────────────────────────── */

/// An event as the EventKit bridge reports it.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct RawEvent {
    /// Shared by every occurrence of a recurring event.
    pub series_id: String,
    pub title: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub all_day: bool,
    pub location: Option<String>,
    pub notes: Option<String>,
    pub url: Option<String>,
    pub calendar_id: String,
    pub calendar_title: String,
    pub calendar_color: Option<String>,
    /// EKEventStatus: 0 none, 1 confirmed, 2 tentative, 3 canceled.
    pub status: i64,
    pub recurring: bool,
    pub organizer: Option<RawParticipant>,
    pub attendees: Vec<RawParticipant>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct RawParticipant {
    pub name: Option<String>,
    pub email: Option<String>,
    /// EKParticipantStatus: 1 pending, 2 accepted, 3 declined, 4 tentative…
    pub status: i64,
    /// EKParticipantType: 1 person, 2 room, 3 resource, 4 group.
    pub kind: i64,
    pub is_current_user: bool,
}

const EVENT_CANCELED: i64 = 3;
const PARTICIPANT_DECLINED: i64 = 3;
const KIND_ROOM: i64 = 2;
const KIND_RESOURCE: i64 = 3;

impl RawParticipant {
    fn is_place(&self) -> bool {
        matches!(self.kind, KIND_ROOM | KIND_RESOURCE)
    }

    /// "Alice Martin", or "alice.martin" from the address when the
    /// invitation has no name.
    fn display_name(&self) -> Option<String> {
        if let Some(name) = self
            .name
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty())
        {
            // Exchange sometimes puts the address in the name field.
            if !name.contains('@') {
                return Some(name.to_string());
            }
        }
        let email = self
            .email
            .as_deref()
            .or(self.name.as_deref())
            .map(str::trim)?;
        let local = email.split('@').next()?.trim();
        (!local.is_empty()).then(|| local.to_string())
    }
}

impl RawEvent {
    /// Identifies this occurrence: the series plus its start.
    pub fn key(&self) -> String {
        format!("{}@{}", self.series_id, self.start_ms)
    }

    /// The current user said no to this one.
    pub fn declined(&self) -> bool {
        self.attendees
            .iter()
            .any(|a| a.is_current_user && a.status == PARTICIPANT_DECLINED)
    }

    /// Other people invited (rooms, resources and the user excluded).
    pub fn other_people(&self) -> Vec<&RawParticipant> {
        self.attendees
            .iter()
            .filter(|a| !a.is_current_user && !a.is_place())
            .collect()
    }

    /// Names to put in the minutes: everyone invited but the user (who is
    /// "Me"), organizer first, without duplicates.
    pub fn participant_names(&self) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        let organizer = self
            .organizer
            .as_ref()
            .filter(|o| !o.is_current_user && !o.is_place());
        for person in organizer.into_iter().chain(self.other_people()) {
            if let Some(name) = person.display_name() {
                if !names.iter().any(|n| n.eq_ignore_ascii_case(&name)) {
                    names.push(name);
                }
            }
        }
        names
    }
}

/* ───────────────────────────── video links ───────────────────────────── */

/// A call link found in an event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct VideoLink {
    /// "meet", "zoom", "teams", "webex", "facetime", "whereby", "jitsi",
    /// "around", "slack", "other". The UI names it.
    pub provider: String,
    pub url: String,
}

struct Provider {
    id: &'static str,
    pattern: Regex,
}

static PROVIDERS: Lazy<Vec<Provider>> = Lazy::new(|| {
    let p = |id, pattern: &str| Provider {
        id,
        pattern: Regex::new(pattern).expect("valid provider pattern"),
    };
    vec![
        p(
            "meet",
            r"(?i)\bhttps?://meet\.google\.com/[a-z0-9\-_/?=&%.]+",
        ),
        p(
            "zoom",
            r"(?i)\bhttps?://(?:[a-z0-9\-]+\.)*zoom(?:gov)?\.(?:us|com)/(?:j|my|w|s|wc)/[^\s<>()\x22']+",
        ),
        p(
            "teams",
            r"(?i)\bhttps?://teams\.(?:microsoft|live)\.com/(?:l/meetup-join|meet)/[^\s<>()\x22']+",
        ),
        p(
            "webex",
            r"(?i)\bhttps?://(?:[a-z0-9\-]+\.)*webex\.com/[^\s<>()\x22']*(?:j\.php|meet|join)[^\s<>()\x22']*",
        ),
        p(
            "facetime",
            r"(?i)\bhttps?://facetime\.apple\.com/join[^\s<>()\x22']*",
        ),
        p("whereby", r"(?i)\bhttps?://whereby\.com/[^\s<>()\x22']+"),
        p("jitsi", r"(?i)\bhttps?://meet\.jit\.si/[^\s<>()\x22']+"),
        p(
            "around",
            r"(?i)\bhttps?://(?:meet\.)?around\.co/[^\s<>()\x22']+",
        ),
        p(
            "slack",
            r"(?i)\bhttps?://app\.slack\.com/huddle/[^\s<>()\x22']+",
        ),
    ]
});

/// Outlook "Safe Links" wrap every URL in an invitation:
/// `https://eur01.safelinks.protection.outlook.com/?url=<encoded>&data=…`.
static SAFELINK: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)https?://[a-z0-9.\-]*safelinks\.protection\.outlook\.com/\S*?[?&]url=([^&\s<>\x22']+)[^\s<>\x22']*")
        .expect("valid safelink pattern")
});

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && bytes[i + 1].is_ascii_hexdigit()
            && bytes[i + 2].is_ascii_hexdigit()
        {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
            if let Ok(v) = u8::from_str_radix(hex, 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Replaces each Safe Link (tracking parameters included) by the URL it
/// wraps, so the real call link can be recognized.
fn unwrap_safelinks(text: &str) -> String {
    SAFELINK
        .replace_all(text, |caps: &regex::Captures<'_>| percent_decode(&caps[1]))
        .into_owned()
}

/// Trailing punctuation that is part of the sentence, not of the URL.
fn trim_url(url: &str) -> &str {
    url.trim_end_matches(['.', ',', ';', ':', '!', '?', ']', '>', '*'])
}

/// The first call link in `text`, if any.
pub fn find_video_link(text: &str) -> Option<VideoLink> {
    if text.is_empty() {
        return None;
    }
    let text = unwrap_safelinks(text);
    // The earliest match wins, so "Zoom (backup: Meet)" picks Zoom.
    PROVIDERS
        .iter()
        .filter_map(|p| p.pattern.find(&text).map(|m| (m.start(), p.id, m.as_str())))
        .min_by_key(|(start, _, _)| *start)
        .map(|(_, id, url)| VideoLink {
            provider: id.to_string(),
            url: trim_url(url).to_string(),
        })
}

/// The event's call link, looked for in its URL, then location, then notes
/// (where Google, Teams and Zoom put the "join" block).
pub fn event_video_link(event: &RawEvent) -> Option<VideoLink> {
    [&event.url, &event.location, &event.notes]
        .into_iter()
        .flatten()
        .find_map(|text| find_video_link(text))
}

/* ───────────────────────────── eligibility ───────────────────────────── */

/// Why an event isn't recorded by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum SkipReason {
    NoVideoLink,
    NoAttendees,
}

/// Events never shown nor recorded: all-day, canceled, declined, from a
/// calendar the user doesn't follow, or not a real time slot.
pub fn is_hidden(event: &RawEvent, settings: &CalendarSettings) -> bool {
    event.all_day
        || event.status == EVENT_CANCELED
        || event.declined()
        || event.end_ms <= event.start_ms
        || settings
            .excluded_calendars
            .iter()
            .any(|id| id == &event.calendar_id)
}

/// Whether the event would be recorded without the user saying anything,
/// and if not, why.
pub fn default_decision(
    event: &RawEvent,
    video: Option<&VideoLink>,
    settings: &CalendarSettings,
) -> Result<(), SkipReason> {
    if settings.only_video_calls && video.is_none() {
        return Err(SkipReason::NoVideoLink);
    }
    if settings.skip_without_attendees && event.other_people().is_empty() {
        return Err(SkipReason::NoAttendees);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn person(name: &str, email: &str) -> RawParticipant {
        RawParticipant {
            name: Some(name.into()),
            email: Some(email.into()),
            status: 2,
            kind: 1,
            is_current_user: false,
        }
    }

    fn me(status: i64) -> RawParticipant {
        RawParticipant {
            name: Some("Marius".into()),
            email: Some("marius@example.com".into()),
            status,
            kind: 1,
            is_current_user: true,
        }
    }

    fn event() -> RawEvent {
        RawEvent {
            series_id: "ABC".into(),
            title: "Point hebdo".into(),
            start_ms: 1_000_000,
            end_ms: 1_000_000 + 30 * 60_000,
            calendar_id: "work".into(),
            attendees: vec![me(2), person("Alice Martin", "alice@example.com")],
            ..Default::default()
        }
    }

    #[test]
    fn finds_links_of_every_common_service() {
        let cases = [
            (
                "Rejoindre avec Google Meet : https://meet.google.com/abc-defg-hij",
                "meet",
                "https://meet.google.com/abc-defg-hij",
            ),
            (
                "https://us02web.zoom.us/j/81234567890?pwd=AbCd.1",
                "zoom",
                "https://us02web.zoom.us/j/81234567890?pwd=AbCd.1",
            ),
            (
                "Microsoft Teams meeting\nJoin: <https://teams.microsoft.com/l/meetup-join/19%3ameeting_X%40thread.v2/0?context=%7b%7d>",
                "teams",
                "https://teams.microsoft.com/l/meetup-join/19%3ameeting_X%40thread.v2/0?context=%7b%7d",
            ),
            (
                "https://teams.live.com/meet/9876543210",
                "teams",
                "https://teams.live.com/meet/9876543210",
            ),
            (
                "https://acme.webex.com/acme/j.php?MTID=m123",
                "webex",
                "https://acme.webex.com/acme/j.php?MTID=m123",
            ),
            (
                "FaceTime: https://facetime.apple.com/join#v=1&p=xyz.",
                "facetime",
                "https://facetime.apple.com/join#v=1&p=xyz",
            ),
            ("https://whereby.com/equipe", "whereby", "https://whereby.com/equipe"),
            ("https://meet.jit.si/MaReunion", "jitsi", "https://meet.jit.si/MaReunion"),
        ];
        for (text, provider, url) in cases {
            let link = find_video_link(text).unwrap_or_else(|| panic!("no link in {text}"));
            assert_eq!(link.provider, provider, "{text}");
            assert_eq!(link.url, url, "{text}");
        }
    }

    #[test]
    fn ignores_text_without_a_call_link() {
        for text in [
            "",
            "Salle 4B, 2e étage",
            "https://docs.google.com/document/d/123",
            "https://zoom.us/pricing",
            "meet.google.com sans lien",
            "https://www.microsoft.com/teams",
        ] {
            assert_eq!(find_video_link(text), None, "{text}");
        }
    }

    #[test]
    fn unwraps_outlook_safe_links() {
        let text = "Rejoindre : https://eur01.safelinks.protection.outlook.com/?url=https%3A%2F%2Fus06web.zoom.us%2Fj%2F123456%3Fpwd%3Dx&data=05%7C01";
        let link = find_video_link(text).unwrap();
        assert_eq!(link.provider, "zoom");
        assert_eq!(link.url, "https://us06web.zoom.us/j/123456?pwd=x");
    }

    #[test]
    fn the_first_link_wins_and_fields_are_searched_in_order() {
        let link =
            find_video_link("Zoom https://zoom.us/j/1 (secours : https://meet.google.com/a-b-c)")
                .unwrap();
        assert_eq!(link.provider, "zoom");

        let mut e = event();
        e.notes = Some("https://meet.google.com/aaa-bbbb-ccc".into());
        e.location = Some("https://zoom.us/j/42".into());
        assert_eq!(event_video_link(&e).unwrap().provider, "zoom");
        e.location = Some("Salle 4B".into());
        assert_eq!(event_video_link(&e).unwrap().provider, "meet");
    }

    #[test]
    fn hidden_events() {
        let settings = CalendarSettings::default();
        assert!(!is_hidden(&event(), &settings));

        let mut all_day = event();
        all_day.all_day = true;
        assert!(is_hidden(&all_day, &settings));

        let mut canceled = event();
        canceled.status = EVENT_CANCELED;
        assert!(is_hidden(&canceled, &settings));

        let mut declined = event();
        declined.attendees[0] = me(PARTICIPANT_DECLINED);
        assert!(is_hidden(&declined, &settings));

        let excluded = CalendarSettings {
            excluded_calendars: vec!["work".into()],
            ..Default::default()
        };
        assert!(is_hidden(&event(), &excluded));
    }

    #[test]
    fn default_decision_needs_a_link_and_people() {
        let settings = CalendarSettings::default();
        let link = find_video_link("https://meet.google.com/abc-defg-hij");
        let e = event();
        assert_eq!(default_decision(&e, link.as_ref(), &settings), Ok(()));
        assert_eq!(
            default_decision(&e, None, &settings),
            Err(SkipReason::NoVideoLink)
        );

        let mut alone = event();
        alone.attendees = vec![
            me(2),
            RawParticipant {
                name: Some("Salle Everest".into()),
                kind: KIND_ROOM,
                ..Default::default()
            },
        ];
        assert_eq!(
            default_decision(&alone, link.as_ref(), &settings),
            Err(SkipReason::NoAttendees)
        );

        let relaxed = CalendarSettings {
            only_video_calls: false,
            skip_without_attendees: false,
            ..Default::default()
        };
        assert_eq!(default_decision(&alone, None, &relaxed), Ok(()));
    }

    #[test]
    fn participant_names_skip_me_rooms_and_duplicates() {
        let mut e = event();
        e.organizer = Some(person("Bob Durand", "bob@example.com"));
        e.attendees.push(person("bob durand", "bob@example.com"));
        e.attendees.push(RawParticipant {
            name: None,
            email: Some("claire.dupont@example.com".into()),
            kind: 1,
            ..Default::default()
        });
        e.attendees.push(RawParticipant {
            name: Some("dan@example.com".into()),
            kind: 1,
            ..Default::default()
        });
        e.attendees.push(RawParticipant {
            name: Some("Salle Everest".into()),
            kind: KIND_ROOM,
            ..Default::default()
        });
        assert_eq!(
            e.participant_names(),
            vec!["Bob Durand", "Alice Martin", "claire.dupont", "dan"]
        );
    }

    #[test]
    fn keys_are_per_occurrence() {
        let a = event();
        let mut b = event();
        b.start_ms += 7 * 24 * 3_600_000;
        assert_ne!(a.key(), b.key());
        assert_eq!(a.series_id, b.series_id);
    }

    #[test]
    fn bridge_json_parses_with_missing_fields() {
        let json = r#"[{"series_id":"X","title":"Démo","start_ms":1,"end_ms":2,
            "all_day":false,"calendar_id":"c","calendar_title":"Travail","status":1,
            "recurring":false,"attendees":[{"name":"A","status":2,"kind":1,"is_current_user":false}]}]"#;
        let events: Vec<RawEvent> = serde_json::from_str(json).unwrap();
        assert_eq!(events[0].title, "Démo");
        assert_eq!(events[0].attendees.len(), 1);
        assert_eq!(events[0].notes, None);
    }

    #[test]
    fn percent_decoding_handles_edges() {
        assert_eq!(percent_decode("a%20b"), "a b");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz"), "%zz");
        assert_eq!(percent_decode("%C3%A9"), "é");
    }
}
