//! Shared dictionary files: export/import and the shared-folder sync.
//!
//! The shared folder holds one `handy-dictionary.json`. Each Mac merges it
//! into its own dictionary and writes the union back, so any number of
//! people can sync through iCloud Drive, Dropbox or a network share without a
//! server. Copies the cloud provider makes on conflicting writes
//! ("handy-dictionary 2.json") are merged too, then removed.
//! A file that cannot be read is never overwritten: losing someone's words to
//! a half-downloaded file would be worse than a failed sync.

use super::entries::{live_entries, merge_entries, DictionaryEntry, MergeMode, MergeStats};
use serde::{Deserialize, Serialize};
use specta::Type;
use std::fs;
use std::path::{Path, PathBuf};

pub const SHARED_FILE_NAME: &str = "handy-dictionary.json";
const FILE_STEM: &str = "handy-dictionary";
const FORMAT: &str = "handy-dictionary";
const FORMAT_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SharedFile {
    pub format: String,
    pub version: u32,
    #[serde(default)]
    pub updated_by: String,
    #[serde(default)]
    pub updated_at: i64,
    #[serde(default)]
    pub entries: Vec<DictionaryEntry>,
}

impl SharedFile {
    pub fn new(entries: Vec<DictionaryEntry>, author: &str, now: i64) -> Self {
        Self {
            format: FORMAT.to_string(),
            version: FORMAT_VERSION,
            updated_by: author.to_string(),
            updated_at: now,
            entries,
        }
    }
}

/// Why a sync or import did not go through. The frontend turns the kind into
/// a sentence; `detail` is only for the logs.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Type)]
#[serde(rename_all = "snake_case")]
pub enum SyncErrorKind {
    FolderMissing,
    /// iCloud has the file but has not downloaded it to this Mac yet.
    NotDownloaded,
    Unreadable,
    WriteFailed,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Type)]
pub struct SyncError {
    pub kind: SyncErrorKind,
    pub detail: String,
}

impl SyncError {
    fn new(kind: SyncErrorKind, detail: impl std::fmt::Display) -> Self {
        Self {
            kind,
            detail: detail.to_string(),
        }
    }
}

/// Accepts our own format, or a plain JSON list of words for people bringing
/// a list from elsewhere.
pub fn parse_entries(
    contents: &str,
    author: &str,
    now: i64,
) -> Result<Vec<DictionaryEntry>, String> {
    let value: serde_json::Value = serde_json::from_str(contents).map_err(|e| e.to_string())?;
    if let Some(words) = value.as_array() {
        if words.iter().all(|w| w.is_string()) {
            return Ok(words
                .iter()
                .filter_map(|w| w.as_str())
                .map(|w| DictionaryEntry::new(super::entries::EntryKind::Word, w, "", author, now))
                .collect());
        }
    }
    let file: SharedFile = serde_json::from_value(value).map_err(|e| e.to_string())?;
    if file.format != FORMAT {
        return Err(format!("not a Handy dictionary (format '{}')", file.format));
    }
    Ok(file.entries)
}

/// Writes next to the destination and renames, so readers (and iCloud) never
/// see a half-written file.
pub fn write_atomically(path: &Path, contents: &str) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = dir.join(format!(".{}.tmp-{}", name, std::process::id()));
    fs::write(&tmp, contents)?;
    fs::rename(&tmp, path).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

pub fn export_to(
    path: &Path,
    entries: &[DictionaryEntry],
    author: &str,
    now: i64,
) -> std::io::Result<usize> {
    let live = live_entries(entries);
    let count = live.len();
    let file = SharedFile::new(live, author, now);
    let json = serde_json::to_string_pretty(&file).map_err(std::io::Error::other)?;
    write_atomically(path, &json)?;
    Ok(count)
}

pub fn import_from(
    path: &Path,
    local: &mut Vec<DictionaryEntry>,
    author: &str,
    now: i64,
) -> Result<MergeStats, SyncError> {
    let contents =
        fs::read_to_string(path).map_err(|e| SyncError::new(SyncErrorKind::Unreadable, e))?;
    let incoming = parse_entries(&contents, author, now)
        .map_err(|e| SyncError::new(SyncErrorKind::Unreadable, e))?;
    Ok(merge_entries(local, &incoming, MergeMode::Import))
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct SyncOutcome {
    /// What the shared folder brought to this Mac.
    pub stats: MergeStats,
    /// Whether the shared file was (re)written.
    pub wrote: bool,
}

/// The iCloud placeholder macOS leaves while a file is still in the cloud.
fn icloud_placeholder(folder: &Path, name: &str) -> PathBuf {
    folder.join(format!(".{}.icloud", name))
}

/// Conflict copies of the shared file left by the cloud provider.
fn conflict_copies(folder: &Path) -> Vec<PathBuf> {
    let Ok(dir) = fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut copies: Vec<PathBuf> = dir
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            name != SHARED_FILE_NAME
                && name.starts_with(FILE_STEM)
                && name.ends_with(".json")
                && p.is_file()
        })
        .collect();
    copies.sort();
    copies
}

/// One sync round with the shared folder.
pub fn sync_with_folder(
    folder: &Path,
    local: &mut Vec<DictionaryEntry>,
    author: &str,
    now: i64,
) -> Result<SyncOutcome, SyncError> {
    if !folder.is_dir() {
        return Err(SyncError::new(
            SyncErrorKind::FolderMissing,
            folder.display(),
        ));
    }
    let shared_path = folder.join(SHARED_FILE_NAME);
    let mut outcome = SyncOutcome::default();

    let remote: Option<Vec<DictionaryEntry>> = if shared_path.is_file() {
        let contents = fs::read_to_string(&shared_path)
            .map_err(|e| SyncError::new(SyncErrorKind::Unreadable, e))?;
        Some(
            parse_entries(&contents, author, now)
                .map_err(|e| SyncError::new(SyncErrorKind::Unreadable, e))?,
        )
    } else if icloud_placeholder(folder, SHARED_FILE_NAME).exists() {
        return Err(SyncError::new(
            SyncErrorKind::NotDownloaded,
            shared_path.display(),
        ));
    } else {
        None
    };

    if let Some(remote) = &remote {
        outcome.stats = merge_entries(local, remote, MergeMode::Sync);
    }

    // Conflict copies: merge what can be read; leave the rest for a human.
    let mut merged_copies = Vec::new();
    for copy in conflict_copies(folder) {
        let parsed = fs::read_to_string(&copy)
            .ok()
            .and_then(|c| parse_entries(&c, author, now).ok());
        if let Some(entries) = parsed {
            let stats = merge_entries(local, &entries, MergeMode::Sync);
            outcome.stats.added += stats.added;
            outcome.stats.updated += stats.updated;
            outcome.stats.removed += stats.removed;
            merged_copies.push(copy);
        }
    }

    // Rewrite only when the folder is behind, to avoid churning iCloud.
    let mut sorted_local = local.clone();
    sorted_local.sort_by(|a, b| a.id.cmp(&b.id));
    let folder_up_to_date = merged_copies.is_empty()
        && remote.as_ref().is_some_and(|remote| {
            let mut sorted_remote: Vec<DictionaryEntry> = remote
                .iter()
                .filter_map(DictionaryEntry::sanitized)
                .collect();
            sorted_remote.sort_by(|a, b| a.id.cmp(&b.id));
            sorted_remote == sorted_local
        });

    if !folder_up_to_date {
        let file = SharedFile::new(sorted_local, author, now);
        let json = serde_json::to_string_pretty(&file)
            .map_err(|e| SyncError::new(SyncErrorKind::WriteFailed, e))?;
        write_atomically(&shared_path, &json)
            .map_err(|e| SyncError::new(SyncErrorKind::WriteFailed, e))?;
        outcome.wrote = true;
        for copy in merged_copies {
            let _ = fs::remove_file(copy);
        }
    }
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::super::entries::EntryKind;
    use super::*;

    fn word(from: &str, by: &str, at: i64) -> DictionaryEntry {
        DictionaryEntry::new(EntryKind::Word, from, "", by, at)
    }

    fn live_terms(entries: &[DictionaryEntry]) -> Vec<String> {
        let mut terms: Vec<String> = live_entries(entries).into_iter().map(|e| e.from).collect();
        terms.sort();
        terms
    }

    #[test]
    fn two_macs_converge_through_the_folder() {
        let folder = tempfile::tempdir().unwrap();
        let mut marius = vec![word("Handy", "Marius", 1)];
        let mut yasmine = vec![word("Yasmine", "Yasmine", 2)];

        sync_with_folder(folder.path(), &mut marius, "Marius", 10).unwrap();
        let y = sync_with_folder(folder.path(), &mut yasmine, "Yasmine", 11).unwrap();
        assert_eq!(y.stats.added, 1);
        let m = sync_with_folder(folder.path(), &mut marius, "Marius", 12).unwrap();
        assert_eq!(m.stats.added, 1);
        assert!(!m.wrote, "nothing new to write back");

        assert_eq!(live_terms(&marius), vec!["Handy", "Yasmine"]);
        assert_eq!(live_terms(&yasmine), vec!["Handy", "Yasmine"]);
    }

    #[test]
    fn deletion_reaches_the_other_mac() {
        let folder = tempfile::tempdir().unwrap();
        let mut marius = vec![word("Wispr", "Marius", 1)];
        let mut yasmine = Vec::new();
        sync_with_folder(folder.path(), &mut marius, "Marius", 2).unwrap();
        sync_with_folder(folder.path(), &mut yasmine, "Yasmine", 3).unwrap();

        marius[0].deleted = true;
        marius[0].updated_at = 4;
        sync_with_folder(folder.path(), &mut marius, "Marius", 5).unwrap();
        let outcome = sync_with_folder(folder.path(), &mut yasmine, "Yasmine", 6).unwrap();
        assert_eq!(outcome.stats.removed, 1);
        assert!(live_terms(&yasmine).is_empty());
    }

    #[test]
    fn unreadable_shared_file_is_left_untouched() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join(SHARED_FILE_NAME);
        fs::write(&path, "{ half written").unwrap();
        let mut local = vec![word("Handy", "Marius", 1)];
        let err = sync_with_folder(folder.path(), &mut local, "Marius", 2).unwrap_err();
        assert_eq!(err.kind, SyncErrorKind::Unreadable);
        assert_eq!(fs::read_to_string(&path).unwrap(), "{ half written");
    }

    #[test]
    fn icloud_placeholder_waits_for_download() {
        let folder = tempfile::tempdir().unwrap();
        fs::write(folder.path().join(".handy-dictionary.json.icloud"), "").unwrap();
        let mut local = vec![word("Handy", "Marius", 1)];
        let err = sync_with_folder(folder.path(), &mut local, "Marius", 2).unwrap_err();
        assert_eq!(err.kind, SyncErrorKind::NotDownloaded);
        assert!(!folder.path().join(SHARED_FILE_NAME).exists());
    }

    #[test]
    fn conflict_copies_are_merged_then_removed() {
        let folder = tempfile::tempdir().unwrap();
        let copy = folder.path().join("handy-dictionary 2.json");
        let file = SharedFile::new(vec![word("Yasmine", "Yasmine", 3)], "Yasmine", 3);
        fs::write(&copy, serde_json::to_string(&file).unwrap()).unwrap();

        let mut local = vec![word("Handy", "Marius", 1)];
        let outcome = sync_with_folder(folder.path(), &mut local, "Marius", 4).unwrap();
        assert_eq!(outcome.stats.added, 1);
        assert!(!copy.exists());
        assert!(folder.path().join(SHARED_FILE_NAME).exists());
    }

    #[test]
    fn missing_folder_is_reported() {
        let mut local = Vec::new();
        let err =
            sync_with_folder(Path::new("/nonexistent/handy"), &mut local, "M", 1).unwrap_err();
        assert_eq!(err.kind, SyncErrorKind::FolderMissing);
    }

    #[test]
    fn export_skips_deleted_words_and_import_never_deletes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("export.json");
        let mut gone = word("Wispr", "Marius", 1);
        gone.deleted = true;
        let count = export_to(&path, &[word("Handy", "Marius", 1), gone], "Marius", 2).unwrap();
        assert_eq!(count, 1);

        let mut yasmine = vec![word("Yasmine", "Yasmine", 1)];
        let stats = import_from(&path, &mut yasmine, "Yasmine", 3).unwrap();
        assert_eq!(stats.added, 1);
        assert_eq!(live_terms(&yasmine), vec!["Handy", "Yasmine"]);
    }

    #[test]
    fn plain_word_lists_can_be_imported() {
        let entries = parse_entries(r#"["Handy", "Yasmine"]"#, "Marius", 1).unwrap();
        assert_eq!(entries.len(), 2);
        assert!(parse_entries(r#"{"format":"other","version":1}"#, "M", 1).is_err());
    }
}
