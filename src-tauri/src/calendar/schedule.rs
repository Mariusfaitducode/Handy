//! When to prompt, start, and propose stopping. Pure: the scheduler feeds it
//! the time, the calendar and the meeting state, and carries out the actions
//! it returns. Every rule is unit-tested below.

use std::collections::HashMap;

use super::detect::{self, RawEvent, SkipReason, VideoLink};
use super::{occurrence_start, CalendarMode, CalendarSettings, UpcomingDecision, UpcomingMeeting};

/// The prompt shows up this long before the start.
pub const LEAD_MS: i64 = 60_000;
/// Someone joining late is still offered to record during the first minutes.
pub const LATE_MS: i64 = 10 * 60_000;
/// Past the scheduled end, this much silence means the call is probably over.
pub const SILENCE_MS: i64 = 3 * 60_000;
/// "Keep recording" holds the "seems over" prompt back this long.
pub const SNOOZE_MS: i64 = 10 * 60_000;
/// Past the scheduled end and with nothing said for this long, nobody is
/// there anymore: recording stops on its own (nothing is lost, it is silence).
pub const GIVE_UP_SILENCE_MS: i64 = 20 * 60_000;

/// An event that could become a meeting.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub key: String,
    pub title: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub calendar_title: String,
    pub calendar_color: Option<String>,
    pub video: Option<VideoLink>,
    pub participants: Vec<String>,
    pub decision: UpcomingDecision,
}

/// The events still to come (or in progress) that are calls, earliest first.
/// Events without a call link are left out when only calls are followed;
/// the same call seen in two calendars appears once.
pub fn candidates(events: &[RawEvent], settings: &CalendarSettings, now_ms: i64) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::new();
    for event in events {
        if detect::is_hidden(event, settings) || event.end_ms <= now_ms {
            continue;
        }
        let video = detect::event_video_link(event);
        let key = event.key();
        let choice = settings.choice(&key);
        let decision = match (
            choice,
            detect::default_decision(event, video.as_ref(), settings),
        ) {
            (Some(false), _) => UpcomingDecision::Skipped,
            (Some(true), _) => UpcomingDecision::Record,
            (None, Ok(())) => UpcomingDecision::Record,
            (None, Err(SkipReason::NoAttendees)) => UpcomingDecision::NoAttendees,
            (None, Err(SkipReason::NoVideoLink)) => continue,
        };
        let title = event.title.trim();
        let duplicate = out.iter().any(|c| {
            c.start_ms == event.start_ms
                && c.title == title
                && c.video.as_ref().map(|v| &v.url) == video.as_ref().map(|v| &v.url)
        });
        if duplicate {
            continue;
        }
        out.push(Candidate {
            key,
            title: title.to_string(),
            start_ms: event.start_ms,
            end_ms: event.end_ms,
            calendar_title: event.calendar_title.clone(),
            calendar_color: event.calendar_color.clone(),
            video,
            participants: event.participant_names(),
            decision,
        });
    }
    out.sort_by(|a, b| {
        a.start_ms
            .cmp(&b.start_ms)
            .then_with(|| a.title.cmp(&b.title))
    });
    out
}

/// What the meeting module reports, reduced to what planning needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeetingView {
    /// A meeting is recording, paused, or finishing.
    pub busy: bool,
    /// Recording right now (not paused).
    pub recording: bool,
    pub meeting_id: Option<i64>,
}

#[cfg(test)]
impl MeetingView {
    pub const IDLE: MeetingView = MeetingView {
        busy: false,
        recording: false,
        meeting_id: None,
    };
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Ask mode: "X starts in 1 min — Record / Ignore".
    PromptSoon(Candidate),
    /// Auto mode: "X starts in 1 min, recording automatically".
    AnnounceAuto(Candidate),
    /// Start recording X now.
    Start(Candidate),
    /// Take back the start prompt of this occurrence: it is no longer
    /// relevant.
    Withdraw(String),
    /// "X seems to be over — Stop / Keep recording".
    PromptEnded { key: String, title: String },
    /// Take back the "seems over" prompt: people are talking again.
    WithdrawEnded(String),
    /// Stop recording: long silence after the end.
    Stop { key: String, title: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Handled {
    Prompted,
    AutoAnnounced,
    Started,
    Dismissed,
    Expired,
}

/// The meeting the calendar started.
#[derive(Debug, Clone, PartialEq)]
struct Ours {
    key: String,
    title: String,
    end_ms: i64,
    meeting_id: i64,
    last_activity_ms: i64,
    ended_prompt_at: Option<i64>,
    snooze_until: i64,
    stopped: bool,
}

/// What has been done about each occurrence, kept between ticks.
#[derive(Debug, Default)]
pub struct Tracker {
    handled: HashMap<String, Handled>,
    ours: Option<Ours>,
}

impl Tracker {
    pub fn plan(
        &mut self,
        now: i64,
        candidates: &[Candidate],
        mode: CalendarMode,
        meeting: MeetingView,
    ) -> Vec<Action> {
        let mut actions = Vec::new();
        self.forget_old(now);
        self.plan_end(now, meeting, &mut actions);

        for c in candidates {
            let state = self.handled.get(&c.key).copied();
            let open = matches!(state, Some(Handled::Prompted | Handled::AutoAnnounced));
            if c.decision != UpcomingDecision::Record || mode == CalendarMode::Off {
                if open {
                    actions.push(Action::Withdraw(c.key.clone()));
                    self.handled.insert(c.key.clone(), Handled::Dismissed);
                }
                continue;
            }
            let window_end = (c.start_ms + LATE_MS).min(c.end_ms);
            if now >= window_end {
                if open {
                    actions.push(Action::Withdraw(c.key.clone()));
                    self.handled.insert(c.key.clone(), Handled::Expired);
                }
                continue;
            }
            if now < c.start_ms - LEAD_MS || meeting.busy {
                continue;
            }
            match (mode, state) {
                (CalendarMode::Ask, None) => {
                    actions.push(Action::PromptSoon(c.clone()));
                    self.handled.insert(c.key.clone(), Handled::Prompted);
                }
                (CalendarMode::Auto, None) if now < c.start_ms => {
                    actions.push(Action::AnnounceAuto(c.clone()));
                    self.handled.insert(c.key.clone(), Handled::AutoAnnounced);
                }
                (CalendarMode::Auto, None | Some(Handled::AutoAnnounced)) if now >= c.start_ms => {
                    actions.push(Action::Start(c.clone()));
                    // Only one meeting at a time: the next one waits.
                    self.handled.insert(c.key.clone(), Handled::Started);
                    break;
                }
                _ => {}
            }
        }
        actions
    }

    fn plan_end(&mut self, now: i64, meeting: MeetingView, actions: &mut Vec<Action>) {
        let Some(ours) = self.ours.as_mut() else {
            return;
        };
        if meeting.meeting_id != Some(ours.meeting_id) || !meeting.busy {
            if ours.ended_prompt_at.is_some() {
                actions.push(Action::WithdrawEnded(ours.key.clone()));
            }
            self.ours = None;
            return;
        }
        if !meeting.recording || ours.stopped || now < ours.end_ms {
            return;
        }
        let silence = now - ours.last_activity_ms.max(ours.end_ms - SILENCE_MS);
        if let Some(at) = ours.ended_prompt_at {
            if ours.last_activity_ms > at {
                // They are still talking: the prompt was wrong.
                actions.push(Action::WithdrawEnded(ours.key.clone()));
                ours.ended_prompt_at = None;
                return;
            }
        }
        if silence >= GIVE_UP_SILENCE_MS {
            if ours.ended_prompt_at.is_some() {
                actions.push(Action::WithdrawEnded(ours.key.clone()));
            }
            actions.push(Action::Stop {
                key: ours.key.clone(),
                title: ours.title.clone(),
            });
            ours.stopped = true;
        } else if silence >= SILENCE_MS
            && ours.ended_prompt_at.is_none()
            && now >= ours.snooze_until
        {
            actions.push(Action::PromptEnded {
                key: ours.key.clone(),
                title: ours.title.clone(),
            });
            ours.ended_prompt_at = Some(now);
        }
    }

    fn forget_old(&mut self, now: i64) {
        self.handled.retain(|key, _| {
            occurrence_start(key).is_none_or(|start| start > now - 2 * 86_400_000)
        });
    }

    /// The calendar's meeting started (from a prompt or automatically).
    pub fn started(&mut self, c: &Candidate, meeting_id: i64, now: i64) {
        self.handled.insert(c.key.clone(), Handled::Started);
        self.ours = Some(Ours {
            key: c.key.clone(),
            title: c.title.clone(),
            end_ms: c.end_ms,
            meeting_id,
            last_activity_ms: now,
            ended_prompt_at: None,
            snooze_until: 0,
            stopped: false,
        });
    }

    /// Starting failed: let a later tick (or the user) try again.
    pub fn start_failed(&mut self, key: &str) {
        if self.handled.get(key) == Some(&Handled::Started) {
            self.handled.insert(key.to_string(), Handled::Prompted);
        }
    }

    /// The user said not to record this occurrence.
    pub fn dismissed(&mut self, key: &str) {
        self.handled.insert(key.to_string(), Handled::Dismissed);
    }

    /// Forgets what was done about an occurrence (only if it was dismissed
    /// or never answered), so it can be offered again.
    pub fn forget(&mut self, key: &str) {
        if matches!(
            self.handled.get(key),
            Some(Handled::Dismissed | Handled::Expired)
        ) {
            self.handled.remove(key);
        }
    }

    /// The user said to keep recording although it seemed over.
    pub fn keep(&mut self, now: i64) {
        if let Some(ours) = self.ours.as_mut() {
            ours.ended_prompt_at = None;
            ours.snooze_until = now + SNOOZE_MS;
        }
    }

    /// Something was said in a meeting.
    pub fn activity(&mut self, meeting_id: i64, at_ms: i64) {
        if let Some(ours) = self.ours.as_mut() {
            if ours.meeting_id == meeting_id && at_ms > ours.last_activity_ms {
                ours.last_activity_ms = at_ms;
            }
        }
    }

    /// The occurrence the calendar is recording, if any.
    pub fn recording_key(&self) -> Option<&str> {
        self.ours.as_ref().map(|o| o.key.as_str())
    }
}

/// The "Upcoming meetings" rows: what starts before `until_ms`, at most
/// `limit` of them.
pub fn upcoming(
    candidates: &[Candidate],
    until_ms: i64,
    limit: usize,
    recording_key: Option<&str>,
) -> Vec<UpcomingMeeting> {
    candidates
        .iter()
        .filter(|c| c.start_ms < until_ms)
        .take(limit)
        .map(|c| UpcomingMeeting {
            key: c.key.clone(),
            title: c.title.clone(),
            start_ms: c.start_ms,
            end_ms: c.end_ms,
            calendar_title: c.calendar_title.clone(),
            calendar_color: c.calendar_color.clone(),
            video: c.video.clone(),
            participants: c.participants.clone(),
            decision: c.decision,
            recording: recording_key == Some(c.key.as_str()),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calendar::detect::RawParticipant;

    const MIN: i64 = 60_000;
    /// 10:00 on some day.
    const T0: i64 = 1_760_000_000_000;

    fn attendee(name: &str, me: bool) -> RawParticipant {
        RawParticipant {
            name: Some(name.into()),
            email: None,
            status: 2,
            kind: 1,
            is_current_user: me,
        }
    }

    fn raw(id: &str, start: i64, minutes: i64, notes: Option<&str>) -> RawEvent {
        RawEvent {
            series_id: id.into(),
            title: format!("Réunion {id}"),
            start_ms: start,
            end_ms: start + minutes * MIN,
            notes: notes.map(Into::into),
            calendar_id: "work".into(),
            calendar_title: "Travail".into(),
            attendees: vec![attendee("Marius", true), attendee("Yasmine", false)],
            ..Default::default()
        }
    }

    fn call(id: &str, start: i64, minutes: i64) -> RawEvent {
        raw(
            id,
            start,
            minutes,
            Some("https://meet.google.com/abc-defg-hij"),
        )
    }

    fn cands(events: &[RawEvent], now: i64) -> Vec<Candidate> {
        candidates(events, &CalendarSettings::default(), now)
    }

    fn recording(id: i64) -> MeetingView {
        MeetingView {
            busy: true,
            recording: true,
            meeting_id: Some(id),
        }
    }

    #[test]
    fn candidates_keep_calls_in_order_without_duplicates() {
        let mut copy = call("B", T0, 30);
        copy.series_id = "B-in-personal-calendar".into();
        copy.title = "Réunion B".into();
        let events = vec![
            call("B", T0, 30),
            call("A", T0 - 60 * MIN, 30), // already over
            raw("Lunch", T0 + 60 * MIN, 60, None),
            copy,
            call("C", T0 + 120 * MIN, 30),
        ];
        let list = cands(&events, T0 - 10 * MIN);
        let titles: Vec<_> = list.iter().map(|c| c.title.as_str()).collect();
        assert_eq!(titles, vec!["Réunion B", "Réunion C"]);
        assert_eq!(list[0].participants, vec!["Yasmine"]);
        assert_eq!(list[0].video.as_ref().unwrap().provider, "meet");
    }

    #[test]
    fn candidates_follow_the_users_choices() {
        let mut alone = call("Solo", T0, 30);
        alone.attendees = vec![attendee("Marius", true)];
        let events = vec![call("A", T0, 30), alone.clone(), raw("Lunch", T0, 60, None)];

        let list = cands(&events, T0 - MIN);
        assert_eq!(list[0].decision, UpcomingDecision::Record);
        assert_eq!(list[1].decision, UpcomingDecision::NoAttendees);
        assert_eq!(list.len(), 2, "no link, not a call");

        let mut settings = CalendarSettings::default();
        settings.set_choice(&events[0].key(), Some(false), T0);
        settings.set_choice(&alone.key(), Some(true), T0);
        let list = candidates(&events, &settings, T0 - MIN);
        assert_eq!(list[0].decision, UpcomingDecision::Skipped);
        assert_eq!(list[1].decision, UpcomingDecision::Record);

        let everything = CalendarSettings {
            only_video_calls: false,
            ..Default::default()
        };
        assert_eq!(candidates(&events, &everything, T0 - MIN).len(), 3);
    }

    #[test]
    fn ask_mode_prompts_once_a_minute_before() {
        let list = cands(&[call("A", T0, 30)], T0 - 5 * MIN);
        let mut t = Tracker::default();
        let ask = CalendarMode::Ask;
        assert!(t
            .plan(T0 - 2 * MIN, &list, ask, MeetingView::IDLE)
            .is_empty());
        let actions = t.plan(T0 - 50_000, &list, ask, MeetingView::IDLE);
        assert!(matches!(&actions[..], [Action::PromptSoon(c)] if c.title == "Réunion A"));
        assert!(t
            .plan(T0 - 30_000, &list, ask, MeetingView::IDLE)
            .is_empty());
        assert!(t.plan(T0 + MIN, &list, ask, MeetingView::IDLE).is_empty());
        // Unanswered: taken back once it's too late to matter.
        assert_eq!(
            t.plan(T0 + LATE_MS, &list, ask, MeetingView::IDLE),
            vec![Action::Withdraw(list[0].key.clone())]
        );
        assert!(t
            .plan(T0 + LATE_MS + MIN, &list, ask, MeetingView::IDLE)
            .is_empty());
    }

    #[test]
    fn a_late_launch_still_prompts_during_the_first_minutes() {
        let list = cands(&[call("A", T0, 30)], T0);
        let mut t = Tracker::default();
        let actions = t.plan(T0 + 4 * MIN, &list, CalendarMode::Ask, MeetingView::IDLE);
        assert!(matches!(&actions[..], [Action::PromptSoon(_)]));

        let mut t = Tracker::default();
        let short = cands(&[call("B", T0, 5)], T0);
        assert!(t
            .plan(T0 + 6 * MIN, &short, CalendarMode::Ask, MeetingView::IDLE)
            .is_empty());
    }

    #[test]
    fn nothing_happens_while_a_meeting_is_already_going() {
        let list = cands(&[call("A", T0, 30)], T0 - 5 * MIN);
        for mode in [CalendarMode::Ask, CalendarMode::Auto] {
            let mut t = Tracker::default();
            assert!(t.plan(T0 - 50_000, &list, mode, recording(7)).is_empty());
            assert!(t.plan(T0 + MIN, &list, mode, recording(7)).is_empty());
            // The manual meeting ends early in the slot: now it's offered.
            assert!(!t
                .plan(T0 + 2 * MIN, &list, mode, MeetingView::IDLE)
                .is_empty());
        }
    }

    #[test]
    fn auto_mode_announces_then_starts_on_time() {
        let list = cands(&[call("A", T0, 30)], T0 - 5 * MIN);
        let mut t = Tracker::default();
        let auto = CalendarMode::Auto;
        let actions = t.plan(T0 - 50_000, &list, auto, MeetingView::IDLE);
        assert!(matches!(&actions[..], [Action::AnnounceAuto(_)]));
        assert!(t
            .plan(T0 - 10_000, &list, auto, MeetingView::IDLE)
            .is_empty());
        let actions = t.plan(T0 + 5_000, &list, auto, MeetingView::IDLE);
        assert!(matches!(&actions[..], [Action::Start(c)] if c.key == list[0].key));
        t.started(&list[0], 1, T0 + 5_000);
        assert!(t.plan(T0 + 20_000, &list, auto, recording(1)).is_empty());
        assert_eq!(t.recording_key(), Some(list[0].key.as_str()));
    }

    #[test]
    fn auto_mode_respects_dont_record_and_retries_failed_starts() {
        let list = cands(&[call("A", T0, 30)], T0 - 5 * MIN);
        let auto = CalendarMode::Auto;
        let mut t = Tracker::default();
        t.plan(T0 - 50_000, &list, auto, MeetingView::IDLE);
        t.dismissed(&list[0].key);
        assert!(t
            .plan(T0 + 5_000, &list, auto, MeetingView::IDLE)
            .is_empty());

        let mut t = Tracker::default();
        t.plan(T0 + 5_000, &list, auto, MeetingView::IDLE);
        t.start_failed(&list[0].key);
        // A failed start becomes a prompt the user can act on; no loop.
        assert!(t
            .plan(T0 + 20_000, &list, auto, MeetingView::IDLE)
            .is_empty());
    }

    #[test]
    fn undoing_ignore_offers_it_again() {
        let list = cands(&[call("A", T0, 30)], T0 - 5 * MIN);
        let mut t = Tracker::default();
        let ask = CalendarMode::Ask;
        t.plan(T0 - 50_000, &list, ask, MeetingView::IDLE);
        t.dismissed(&list[0].key);
        assert!(t
            .plan(T0 - 40_000, &list, ask, MeetingView::IDLE)
            .is_empty());
        t.forget(&list[0].key);
        assert_eq!(t.plan(T0 - 30_000, &list, ask, MeetingView::IDLE).len(), 1);
        // A started meeting is never forgotten (it would start twice).
        t.started(&list[0], 1, T0);
        t.forget(&list[0].key);
        assert!(t.plan(T0 + MIN, &list, ask, MeetingView::IDLE).is_empty());
    }

    #[test]
    fn two_back_to_back_calls_start_one_at_a_time() {
        let list = cands(&[call("A", T0, 30), call("B", T0, 30)], T0 - MIN);
        let mut t = Tracker::default();
        let actions = t.plan(T0 + 1_000, &list, CalendarMode::Auto, MeetingView::IDLE);
        assert_eq!(actions.len(), 1);
    }

    #[test]
    fn skipping_after_the_prompt_takes_it_back() {
        let events = [call("A", T0, 30)];
        let list = cands(&events, T0 - 5 * MIN);
        let mut t = Tracker::default();
        t.plan(T0 - 50_000, &list, CalendarMode::Ask, MeetingView::IDLE);
        let mut settings = CalendarSettings::default();
        settings.set_choice(&events[0].key(), Some(false), T0);
        let skipped = candidates(&events, &settings, T0 - 5 * MIN);
        assert_eq!(
            t.plan(T0 - 40_000, &skipped, CalendarMode::Ask, MeetingView::IDLE),
            vec![Action::Withdraw(list[0].key.clone())]
        );
    }

    #[test]
    fn end_needs_the_scheduled_time_and_silence() {
        let list = cands(&[call("A", T0, 30)], T0);
        let end = T0 + 30 * MIN;
        let mut t = Tracker::default();
        t.started(&list[0], 3, T0);
        let ask = CalendarMode::Ask;
        // Long silence before the end: nothing.
        assert!(t.plan(end - MIN, &list, ask, recording(3)).is_empty());
        // Still talking at the end: nothing.
        t.activity(3, end - 30_000);
        assert!(t.plan(end + MIN, &list, ask, recording(3)).is_empty());
        // Quiet for 3 minutes past the end: ask.
        let actions = t.plan(end - 30_000 + SILENCE_MS, &list, ask, recording(3));
        assert!(
            matches!(&actions[..], [Action::PromptEnded { title, .. }] if title == "Réunion A")
        );
        // Asked once.
        assert!(t.plan(end + 5 * MIN, &list, ask, recording(3)).is_empty());
    }

    #[test]
    fn talking_again_takes_the_end_prompt_back_and_keep_snoozes_it() {
        let list = cands(&[call("A", T0, 30)], T0);
        let end = T0 + 30 * MIN;
        let mut t = Tracker::default();
        t.started(&list[0], 3, end - 10 * MIN);
        let ask = CalendarMode::Ask;
        assert_eq!(t.plan(end, &list, ask, recording(3)).len(), 1);
        t.activity(3, end + 30_000);
        assert_eq!(
            t.plan(end + MIN, &list, ask, recording(3)),
            vec![Action::WithdrawEnded(list[0].key.clone())]
        );
        let again = end + 30_000 + SILENCE_MS;
        assert_eq!(t.plan(again, &list, ask, recording(3)).len(), 1);
        t.keep(again);
        assert!(t
            .plan(again + SNOOZE_MS - 1, &list, ask, recording(3))
            .is_empty());
        assert_eq!(t.plan(again + SNOOZE_MS, &list, ask, recording(3)).len(), 1);
    }

    #[test]
    fn long_silence_after_the_end_stops_once() {
        let list = cands(&[call("A", T0, 30)], T0);
        let end = T0 + 30 * MIN;
        let mut t = Tracker::default();
        t.started(&list[0], 3, T0);
        t.activity(3, end);
        let ask = CalendarMode::Ask;
        t.plan(end + SILENCE_MS, &list, ask, recording(3));
        let actions = t.plan(end + GIVE_UP_SILENCE_MS, &list, ask, recording(3));
        assert_eq!(
            actions,
            vec![
                Action::WithdrawEnded(list[0].key.clone()),
                Action::Stop {
                    key: list[0].key.clone(),
                    title: "Réunion A".into()
                }
            ]
        );
        assert!(t
            .plan(end + GIVE_UP_SILENCE_MS + MIN, &list, ask, recording(3))
            .is_empty());
    }

    #[test]
    fn paused_or_other_meetings_are_left_alone() {
        let list = cands(&[call("A", T0, 30)], T0);
        let end = T0 + 30 * MIN;
        let mut t = Tracker::default();
        t.started(&list[0], 3, T0);
        let paused = MeetingView {
            busy: true,
            recording: false,
            meeting_id: Some(3),
        };
        assert!(t
            .plan(end + GIVE_UP_SILENCE_MS, &list, CalendarMode::Ask, paused)
            .is_empty());
        // The user stopped it and started another one: not ours anymore.
        assert!(t
            .plan(
                end + GIVE_UP_SILENCE_MS,
                &list,
                CalendarMode::Ask,
                recording(4)
            )
            .is_empty());
        assert_eq!(t.recording_key(), None);
    }

    #[test]
    fn turning_the_feature_off_takes_prompts_back() {
        let list = cands(&[call("A", T0, 30)], T0 - 5 * MIN);
        let mut t = Tracker::default();
        t.plan(T0 - 50_000, &list, CalendarMode::Ask, MeetingView::IDLE);
        assert_eq!(
            t.plan(T0 - 40_000, &list, CalendarMode::Off, MeetingView::IDLE),
            vec![Action::Withdraw(list[0].key.clone())]
        );
    }

    #[test]
    fn upcoming_rows_are_limited_and_flag_the_recording() {
        let events: Vec<_> = (0..5)
            .map(|i| call(&format!("E{i}"), T0 + i * 60 * MIN, 30))
            .collect();
        let list = cands(&events, T0);
        let rows = upcoming(&list, T0 + 3 * 60 * MIN, 10, Some(&list[0].key));
        assert_eq!(rows.len(), 3);
        assert!(rows[0].recording && !rows[1].recording);
        assert_eq!(upcoming(&list, i64::MAX, 2, None).len(), 2);
    }
}
