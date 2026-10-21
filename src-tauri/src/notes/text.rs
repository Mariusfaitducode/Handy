//! Pure text helpers for voice notes: titles, file names, Markdown layout and
//! LLM reply parsing. Everything here is side-effect free so it can be unit
//! tested without an `AppHandle`.

use chrono::{DateTime, Local};
use serde::Deserialize;

/// Longest title we keep. Long enough for a meaningful phrase, short enough
/// for a list row, a file name and a Notion page title.
pub const MAX_TITLE_CHARS: usize = 72;
/// Word budget for a title derived without an LLM.
const HEURISTIC_TITLE_WORDS: usize = 8;

/// A note ready to be stored: what the user said, tidied up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteDraft {
    pub title: String,
    pub body: String,
}

/// Derive a title from the start of the transcript: its first sentence, cut
/// to a few words. Used when no LLM is configured or when it fails.
pub fn heuristic_title(text: &str) -> String {
    let text = text.trim();
    if text.is_empty() {
        return String::new();
    }

    // First sentence (or first line), whichever ends first.
    let end = text
        .char_indices()
        .find(|(_, c)| matches!(c, '.' | '!' | '?' | '\n' | '…'))
        .map(|(i, _)| i)
        .unwrap_or(text.len());
    let sentence = text[..end].trim();
    let sentence = if sentence.is_empty() { text } else { sentence };

    let words: Vec<&str> = sentence.split_whitespace().collect();
    let mut title = words
        .iter()
        .take(HEURISTIC_TITLE_WORDS)
        .copied()
        .collect::<Vec<_>>()
        .join(" ");
    let truncated_words = words.len() > HEURISTIC_TITLE_WORDS;

    title = clean_title(&title);
    if truncated_words && !title.is_empty() {
        title.push('…');
    }
    capitalize_first(&title)
}

/// Normalise a title from any source: single line, no wrapping quotes or
/// trailing punctuation, bounded length.
pub fn clean_title(title: &str) -> String {
    let single_line = title.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = single_line
        .trim()
        .trim_matches(['"', '\'', '«', '»', '“', '”', '#', '*'])
        .trim()
        .trim_end_matches(['.', ',', ';', ':', '!', '?'])
        .trim();

    if trimmed.chars().count() <= MAX_TITLE_CHARS {
        return trimmed.to_string();
    }

    // Cut at the last word boundary that fits, then mark the cut.
    let mut out = String::new();
    for word in trimmed.split(' ') {
        let next_len = out.chars().count() + word.chars().count() + usize::from(!out.is_empty());
        if next_len > MAX_TITLE_CHARS - 1 {
            break;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    if out.is_empty() {
        out = trimmed.chars().take(MAX_TITLE_CHARS - 1).collect();
    }
    out.push('…');
    out
}

fn capitalize_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Draft used when no LLM is involved: the transcript as-is plus a derived title.
pub fn plain_draft(raw: &str) -> NoteDraft {
    NoteDraft {
        title: heuristic_title(raw),
        body: raw.trim().to_string(),
    }
}

#[derive(Deserialize)]
struct LlmNote {
    #[serde(default)]
    title: String,
    #[serde(default)]
    body: String,
}

/// Strip a leading `<think>…</think>` block some reasoning models emit.
fn strip_think_block(s: &str) -> &str {
    if let Some(rest) = s.trim_start().strip_prefix("<think>") {
        if let Some(end) = rest.find("</think>") {
            return rest[end + "</think>".len()..].trim_start();
        }
    }
    s
}

/// Parse the LLM reply (`{"title": …, "body": …}`), tolerating code fences,
/// a reasoning preamble or prose around the JSON object. Falls back to the
/// raw transcript for any missing part so a sloppy reply never loses words.
pub fn parse_llm_reply(reply: &str, raw: &str) -> Option<NoteDraft> {
    let reply =
        strip_think_block(reply).replace(['\u{200B}', '\u{200C}', '\u{200D}', '\u{FEFF}'], "");
    let start = reply.find('{')?;
    let end = reply.rfind('}')?;
    if end <= start {
        return None;
    }
    let parsed: LlmNote = serde_json::from_str(&reply[start..=end]).ok()?;

    let body = parsed.body.trim();
    let body = if body.is_empty() {
        raw.trim().to_string()
    } else {
        body.to_string()
    };
    let title = clean_title(&parsed.title);
    let title = if title.is_empty() {
        heuristic_title(&body)
    } else {
        title
    };
    Some(NoteDraft { title, body })
}

/// Characters that are invalid (or awkward) in file names on any desktop OS.
fn is_forbidden_filename_char(c: char) -> bool {
    matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control()
}

/// Turn a title into a safe file-name fragment.
pub fn sanitize_filename(title: &str) -> String {
    let cleaned: String = title
        .chars()
        .map(|c| {
            if is_forbidden_filename_char(c) {
                ' '
            } else {
                c
            }
        })
        .collect();
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    // Leading dots would hide the file on macOS/Linux.
    let collapsed = collapsed.trim_start_matches('.').trim().to_string();
    let bounded: String = collapsed.chars().take(60).collect();
    bounded.trim_end_matches(['.', ' ', '…']).to_string()
}

/// `2026-10-21.md`: the journal file for the day the note was taken.
pub fn daily_file_name(at: &DateTime<Local>) -> String {
    format!("{}.md", at.format("%Y-%m-%d"))
}

/// `2026-10-21 14-32 Idea for the landing page.md`. Sorts chronologically
/// in any file browser and stays readable.
pub fn note_file_name(at: &DateTime<Local>, title: &str) -> String {
    let stem = sanitize_filename(title);
    let prefix = at.format("%Y-%m-%d %H-%M");
    if stem.is_empty() {
        format!("{prefix}.md")
    } else {
        format!("{prefix} {stem}.md")
    }
}

/// Marker that delimits a note inside a daily journal file. Invisible in
/// rendered Markdown, and lets Handy remove exactly that section on delete.
pub fn section_marker(uid: &str) -> String {
    format!("<!-- handy-note:{uid} -->")
}

const MARKER_PREFIX: &str = "<!-- handy-note:";

/// Header written at the top of a new daily journal file.
pub fn daily_file_header(at: &DateTime<Local>) -> String {
    format!("# {}\n", at.format("%Y-%m-%d"))
}

/// The block appended to a daily journal file for one note.
pub fn daily_section(uid: &str, at: &DateTime<Local>, draft: &NoteDraft) -> String {
    format!(
        "\n{}\n## {} · {}\n\n{}\n",
        section_marker(uid),
        at.format("%H:%M"),
        draft.title,
        draft.body.trim()
    )
}

/// Full contents of a one-file-per-note Markdown file.
pub fn note_file_contents(at: &DateTime<Local>, draft: &NoteDraft) -> String {
    format!(
        "---\ntitle: {}\ncreated: {}\nsource: Handy\n---\n\n# {}\n\n{}\n",
        yaml_quote(&draft.title),
        at.to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
        draft.title,
        draft.body.trim()
    )
}

fn yaml_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Remove the section tagged with `uid` from a daily journal. A section runs
/// from its marker line to the next marker (or the end of the file). Returns
/// `None` when the marker is not found, so callers never rewrite a file they
/// don't understand.
pub fn remove_daily_section(contents: &str, uid: &str) -> Option<String> {
    let marker = section_marker(uid);
    let lines: Vec<&str> = contents.split_inclusive('\n').collect();
    let start = lines.iter().position(|l| l.trim() == marker)?;
    let end = lines[start + 1..]
        .iter()
        .position(|l| l.trim_start().starts_with(MARKER_PREFIX))
        .map(|offset| start + 1 + offset)
        .unwrap_or(lines.len());

    let mut before: String = lines[..start].concat();
    let after: String = lines[end..].concat();

    // Keep exactly one blank line between the surviving neighbours.
    while before.ends_with("\n\n") {
        before.pop();
    }
    let mut out = before;
    if !after.is_empty() {
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(after.trim_start_matches('\n'));
    }
    Some(out)
}

/// True when a daily journal holds nothing but its header (and whitespace),
/// i.e. deleting its last note should delete the file too.
pub fn daily_file_is_empty(contents: &str) -> bool {
    contents
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .all(|l| l.starts_with("# ") && !l.starts_with("## "))
}

/// Accept a Notion page link or a bare ID and return the dashed page UUID.
/// Notion links end in the 32-hex-char ID, optionally with dashes, and may
/// carry a `?v=` / `?pvs=` query or a `#block` fragment.
pub fn notion_page_id(input: &str) -> Option<String> {
    let input = input.trim();
    let path = input.split(['?', '#']).next().unwrap_or(input);
    let hex: String = path
        .chars()
        .rev()
        .filter(|c| *c != '-')
        .take_while(|c| c.is_ascii_hexdigit())
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    if hex.len() < 32 {
        return None;
    }
    let id = &hex[hex.len() - 32..];
    Some(format!(
        "{}-{}-{}-{}-{}",
        &id[0..8],
        &id[8..12],
        &id[12..16],
        &id[16..20],
        &id[20..32]
    ))
}

/// One Notion block derived from the note body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BodyBlock {
    Paragraph(String),
    Bullet(String),
}

/// Split the body into Notion-sized blocks: one per line, bullets kept as
/// bullets, each chunk under Notion's 2000-character rich-text limit.
pub fn body_blocks(body: &str) -> Vec<BodyBlock> {
    const LIMIT: usize = 1900;
    let mut blocks = Vec::new();
    for line in body.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let (is_bullet, text) = match line
            .strip_prefix("- ")
            .or_else(|| line.strip_prefix("* "))
            .or_else(|| line.strip_prefix("• "))
        {
            Some(rest) => (true, rest.trim()),
            None => (false, line),
        };
        for chunk in chunk_chars(text, LIMIT) {
            blocks.push(if is_bullet {
                BodyBlock::Bullet(chunk)
            } else {
                BodyBlock::Paragraph(chunk)
            });
        }
    }
    blocks
}

fn chunk_chars(text: &str, limit: usize) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    chars
        .chunks(limit)
        .map(|c| c.iter().collect::<String>())
        .collect()
}

/// Escape text for the HTML body Apple Notes expects.
pub fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// HTML body for an Apple Notes note. Notes uses the first line as the
/// note's name, so the title goes first as a heading.
pub fn apple_notes_html(draft: &NoteDraft) -> String {
    let mut html = format!("<div><h1>{}</h1></div>", html_escape(&draft.title));
    let mut in_list = false;
    for block in body_blocks(&draft.body) {
        match block {
            BodyBlock::Bullet(text) => {
                if !in_list {
                    html.push_str("<ul>");
                    in_list = true;
                }
                html.push_str(&format!("<li>{}</li>", html_escape(&text)));
            }
            BodyBlock::Paragraph(text) => {
                if in_list {
                    html.push_str("</ul>");
                    in_list = false;
                }
                html.push_str(&format!("<div>{}</div>", html_escape(&text)));
            }
        }
    }
    if in_list {
        html.push_str("</ul>");
    }
    html
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at() -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 10, 21, 14, 32, 5).unwrap()
    }

    #[test]
    fn heuristic_title_uses_first_sentence() {
        assert_eq!(
            heuristic_title("rappeler Yasmine demain. Et aussi acheter du pain."),
            "Rappeler Yasmine demain"
        );
    }

    #[test]
    fn heuristic_title_truncates_long_sentences_on_words() {
        let title = heuristic_title(
            "il faudrait vraiment que je pense à revoir toute la page d'accueil du site",
        );
        assert_eq!(title, "Il faudrait vraiment que je pense à revoir…");
    }

    #[test]
    fn heuristic_title_of_blank_text_is_empty() {
        assert_eq!(heuristic_title("   \n "), "");
    }

    #[test]
    fn heuristic_title_starting_with_punctuation_falls_back_to_text() {
        assert_eq!(heuristic_title("...bon"), "...bon");
    }

    #[test]
    fn clean_title_strips_quotes_and_trailing_punctuation() {
        assert_eq!(clean_title("  « Idée produit. »  "), "Idée produit");
        assert_eq!(
            clean_title("\"Plan\nde la semaine:\""),
            "Plan de la semaine"
        );
    }

    #[test]
    fn clean_title_bounds_length_on_word_boundary() {
        let long = "mot ".repeat(40);
        let title = clean_title(&long);
        assert!(title.chars().count() <= MAX_TITLE_CHARS);
        assert!(title.ends_with('…'));
        assert!(!title.contains("  "));
    }

    #[test]
    fn llm_reply_is_parsed_from_plain_json() {
        let draft = parse_llm_reply(
            r#"{"title":"Idée landing","body":"Mettre la démo en premier."}"#,
            "raw",
        )
        .unwrap();
        assert_eq!(draft.title, "Idée landing");
        assert_eq!(draft.body, "Mettre la démo en premier.");
    }

    #[test]
    fn llm_reply_tolerates_fences_and_think_blocks() {
        let reply = "<think>hmm</think>\n```json\n{\"title\": \"Courses.\", \"body\": \"- pain\\n- lait\"}\n```";
        let draft = parse_llm_reply(reply, "raw").unwrap();
        assert_eq!(draft.title, "Courses");
        assert_eq!(draft.body, "- pain\n- lait");
    }

    #[test]
    fn llm_reply_missing_parts_fall_back_to_raw() {
        let draft = parse_llm_reply(r#"{"title":""}"#, "acheter du pain. vite").unwrap();
        assert_eq!(draft.body, "acheter du pain. vite");
        assert_eq!(draft.title, "Acheter du pain");
    }

    #[test]
    fn llm_reply_without_json_is_rejected() {
        assert!(parse_llm_reply("Sure! Here is your note.", "raw").is_none());
        assert!(parse_llm_reply("} oops {", "raw").is_none());
    }

    #[test]
    fn filenames_are_sanitized() {
        assert_eq!(sanitize_filename("a/b:c*d?\"e\"<f>|g"), "a b c d e f g");
        assert_eq!(sanitize_filename("..hidden"), "hidden");
        assert_eq!(sanitize_filename("Fin…"), "Fin");
        assert!(sanitize_filename(&"x".repeat(200)).chars().count() <= 60);
    }

    #[test]
    fn note_file_name_is_chronological_and_readable() {
        assert_eq!(
            note_file_name(&at(), "Idée: landing page"),
            "2026-10-21 14-32 Idée landing page.md"
        );
        assert_eq!(note_file_name(&at(), "///"), "2026-10-21 14-32.md");
        assert_eq!(daily_file_name(&at()), "2026-10-21.md");
    }

    #[test]
    fn note_file_contents_has_front_matter() {
        let draft = NoteDraft {
            title: "Say \"hi\"".into(),
            body: "Body\n".into(),
        };
        let contents = note_file_contents(&at(), &draft);
        assert!(contents.starts_with("---\ntitle: \"Say \\\"hi\\\"\"\n"));
        assert!(contents.contains("source: Handy"));
        assert!(contents.ends_with("# Say \"hi\"\n\nBody\n"));
    }

    fn journal() -> String {
        let a = NoteDraft {
            title: "First".into(),
            body: "one".into(),
        };
        let b = NoteDraft {
            title: "Second".into(),
            body: "two\n\nlines".into(),
        };
        let c = NoteDraft {
            title: "Third".into(),
            body: "three".into(),
        };
        format!(
            "{}{}{}{}",
            daily_file_header(&at()),
            daily_section("1-1", &at(), &a),
            daily_section("1-2", &at(), &b),
            daily_section("1-3", &at(), &c)
        )
    }

    #[test]
    fn removing_a_middle_section_keeps_neighbours() {
        let out = remove_daily_section(&journal(), "1-2").unwrap();
        assert!(out.contains("## 14:32 · First\n\none\n"));
        assert!(!out.contains("Second"));
        assert!(!out.contains("lines"));
        assert!(out.contains("<!-- handy-note:1-3 -->\n## 14:32 · Third"));
        assert!(!out.contains("\n\n\n"));
    }

    #[test]
    fn removing_the_last_section_trims_the_tail() {
        let out = remove_daily_section(&journal(), "1-3").unwrap();
        assert!(!out.contains("Third"));
        assert!(out.ends_with("lines\n"));
    }

    #[test]
    fn removing_an_unknown_section_is_refused() {
        assert!(remove_daily_section(&journal(), "9-9").is_none());
    }

    #[test]
    fn journal_with_only_header_is_empty() {
        let mut contents = journal();
        for uid in ["1-1", "1-2", "1-3"] {
            contents = remove_daily_section(&contents, uid).unwrap();
        }
        assert!(daily_file_is_empty(&contents));
        assert!(!daily_file_is_empty(&journal()));
    }

    #[test]
    fn notion_ids_are_extracted_from_links() {
        let expected = "1a2b3c4d-5e6f-7a8b-9c0d-1e2f3a4b5c6d";
        for input in [
            "https://www.notion.so/acme/Journal-1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d",
            "https://www.notion.so/1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d?pvs=4",
            "notion.so/Journal-1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d#abc",
            "1a2b3c4d-5e6f-7a8b-9c0d-1e2f3a4b5c6d",
            "  1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d  ",
        ] {
            assert_eq!(notion_page_id(input).as_deref(), Some(expected), "{input}");
        }
        assert_eq!(notion_page_id("https://www.notion.so/acme/Journal"), None);
        assert_eq!(notion_page_id(""), None);
    }

    #[test]
    fn body_blocks_keep_bullets_and_split_long_lines() {
        let long = "a".repeat(4000);
        let body = format!("Intro\n\n- one\n* two\n{long}");
        let blocks = body_blocks(&body);
        assert_eq!(blocks[0], BodyBlock::Paragraph("Intro".into()));
        assert_eq!(blocks[1], BodyBlock::Bullet("one".into()));
        assert_eq!(blocks[2], BodyBlock::Bullet("two".into()));
        assert_eq!(blocks.len(), 6);
        assert!(blocks[3..].iter().all(|b| match b {
            BodyBlock::Paragraph(t) => t.chars().count() <= 1900,
            BodyBlock::Bullet(_) => false,
        }));
    }

    #[test]
    fn apple_notes_html_is_escaped_and_structured() {
        let draft = NoteDraft {
            title: "A & B".into(),
            body: "<intro>\n- x\n- y\nend".into(),
        };
        assert_eq!(
            apple_notes_html(&draft),
            "<div><h1>A &amp; B</h1></div><div>&lt;intro&gt;</div><ul><li>x</li><li>y</li></ul><div>end</div>"
        );
    }
}
