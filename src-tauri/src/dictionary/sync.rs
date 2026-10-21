//! Shared dictionary files: export/import and the shared-folder sync.
//!
//! The shared folder holds one `handy-dictionary.json`. Each Mac merges it
//! into its own dictionary and writes the union back, so any number of
//! people can sync through iCloud Drive, Dropbox or a network share without a
//! server. Copies the cloud provider makes on conflicting writes
//! ("handy-dictionary 2.json") are merged too, then removed.
//! A file that cannot be read is never overwritten: losing someone's words to
//! a half-downloaded file would be worse than a failed sync.
//!
//! The shared folder is writable by others, so nothing in it is trusted:
//! symbolic links are never followed, reads are size-capped, and writes go
//! through a randomly named temporary file created exclusively.

use super::entries::{live_entries, merge_entries, DictionaryEntry, MergeMode, MergeStats};
use serde::{Deserialize, Serialize};
use specta::Type;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

pub const SHARED_FILE_NAME: &str = "handy-dictionary.json";
const FILE_STEM: &str = "handy-dictionary";
const FORMAT: &str = "handy-dictionary";
const FORMAT_VERSION: u32 = 1;
/// Largest dictionary file read. Thousands of words fit in well under 1 MB.
pub const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;

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

/// Whether `contents` is a file in our own format (not a plain word list).
fn is_handy_dictionary(contents: &str) -> bool {
    serde_json::from_str::<SharedFile>(contents).is_ok_and(|file| file.format == FORMAT)
}

pub fn has_json_extension(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
}

fn is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
}

/// Reads a dictionary file: never through a symbolic link, only a regular
/// file, and at most [`MAX_FILE_BYTES`].
pub fn read_dictionary_file(path: &Path) -> io::Result<String> {
    if is_symlink(path) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "refusing to follow a symbolic link",
        ));
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    // Closes the gap between the check above and the open: a link swapped
    // in meanwhile makes the open fail, and a FIFO cannot block it.
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
    let too_large = || io::Error::new(io::ErrorKind::InvalidData, "file too large");
    if meta.len() > MAX_FILE_BYTES {
        return Err(too_large());
    }
    let mut contents = String::new();
    file.take(MAX_FILE_BYTES + 1)
        .read_to_string(&mut contents)?;
    if contents.len() as u64 > MAX_FILE_BYTES {
        return Err(too_large());
    }
    Ok(contents)
}

/// Writes next to the destination and renames, so readers (and iCloud) never
/// see a half-written file. The temporary file gets a random name and is
/// created exclusively, so a file or link planted in a shared folder cannot
/// redirect the write; the rename replaces a link at `path`, never its target.
pub fn write_atomically(path: &Path, contents: &str) -> io::Result<()> {
    let dir = match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => Path::new("."),
    };
    let mut tmp = tempfile::Builder::new()
        .prefix(".handy-dictionary-")
        .suffix(".tmp")
        .tempfile_in(dir)?;
    tmp.write_all(contents.as_bytes())?;
    tmp.as_file().sync_all()?;
    // Readable by the other people sharing the folder, like a normal file.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tmp.as_file()
            .set_permissions(fs::Permissions::from_mode(0o644))?;
    }
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

/// Why an export was refused. The code is what the frontend translates.
#[derive(Debug, PartialEq, Eq)]
pub enum ExportError {
    NotJson,
    /// The destination exists and is not a Handy dictionary (or is a link).
    NotADictionary,
    WriteFailed(String),
}

impl std::fmt::Display for ExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotJson => f.write_str("not_json"),
            Self::NotADictionary => f.write_str("not_a_dictionary"),
            Self::WriteFailed(detail) => write!(f, "write_failed: {}", detail),
        }
    }
}

/// An export may only create a `.json` file or replace an earlier export.
fn check_export_target(path: &Path) -> Result<(), ExportError> {
    if !has_json_extension(path) {
        return Err(ExportError::NotJson);
    }
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(ExportError::WriteFailed(e.to_string())),
        Ok(meta) if !meta.file_type().is_file() => Err(ExportError::NotADictionary),
        Ok(_) => match read_dictionary_file(path) {
            Ok(contents) if is_handy_dictionary(&contents) => Ok(()),
            _ => Err(ExportError::NotADictionary),
        },
    }
}

pub fn export_to(
    path: &Path,
    entries: &[DictionaryEntry],
    author: &str,
    now: i64,
) -> Result<usize, ExportError> {
    check_export_target(path)?;
    let live = live_entries(entries);
    let count = live.len();
    let file = SharedFile::new(live, author, now);
    let json =
        serde_json::to_string_pretty(&file).map_err(|e| ExportError::WriteFailed(e.to_string()))?;
    write_atomically(path, &json).map_err(|e| ExportError::WriteFailed(e.to_string()))?;
    Ok(count)
}

pub fn import_from(
    path: &Path,
    local: &mut Vec<DictionaryEntry>,
    author: &str,
    now: i64,
) -> Result<MergeStats, SyncError> {
    if !has_json_extension(path) {
        return Err(SyncError::new(
            SyncErrorKind::Unreadable,
            "not a .json file",
        ));
    }
    let contents =
        read_dictionary_file(path).map_err(|e| SyncError::new(SyncErrorKind::Unreadable, e))?;
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

/// Conflict copies of the shared file left by the cloud provider. Only
/// regular files: a symbolic link is neither read nor removed.
fn conflict_copies(folder: &Path) -> Vec<PathBuf> {
    let Ok(dir) = fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut copies: Vec<PathBuf> = dir
        .flatten()
        // `DirEntry::file_type` describes the entry itself, not a link target.
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .map(|e| e.path())
        .filter(|p| {
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            name != SHARED_FILE_NAME && name.starts_with(FILE_STEM) && name.ends_with(".json")
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

    // A symbolic link in place of the shared file is ignored here and
    // replaced by a real file on write (the rename does not follow it).
    let shared_is_file = fs::symlink_metadata(&shared_path).is_ok_and(|m| m.file_type().is_file());
    if is_symlink(&shared_path) {
        log::warn!(
            "Ignoring symbolic link in place of {}",
            shared_path.display()
        );
    }

    let remote: Option<Vec<DictionaryEntry>> = if shared_is_file {
        let contents = read_dictionary_file(&shared_path)
            .map_err(|e| SyncError::new(SyncErrorKind::Unreadable, e))?;
        Some(
            parse_entries(&contents, author, now)
                .map_err(|e| SyncError::new(SyncErrorKind::Unreadable, e))?,
        )
    } else if fs::symlink_metadata(icloud_placeholder(folder, SHARED_FILE_NAME)).is_ok() {
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
        let parsed = read_dictionary_file(&copy)
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

    fn dictionary_json(words: &[&str], by: &str) -> String {
        let entries = words.iter().map(|w| word(w, by, 1)).collect();
        serde_json::to_string(&SharedFile::new(entries, by, 1)).unwrap()
    }

    fn leftover_temp_files(dir: &Path) -> Vec<String> {
        fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".tmp") || n.contains(".tmp-"))
            .collect()
    }

    #[test]
    fn atomic_write_leaves_no_temporary_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.json");
        write_atomically(&path, "first").unwrap();
        write_atomically(&path, "second").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "second");
        assert!(leftover_temp_files(dir.path()).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn planted_link_at_the_old_temporary_name_is_not_followed() {
        use std::os::unix::fs::symlink;
        let folder = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let victim = outside.path().join("victim.txt");
        fs::write(&victim, "precious").unwrap();
        // The name the previous version wrote through, predictable from the PID.
        let planted =
            folder
                .path()
                .join(format!(".{}.tmp-{}", SHARED_FILE_NAME, std::process::id()));
        symlink(&victim, &planted).unwrap();

        let mut local = vec![word("Handy", "Marius", 1)];
        let outcome = sync_with_folder(folder.path(), &mut local, "Marius", 2).unwrap();
        assert!(outcome.wrote);
        assert_eq!(fs::read_to_string(&victim).unwrap(), "precious");
    }

    #[cfg(unix)]
    #[test]
    fn shared_file_link_is_ignored_and_replaced_not_followed() {
        use std::os::unix::fs::symlink;
        let folder = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("elsewhere.json");
        let original = dictionary_json(&["Intrus"], "Mallory");
        fs::write(&target, &original).unwrap();
        let shared = folder.path().join(SHARED_FILE_NAME);
        symlink(&target, &shared).unwrap();

        let mut local = vec![word("Handy", "Marius", 1)];
        let outcome = sync_with_folder(folder.path(), &mut local, "Marius", 2).unwrap();
        assert_eq!(outcome.stats.added, 0, "the link's target is not read");
        assert_eq!(live_terms(&local), vec!["Handy"]);
        assert_eq!(fs::read_to_string(&target).unwrap(), original);
        let meta = fs::symlink_metadata(&shared).unwrap();
        assert!(meta.file_type().is_file(), "the link became a real file");
    }

    #[cfg(unix)]
    #[test]
    fn linked_conflict_copies_are_neither_read_nor_removed() {
        use std::os::unix::fs::symlink;
        let folder = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("private.json");
        fs::write(&target, dictionary_json(&["Secret"], "Mallory")).unwrap();
        let copy = folder.path().join("handy-dictionary 2.json");
        symlink(&target, &copy).unwrap();

        let mut local = vec![word("Handy", "Marius", 1)];
        let outcome = sync_with_folder(folder.path(), &mut local, "Marius", 2).unwrap();
        assert_eq!(outcome.stats.added, 0);
        assert_eq!(live_terms(&local), vec!["Handy"]);
        assert!(fs::symlink_metadata(&copy).is_ok(), "link left in place");
        assert!(target.exists());
    }

    #[test]
    fn oversized_shared_file_is_refused_and_left_untouched() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join(SHARED_FILE_NAME);
        let file = fs::File::create(&path).unwrap();
        file.set_len(MAX_FILE_BYTES + 1).unwrap();
        drop(file);

        let mut local = vec![word("Handy", "Marius", 1)];
        let err = sync_with_folder(folder.path(), &mut local, "Marius", 2).unwrap_err();
        assert_eq!(err.kind, SyncErrorKind::Unreadable);
        assert_eq!(fs::metadata(&path).unwrap().len(), MAX_FILE_BYTES + 1);
    }

    #[cfg(unix)]
    #[test]
    fn reading_refuses_links_and_non_files() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.json");
        fs::write(&real, "[]").unwrap();
        let link = dir.path().join("link.json");
        symlink(&real, &link).unwrap();
        assert_eq!(read_dictionary_file(&real).unwrap(), "[]");
        assert!(read_dictionary_file(&link).is_err());
        assert!(read_dictionary_file(dir.path()).is_err());
    }

    #[test]
    fn export_requires_a_json_file() {
        let dir = tempfile::tempdir().unwrap();
        let entries = [word("Handy", "Marius", 1)];
        for name in ["dictionary.txt", "dictionary", ".zshrc"] {
            let path = dir.path().join(name);
            assert_eq!(
                export_to(&path, &entries, "Marius", 2),
                Err(ExportError::NotJson)
            );
            assert!(!path.exists());
        }
        let upper = dir.path().join("DICTIONARY.JSON");
        assert_eq!(export_to(&upper, &entries, "Marius", 2), Ok(1));
    }

    #[test]
    fn export_never_overwrites_a_foreign_file() {
        let dir = tempfile::tempdir().unwrap();
        let entries = [word("Handy", "Marius", 1)];
        let foreign = [
            ("package.json", r#"{"name":"app"}"#),
            ("words.json", r#"["Handy"]"#),
            ("other.json", r#"{"format":"other","version":1}"#),
            ("broken.json", "{ not json"),
        ];
        for (name, contents) in foreign {
            let path = dir.path().join(name);
            fs::write(&path, contents).unwrap();
            assert_eq!(
                export_to(&path, &entries, "Marius", 2),
                Err(ExportError::NotADictionary),
                "{name}"
            );
            assert_eq!(fs::read_to_string(&path).unwrap(), contents);
        }
        let folder = dir.path().join("folder.json");
        fs::create_dir(&folder).unwrap();
        assert_eq!(
            export_to(&folder, &entries, "Marius", 2),
            Err(ExportError::NotADictionary)
        );
    }

    #[test]
    fn export_replaces_an_earlier_export() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("handy-dictionary.json");
        export_to(&path, &[word("Handy", "Marius", 1)], "Marius", 2).unwrap();
        let entries = [word("Handy", "Marius", 1), word("Yasmine", "Marius", 3)];
        assert_eq!(export_to(&path, &entries, "Marius", 4), Ok(2));
        let mut imported = Vec::new();
        import_from(&path, &mut imported, "Yasmine", 5).unwrap();
        assert_eq!(live_terms(&imported), vec!["Handy", "Yasmine"]);
    }

    #[cfg(unix)]
    #[test]
    fn export_refuses_to_write_through_a_link() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("earlier.json");
        let original = dictionary_json(&["Handy"], "Marius");
        fs::write(&target, &original).unwrap();
        let link = dir.path().join("link.json");
        symlink(&target, &link).unwrap();
        assert_eq!(
            export_to(&link, &[word("Other", "Marius", 1)], "Marius", 2),
            Err(ExportError::NotADictionary)
        );
        assert_eq!(fs::read_to_string(&target).unwrap(), original);
    }

    #[test]
    fn import_requires_a_json_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("words.txt");
        fs::write(&path, r#"["Handy"]"#).unwrap();
        let mut local = Vec::new();
        let err = import_from(&path, &mut local, "Marius", 1).unwrap_err();
        assert_eq!(err.kind, SyncErrorKind::Unreadable);
        assert!(local.is_empty());
    }

    #[test]
    fn plain_word_lists_can_be_imported() {
        let entries = parse_entries(r#"["Handy", "Yasmine"]"#, "Marius", 1).unwrap();
        assert_eq!(entries.len(), 2);
        assert!(parse_entries(r#"{"format":"other","version":1}"#, "M", 1).is_err());
    }
}
