//! Pure transcript logic for meetings: which segments to keep, how the
//! transcript reads as text and Markdown, and how it is split for a model.

use serde::{Deserialize, Serialize};
use specta::Type;

/// Who spoke. The microphone track is the user; the system track is everyone
/// else on the call. Two tracks give this attribution without diarization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum Speaker {
    Me,
    Others,
}

impl Speaker {
    pub fn as_str(self) -> &'static str {
        match self {
            Speaker::Me => "me",
            Speaker::Others => "others",
        }
    }

    pub fn parse(value: &str) -> Speaker {
        if value == "me" {
            Speaker::Me
        } else {
            Speaker::Others
        }
    }
}

/// One transcribed utterance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct MeetingSegment {
    pub id: i64,
    pub speaker: Speaker,
    /// Milliseconds since the meeting started (pauses excluded).
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
}

/// Lower-case words, punctuation stripped, for comparing transcripts.
pub fn normalized_words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// Phrases speech models emit on silence or noise, learned from subtitled
/// video. A segment that is only one of these is dropped.
const HALLUCINATIONS: &[&str] = &[
    "sous titres réalisés par la communauté d amara org",
    "sous titres réalisés para la communauté d amara org",
    "sous titrage st 501",
    "sous titrage société radio canada",
    "sous titres par la communauté d amara org",
    "merci d avoir regardé cette vidéo",
    "merci d avoir regardé",
    "merci de votre attention",
    "abonnez vous",
    "n oubliez pas de vous abonner",
    "thank you for watching",
    "thanks for watching",
    "please subscribe",
    "subtitles by the amara org community",
    "you",
];

/// Whether a transcribed segment is noise rather than speech: empty, only
/// punctuation, or a known hallucination.
pub fn is_noise(text: &str) -> bool {
    let words = normalized_words(text);
    if words.is_empty() {
        return true;
    }
    let joined = words.join(" ");
    HALLUCINATIONS.contains(&joined.as_str())
}

/// How far apart (ms) two segments may be and still be the same sound heard
/// on both tracks: the microphone picks up the speakers a few hundred
/// milliseconds after the system track plays them, and utterances are cut at
/// slightly different places.
const ECHO_WINDOW_MS: i64 = 2_500;

/// Whether `mic` is the microphone hearing `others` through the speakers
/// rather than the user talking. Requires the segments to overlap in time and
/// most of the microphone words to appear in the other segment.
pub fn is_echo(mic: &MeetingSegment, others: &MeetingSegment) -> bool {
    if mic.start_ms > others.end_ms + ECHO_WINDOW_MS
        || others.start_ms > mic.end_ms + ECHO_WINDOW_MS
    {
        return false;
    }
    let mic_words = normalized_words(&mic.text);
    let other_words = normalized_words(&others.text);
    if mic_words.is_empty() || other_words.is_empty() {
        return false;
    }
    if mic_words.len() < 3 {
        // "Oui", "d'accord": both sides say these all the time. Only an exact
        // match this close in time is an echo.
        return mic_words == other_words;
    }
    let mut pool = other_words.clone();
    let mut found = 0usize;
    for word in &mic_words {
        if let Some(pos) = pool.iter().position(|w| w == word) {
            pool.swap_remove(pos);
            found += 1;
        }
    }
    found as f64 / mic_words.len() as f64 >= 0.7
}

/// `ms` as `m:ss`, or `h:mm:ss` past an hour.
pub fn format_timestamp(ms: i64) -> String {
    let total = ms.max(0) / 1000;
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Consecutive segments of the same speaker read as one turn.
#[derive(Debug, Clone, PartialEq)]
pub struct Turn {
    pub speaker: Speaker,
    pub start_ms: i64,
    pub text: String,
}

/// Turns, in time order. Segments of one speaker closer than `GAP` merge.
pub fn turns(segments: &[MeetingSegment]) -> Vec<Turn> {
    const GAP_MS: i64 = 4_000;
    let mut sorted: Vec<&MeetingSegment> = segments.iter().collect();
    sorted.sort_by_key(|s| (s.start_ms, s.id));
    let mut out: Vec<(Turn, i64)> = Vec::new();
    for seg in sorted {
        let text = seg.text.trim();
        if text.is_empty() {
            continue;
        }
        if let Some((turn, end)) = out.last_mut() {
            if turn.speaker == seg.speaker && seg.start_ms - *end <= GAP_MS {
                turn.text.push(' ');
                turn.text.push_str(text);
                *end = (*end).max(seg.end_ms);
                continue;
            }
        }
        out.push((
            Turn {
                speaker: seg.speaker,
                start_ms: seg.start_ms,
                text: text.to_string(),
            },
            seg.end_ms,
        ));
    }
    out.into_iter().map(|(turn, _)| turn).collect()
}

/// Speaker labels in the transcript language.
#[derive(Debug, Clone, Copy)]
pub struct Labels<'a> {
    pub me: &'a str,
    pub others: &'a str,
}

impl Labels<'_> {
    pub fn of(&self, speaker: Speaker) -> &str {
        match speaker {
            Speaker::Me => self.me,
            Speaker::Others => self.others,
        }
    }
}

/// The transcript as plain lines, `[m:ss] Speaker: text`, for copying and
/// for the summary model.
pub fn plain_transcript(segments: &[MeetingSegment], labels: Labels<'_>) -> String {
    turns(segments)
        .iter()
        .map(|turn| {
            format!(
                "[{}] {}: {}",
                format_timestamp(turn.start_ms),
                labels.of(turn.speaker),
                turn.text
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Splits transcript lines into pieces of at most `max_chars`, never inside a
/// line unless that line alone is longer.
pub fn split_for_model(transcript: &str, max_chars: usize) -> Vec<String> {
    let max_chars = max_chars.max(1);
    let mut pieces = Vec::new();
    let mut current = String::new();
    for line in transcript.lines() {
        let mut line = line;
        while line.chars().count() > max_chars {
            if !current.is_empty() {
                pieces.push(std::mem::take(&mut current));
            }
            let split = line
                .char_indices()
                .nth(max_chars)
                .map(|(i, _)| i)
                .unwrap_or(line.len());
            pieces.push(line[..split].to_string());
            line = &line[split..];
        }
        let needed = if current.is_empty() {
            line.chars().count()
        } else {
            current.chars().count() + 1 + line.chars().count()
        };
        if needed > max_chars && !current.is_empty() {
            pieces.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push('\n');
        }
        current.push_str(line);
    }
    if !current.is_empty() {
        pieces.push(current);
    }
    pieces
}

/// What the Markdown export needs to know about a meeting.
pub struct MarkdownMeeting<'a> {
    pub title: &'a str,
    /// Already formatted for the reader, e.g. "21/10/2026 14:30".
    pub when: &'a str,
    pub duration_ms: i64,
    pub summary: Option<&'a str>,
    pub segments: &'a [MeetingSegment],
}

/// Headings of the Markdown export, in the export language.
pub struct MarkdownStrings<'a> {
    pub date: &'a str,
    pub duration: &'a str,
    pub summary: &'a str,
    pub transcript: &'a str,
    pub labels: Labels<'a>,
}

/// The whole meeting as one Markdown document: header, summary when there is
/// one, then the timestamped transcript.
pub fn render_markdown(meeting: &MarkdownMeeting<'_>, strings: &MarkdownStrings<'_>) -> String {
    let mut out = String::new();
    out.push_str(&format!("# {}\n\n", meeting.title.trim()));
    out.push_str(&format!("- **{}** : {}\n", strings.date, meeting.when));
    out.push_str(&format!(
        "- **{}** : {}\n\n",
        strings.duration,
        format_duration(meeting.duration_ms)
    ));
    if let Some(summary) = meeting.summary.map(str::trim).filter(|s| !s.is_empty()) {
        out.push_str(&format!("## {}\n\n", strings.summary));
        out.push_str(&demote_headings(summary));
        out.push_str("\n\n");
    }
    out.push_str(&format!("## {}\n\n", strings.transcript));
    for turn in turns(meeting.segments) {
        out.push_str(&format!(
            "**{}** · {}  \n{}\n\n",
            strings.labels.of(turn.speaker),
            format_timestamp(turn.start_ms),
            turn.text
        ));
    }
    out.trim_end().to_string() + "\n"
}

/// `h h mm` / `mm min` / `< 1 min`, language-neutral.
pub fn format_duration(ms: i64) -> String {
    let minutes = (ms.max(0) + 30_000) / 60_000;
    if minutes == 0 {
        "< 1 min".to_string()
    } else if minutes < 60 {
        format!("{minutes} min")
    } else {
        format!("{} h {:02}", minutes / 60, minutes % 60)
    }
}

/// Pushes every Markdown heading in `text` one level down (`#` → `###`), so a
/// summary's own `##` sections nest under the export's `## Summary`.
fn demote_headings(text: &str) -> String {
    text.lines()
        .map(|line| {
            let trimmed = line.trim_start();
            if trimmed.starts_with('#') {
                let level = trimmed.chars().take_while(|&c| c == '#').count();
                let rest = trimmed[level..].trim_start();
                format!("{} {}", "#".repeat((level + 1).clamp(3, 6)), rest)
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A file name for the export: the date, then the title with characters that
/// file systems or note apps dislike removed.
pub fn export_file_name(date_prefix: &str, title: &str) -> String {
    let cleaned: String = title
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '#' | '^' | '[' | ']' => ' ',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let cleaned: String = cleaned.chars().take(80).collect();
    let cleaned = cleaned.trim_matches(|c: char| c == '.' || c.is_whitespace());
    if cleaned.is_empty() {
        format!("{date_prefix}.md")
    } else {
        format!("{date_prefix} {cleaned}.md")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(id: i64, speaker: Speaker, start: i64, end: i64, text: &str) -> MeetingSegment {
        MeetingSegment {
            id,
            speaker,
            start_ms: start,
            end_ms: end,
            text: text.to_string(),
        }
    }

    const LABELS: Labels<'static> = Labels {
        me: "Moi",
        others: "Participants",
    };

    #[test]
    fn known_hallucinations_are_noise() {
        assert!(is_noise(
            "Sous-titres réalisés par la communauté d'Amara.org"
        ));
        assert!(is_noise("Merci d'avoir regardé !"));
        assert!(is_noise("Thanks for watching."));
        assert!(is_noise("  ...  "));
        assert!(!is_noise(
            "Merci d'avoir regardé le document, on peut avancer ?"
        ));
        assert!(!is_noise("On se voit demain."));
    }

    #[test]
    fn echo_requires_overlap_and_shared_words() {
        let others = seg(
            1,
            Speaker::Others,
            10_000,
            16_000,
            "On valide le budget pour le trimestre prochain",
        );
        let echo = seg(
            2,
            Speaker::Me,
            10_300,
            16_200,
            "on valide le budget pour le trimestre",
        );
        let later = seg(
            3,
            Speaker::Me,
            30_000,
            33_000,
            "on valide le budget pour le trimestre",
        );
        let reply = seg(
            4,
            Speaker::Me,
            16_500,
            18_000,
            "Parfait, je m'occupe du devis",
        );
        assert!(is_echo(&echo, &others));
        assert!(!is_echo(&later, &others), "too far apart in time");
        assert!(!is_echo(&reply, &others), "different words");
    }

    #[test]
    fn short_replies_are_only_echo_when_identical() {
        let others = seg(1, Speaker::Others, 0, 1_000, "D'accord.");
        let same = seg(2, Speaker::Me, 200, 1_100, "d'accord");
        let different = seg(3, Speaker::Me, 200, 1_100, "Oui, d'accord");
        assert!(is_echo(&same, &others));
        assert!(!is_echo(&different, &others));
    }

    #[test]
    fn timestamps_format_minutes_and_hours() {
        assert_eq!(format_timestamp(0), "0:00");
        assert_eq!(format_timestamp(65_400), "1:05");
        assert_eq!(format_timestamp(3_725_000), "1:02:05");
        assert_eq!(format_timestamp(-5), "0:00");
    }

    #[test]
    fn durations_round_to_minutes() {
        assert_eq!(format_duration(10_000), "< 1 min");
        assert_eq!(format_duration(42 * 60_000 + 10_000), "42 min");
        assert_eq!(format_duration(65 * 60_000), "1 h 05");
    }

    #[test]
    fn consecutive_segments_of_one_speaker_merge_into_a_turn() {
        let segments = vec![
            seg(3, Speaker::Others, 9_000, 11_000, "Oui."),
            seg(1, Speaker::Me, 0, 3_000, "Bonjour à tous."),
            seg(2, Speaker::Me, 4_000, 8_000, "On commence ?"),
            seg(4, Speaker::Me, 30_000, 31_000, "Merci."),
        ];
        let t = turns(&segments);
        assert_eq!(t.len(), 3);
        assert_eq!(t[0].text, "Bonjour à tous. On commence ?");
        assert_eq!(t[1].speaker, Speaker::Others);
        assert_eq!(t[2].start_ms, 30_000);
    }

    #[test]
    fn plain_transcript_has_timestamps_and_labels() {
        let segments = vec![
            seg(1, Speaker::Me, 0, 2_000, "Salut."),
            seg(2, Speaker::Others, 62_000, 64_000, "Bonjour !"),
        ];
        assert_eq!(
            plain_transcript(&segments, LABELS),
            "[0:00] Moi: Salut.\n[1:02] Participants: Bonjour !"
        );
    }

    #[test]
    fn split_keeps_lines_whole_and_bounded() {
        let text = "aaaa\nbbbb\ncccc\ndddd";
        assert_eq!(split_for_model(text, 9), vec!["aaaa\nbbbb", "cccc\ndddd"]);
        assert_eq!(split_for_model(text, 100), vec![text]);
        // A single overlong line is split rather than exceeding the limit.
        let long = "x".repeat(25);
        let pieces = split_for_model(&long, 10);
        assert_eq!(pieces.len(), 3);
        assert!(pieces.iter().all(|p| p.chars().count() <= 10));
    }

    #[test]
    fn markdown_export_nests_the_summary_and_lists_turns() {
        let segments = vec![
            seg(1, Speaker::Me, 0, 2_000, "Salut."),
            seg(2, Speaker::Others, 3_000, 5_000, "Bonjour."),
        ];
        let md = render_markdown(
            &MarkdownMeeting {
                title: "Point hebdo",
                when: "21/10/2026 14:30",
                duration_ms: 42 * 60_000,
                summary: Some("## Résumé\nTout va bien.\n## Actions\n- [ ] Moi : envoyer le devis"),
                segments: &segments,
            },
            &MarkdownStrings {
                date: "Date",
                duration: "Durée",
                summary: "Compte rendu",
                transcript: "Transcription",
                labels: LABELS,
            },
        );
        assert!(md.starts_with("# Point hebdo\n"));
        assert!(md.contains("- **Durée** : 42 min"));
        assert!(md.contains("## Compte rendu\n\n### Résumé\nTout va bien."));
        assert!(md.contains("### Actions"));
        assert!(md.contains("**Participants** · 0:03  \nBonjour."));
        assert!(md.ends_with("Bonjour.\n"));
    }

    #[test]
    fn markdown_export_without_summary_has_only_the_transcript() {
        let md = render_markdown(
            &MarkdownMeeting {
                title: "Réunion",
                when: "hier",
                duration_ms: 0,
                summary: None,
                segments: &[],
            },
            &MarkdownStrings {
                date: "Date",
                duration: "Duration",
                summary: "Summary",
                transcript: "Transcript",
                labels: LABELS,
            },
        );
        assert!(!md.contains("Summary"));
        assert!(md.contains("## Transcript"));
    }

    #[test]
    fn export_file_names_are_safe() {
        assert_eq!(
            export_file_name("2026-10-21 14h30", "Point: produit / design?"),
            "2026-10-21 14h30 Point produit design.md"
        );
        assert_eq!(export_file_name("2026-10-21", "   "), "2026-10-21.md");
        assert_eq!(export_file_name("d", "..."), "d.md");
    }

    #[test]
    fn speaker_round_trips_through_its_string() {
        for s in [Speaker::Me, Speaker::Others] {
            assert_eq!(Speaker::parse(s.as_str()), s);
        }
    }
}
