//! Local Markdown destination: a journal file per day or a file per note.

use super::settings::MarkdownLayout;
use super::text::{self, NoteDraft};
use chrono::{DateTime, Local};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Stable error codes the UI translates.
pub const ERR_PERMISSION: &str = "permission_denied";
pub const ERR_IO: &str = "io";

fn io_code(e: &io::Error) -> String {
    match e.kind() {
        io::ErrorKind::PermissionDenied => ERR_PERMISSION.to_string(),
        _ => format!("{ERR_IO}: {e}"),
    }
}

/// Write the note and return the file it landed in.
pub fn write_note(
    folder: &Path,
    layout: MarkdownLayout,
    uid: &str,
    at: &DateTime<Local>,
    draft: &NoteDraft,
) -> Result<PathBuf, String> {
    fs::create_dir_all(folder).map_err(|e| io_code(&e))?;
    match layout {
        MarkdownLayout::Daily => {
            let path = folder.join(text::daily_file_name(at));
            let is_new = !path.exists();
            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .map_err(|e| io_code(&e))?;
            let mut chunk = String::new();
            if is_new {
                chunk.push_str(&text::daily_file_header(at));
            }
            chunk.push_str(&text::daily_section(uid, at, draft));
            file.write_all(chunk.as_bytes()).map_err(|e| io_code(&e))?;
            Ok(path)
        }
        MarkdownLayout::PerNote => {
            let path = unique_path(folder, &text::note_file_name(at, &draft.title));
            // create_new: never overwrite a file the user may have edited.
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .map_err(|e| io_code(&e))?;
            file.write_all(text::note_file_contents(at, draft).as_bytes())
                .map_err(|e| io_code(&e))?;
            Ok(path)
        }
    }
}

/// `name.md`, or `name (2).md`, `name (3).md`… when taken.
fn unique_path(folder: &Path, file_name: &str) -> PathBuf {
    let candidate = folder.join(file_name);
    if !candidate.exists() {
        return candidate;
    }
    let stem = file_name.trim_end_matches(".md");
    (2..)
        .map(|n| folder.join(format!("{stem} ({n}).md")))
        .find(|p| !p.exists())
        .expect("an unused file name always exists")
}

/// What happened to the Markdown copy when a note was deleted.
#[derive(Debug, PartialEq, Eq)]
pub enum RemoveOutcome {
    Removed,
    /// The file or section was already gone.
    Missing,
    /// The journal no longer contains Handy's marker (edited by hand), so it
    /// was left untouched.
    LeftUntouched,
}

/// Remove a note's Markdown copy. A per-note file is deleted; a daily
/// journal loses only that note's section (and is deleted once empty).
pub fn remove_note(path: &Path, uid: &str) -> Result<RemoveOutcome, String> {
    let contents = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(RemoveOutcome::Missing),
        Err(e) => return Err(io_code(&e)),
    };

    let marker = text::section_marker(uid);
    if !contents.contains(&marker) {
        // Per-note files carry no marker: they are entirely the note. A daily
        // journal without the marker was edited; don't guess.
        if contents.contains("<!-- handy-note:") {
            return Ok(RemoveOutcome::LeftUntouched);
        }
        if contents.starts_with("---\n") && contents.contains("source: Handy") {
            fs::remove_file(path).map_err(|e| io_code(&e))?;
            return Ok(RemoveOutcome::Removed);
        }
        return Ok(RemoveOutcome::LeftUntouched);
    }

    let Some(updated) = text::remove_daily_section(&contents, uid) else {
        return Ok(RemoveOutcome::LeftUntouched);
    };
    if text::daily_file_is_empty(&updated) {
        fs::remove_file(path).map_err(|e| io_code(&e))?;
    } else {
        fs::write(path, updated).map_err(|e| io_code(&e))?;
    }
    Ok(RemoveOutcome::Removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "handy-md-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn draft(title: &str) -> NoteDraft {
        NoteDraft {
            title: title.into(),
            body: format!("{title} body"),
        }
    }

    #[test]
    fn daily_layout_appends_and_removes_sections() {
        let dir = temp_dir("daily");
        let at = Local.with_ymd_and_hms(2026, 10, 21, 9, 5, 0).unwrap();
        let p1 = write_note(&dir, MarkdownLayout::Daily, "1-1", &at, &draft("Un")).unwrap();
        let p2 = write_note(&dir, MarkdownLayout::Daily, "1-2", &at, &draft("Deux")).unwrap();
        assert_eq!(p1, p2);
        let contents = fs::read_to_string(&p1).unwrap();
        assert!(contents.starts_with("# 2026-10-21\n"));
        assert_eq!(contents.matches("# 2026-10-21").count(), 1);
        assert!(contents.contains("## 09:05 · Un"));
        assert!(contents.contains("## 09:05 · Deux"));

        assert_eq!(remove_note(&p1, "1-1").unwrap(), RemoveOutcome::Removed);
        let contents = fs::read_to_string(&p1).unwrap();
        assert!(!contents.contains("Un body"));
        assert!(contents.contains("Deux body"));

        assert_eq!(remove_note(&p1, "1-2").unwrap(), RemoveOutcome::Removed);
        assert!(!p1.exists(), "an emptied journal is deleted");
        assert_eq!(remove_note(&p1, "1-2").unwrap(), RemoveOutcome::Missing);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn per_note_layout_never_overwrites() {
        let dir = temp_dir("pernote");
        let at = Local.with_ymd_and_hms(2026, 10, 21, 9, 5, 0).unwrap();
        let p1 = write_note(&dir, MarkdownLayout::PerNote, "1-1", &at, &draft("Idée")).unwrap();
        let p2 = write_note(&dir, MarkdownLayout::PerNote, "1-2", &at, &draft("Idée")).unwrap();
        assert_ne!(p1, p2);
        assert!(p2.to_string_lossy().ends_with("Idée (2).md"));

        assert_eq!(remove_note(&p1, "1-1").unwrap(), RemoveOutcome::Removed);
        assert!(!p1.exists());
        assert!(p2.exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn hand_edited_files_are_left_alone() {
        let dir = temp_dir("edited");
        let path = dir.join("mine.md");
        fs::write(&path, "my own notes\n").unwrap();
        assert_eq!(
            remove_note(&path, "1-1").unwrap(),
            RemoveOutcome::LeftUntouched
        );
        assert!(path.exists());
        let _ = fs::remove_dir_all(dir);
    }
}
