//! The personal dictionary: vocabulary words and "heard → written"
//! replacements, with provenance, learnt from corrections and shareable
//! between Macs.
//!
//! `dictionary.json` in the app data dir is the source of truth. The parts the
//! transcription pipeline needs are mirrored into the settings
//! (`custom_words`, `dictionary_replacements`) whenever the dictionary
//! changes, so the pipeline keeps reading plain settings.

pub mod entries;
pub mod replace;
pub mod suggest;
pub mod sync;

use entries::{
    clean_term, entry_id, fold, live_entries, vocabulary, DictionaryEntry, EntryKind, MergeStats,
    MAX_TERM_CHARS,
};
use log::{debug, error, info, warn};
use serde::{Deserialize, Serialize};
use specta::Type;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use suggest::DictionarySuggestion;
use sync::SyncError;
use tauri::{AppHandle, Manager};
use tauri_specta::Event;

use crate::settings::{self, WordReplacement};

const FILE_NAME: &str = "dictionary.json";
/// How often the shared folder is checked while Handy runs.
const SYNC_INTERVAL: Duration = Duration::from_secs(60);
/// Recent dictations scanned for recurring names.
pub const RECURRING_SCAN_LIMIT: usize = 300;

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// What is persisted locally. Only `entries` is ever shared.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct LocalDictionary {
    #[serde(default)]
    author: String,
    #[serde(default)]
    sync_folder: Option<String>,
    #[serde(default)]
    last_sync_at: Option<i64>,
    #[serde(default)]
    last_sync_error: Option<SyncError>,
    /// Suggestion keys the user said no to.
    #[serde(default)]
    dismissed: Vec<String>,
    #[serde(default)]
    entries: Vec<DictionaryEntry>,
}

#[derive(Clone, Debug, Serialize, Type)]
pub struct SyncStatus {
    pub folder: Option<String>,
    pub last_sync_at: Option<i64>,
    pub error: Option<SyncError>,
}

/// Everything the dictionary page shows.
#[derive(Clone, Debug, Serialize, Type)]
pub struct DictionarySnapshot {
    /// Live entries, newest first.
    pub entries: Vec<DictionaryEntry>,
    /// The name stamped on entries added on this Mac.
    pub author: String,
    pub sync: SyncStatus,
}

#[derive(Clone, Debug, Serialize, Type)]
pub struct ImportSummary {
    pub stats: MergeStats,
    pub snapshot: DictionarySnapshot,
}

/// Emitted whenever the dictionary changes, including from a background sync.
#[derive(Clone, Debug, Serialize, Deserialize, Type, Event)]
pub struct DictionaryChangedEvent {
    /// Entries a sync just brought in from the shared folder.
    pub received: MergeStats,
}

pub struct DictionaryManager {
    app: AppHandle,
    path: PathBuf,
    state: Mutex<LocalDictionary>,
    /// One sync at a time; a sync requested meanwhile just waits.
    sync_lock: Mutex<()>,
}

/// First name of the macOS account, so entries read "Marius" out of the box.
fn default_author() -> String {
    #[cfg(target_os = "macos")]
    {
        if let Ok(out) = std::process::Command::new("id").arg("-F").output() {
            let full = String::from_utf8_lossy(&out.stdout);
            if let Some(first) = full.split_whitespace().next() {
                return first.to_string();
            }
        }
    }
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .ok()
        .and_then(|u| {
            let mut chars = u.chars();
            chars
                .next()
                .map(|c| c.to_uppercase().chain(chars).collect::<String>())
        })
        .unwrap_or_else(|| "Handy".to_string())
}

fn validate_term(term: &str) -> Result<String, String> {
    let term = clean_term(term);
    if term.is_empty() {
        return Err("empty".into());
    }
    if term.chars().count() > MAX_TERM_CHARS {
        return Err("too_long".into());
    }
    Ok(term)
}

impl DictionaryManager {
    pub fn new(app: &AppHandle) -> Result<Self, String> {
        let dir = crate::portable::app_data_dir(app).map_err(|e| e.to_string())?;
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = dir.join(FILE_NAME);
        let mut local = Self::load(&path);
        if local.author.trim().is_empty() {
            local.author = default_author();
        }

        // Words that only exist in the settings (first launch with this
        // feature, or edited by an older build) join the dictionary.
        let settings = settings::get_settings(app);
        let known: HashSet<String> = local.entries.iter().map(|e| e.id.clone()).collect();
        let base = now_ms() - settings.custom_words.len() as i64;
        let mut adopted = 0;
        for (i, word) in settings.custom_words.iter().enumerate() {
            let Ok(word) = validate_term(word) else {
                continue;
            };
            let targets_known = local
                .entries
                .iter()
                .any(|e| !e.deleted && fold(e.written_form()) == fold(&word));
            if !known.contains(&entry_id(EntryKind::Word, &word)) && !targets_known {
                local.entries.push(DictionaryEntry::new(
                    EntryKind::Word,
                    &word,
                    "",
                    &local.author,
                    base + i as i64,
                ));
                adopted += 1;
            }
        }
        if adopted > 0 {
            info!(
                "Dictionary adopted {} custom word(s) from settings",
                adopted
            );
        }

        let manager = Self {
            app: app.clone(),
            path,
            state: Mutex::new(local),
            sync_lock: Mutex::new(()),
        };
        {
            let state = manager.lock();
            manager.persist(&state);
        }
        Ok(manager)
    }

    fn load(path: &Path) -> LocalDictionary {
        let Ok(contents) = std::fs::read_to_string(path) else {
            return LocalDictionary::default();
        };
        match serde_json::from_str(&contents) {
            Ok(local) => local,
            Err(e) => {
                // Keep the damaged file for recovery rather than overwrite it.
                let backup = path.with_extension(format!("json.broken-{}", now_ms()));
                error!(
                    "Dictionary file unreadable ({}); kept as {}",
                    e,
                    backup.display()
                );
                let _ = std::fs::rename(path, &backup);
                LocalDictionary::default()
            }
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, LocalDictionary> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Saves the file and mirrors what the pipeline needs into the settings.
    fn persist(&self, state: &LocalDictionary) {
        match serde_json::to_string_pretty(state) {
            Ok(json) => {
                if let Err(e) = sync::write_atomically(&self.path, &json) {
                    error!("Failed to save dictionary: {}", e);
                }
            }
            Err(e) => error!("Failed to serialize dictionary: {}", e),
        }

        let words = vocabulary(&state.entries);
        let replacements: Vec<WordReplacement> = live_entries(&state.entries)
            .into_iter()
            .filter(|e| e.kind == EntryKind::Replacement)
            .map(|e| WordReplacement {
                from: e.from,
                to: e.to,
            })
            .collect();
        let mut app_settings = settings::get_settings(&self.app);
        if app_settings.custom_words != words
            || app_settings.dictionary_replacements != replacements
        {
            app_settings.custom_words = words;
            app_settings.dictionary_replacements = replacements;
            settings::write_settings(&self.app, app_settings);
        }
    }

    fn snapshot_of(state: &LocalDictionary) -> DictionarySnapshot {
        DictionarySnapshot {
            entries: live_entries(&state.entries),
            author: state.author.clone(),
            sync: SyncStatus {
                folder: state.sync_folder.clone(),
                last_sync_at: state.last_sync_at,
                error: state.last_sync_error.clone(),
            },
        }
    }

    fn emit_changed(&self, received: MergeStats) {
        if let Err(e) = (DictionaryChangedEvent { received }).emit(&self.app) {
            warn!("Failed to emit dictionary change: {}", e);
        }
    }

    pub fn snapshot(&self) -> DictionarySnapshot {
        Self::snapshot_of(&self.lock())
    }

    /// Applies a local edit, saves, notifies and pushes it to the shared folder.
    fn mutate<T>(
        self: &Arc<Self>,
        edit: impl FnOnce(&mut LocalDictionary) -> Result<T, String>,
    ) -> Result<DictionarySnapshot, String> {
        let snapshot = {
            let mut state = self.lock();
            edit(&mut state)?;
            self.persist(&state);
            Self::snapshot_of(&state)
        };
        self.emit_changed(MergeStats::default());
        self.request_sync();
        Ok(snapshot)
    }

    /// Adds a word (`to` empty) or a replacement. Re-adding an existing
    /// replacement with a new target updates it.
    pub fn add(self: &Arc<Self>, from: &str, to: &str) -> Result<DictionarySnapshot, String> {
        let from = validate_term(from)?;
        let to = clean_term(to);
        let kind = if to.is_empty() || to == from {
            EntryKind::Word
        } else {
            validate_term(&to)?;
            EntryKind::Replacement
        };
        self.mutate(|state| {
            let id = entry_id(kind, &from);
            let now = now_ms();
            let author = state.author.clone();
            match state.entries.iter_mut().find(|e| e.id == id) {
                Some(existing) if !existing.deleted && existing.to == to => {
                    Err("duplicate".to_string())
                }
                Some(existing) if !existing.deleted => {
                    existing.to = to.clone();
                    existing.updated_at = now;
                    Ok(())
                }
                Some(existing) => {
                    *existing = DictionaryEntry::new(kind, &from, &to, &author, now);
                    Ok(())
                }
                None => {
                    state
                        .entries
                        .push(DictionaryEntry::new(kind, &from, &to, &author, now));
                    Ok(())
                }
            }
        })
    }

    pub fn remove(self: &Arc<Self>, id: &str) -> Result<DictionarySnapshot, String> {
        self.mutate(|state| {
            let entry = state
                .entries
                .iter_mut()
                .find(|e| e.id == id && !e.deleted)
                .ok_or_else(|| "not_found".to_string())?;
            entry.deleted = true;
            entry.updated_at = now_ms();
            Ok(())
        })
    }

    /// Replaces the live words with `words` (the legacy custom-words command).
    pub fn set_words(self: &Arc<Self>, words: &[String]) -> Result<(), String> {
        let wanted: Vec<String> = words.iter().filter_map(|w| validate_term(w).ok()).collect();
        let wanted_ids: HashSet<String> = wanted
            .iter()
            .map(|w| entry_id(EntryKind::Word, w))
            .collect();
        self.mutate(|state| {
            let now = now_ms();
            let author = state.author.clone();
            for entry in state.entries.iter_mut() {
                if entry.kind == EntryKind::Word
                    && !entry.deleted
                    && !wanted_ids.contains(&entry.id)
                {
                    entry.deleted = true;
                    entry.updated_at = now;
                }
            }
            for word in &wanted {
                let id = entry_id(EntryKind::Word, word);
                match state.entries.iter_mut().find(|e| e.id == id) {
                    Some(e) if !e.deleted => {}
                    Some(e) => *e = DictionaryEntry::new(EntryKind::Word, word, "", &author, now),
                    None => state.entries.push(DictionaryEntry::new(
                        EntryKind::Word,
                        word,
                        "",
                        &author,
                        now,
                    )),
                }
            }
            Ok(())
        })
        .map(|_| ())
    }

    pub fn set_author(self: &Arc<Self>, name: &str) -> Result<DictionarySnapshot, String> {
        let name: String = clean_term(name).chars().take(40).collect();
        if name.is_empty() {
            return Err("empty".into());
        }
        self.mutate(|state| {
            state.author = name;
            Ok(())
        })
    }

    pub fn dismiss(self: &Arc<Self>, key: &str) -> Result<(), String> {
        let mut state = self.lock();
        if !state.dismissed.iter().any(|k| k == key) {
            state.dismissed.push(key.to_string());
            self.persist(&state);
        }
        Ok(())
    }

    /// Drops what is already in the dictionary or was dismissed.
    fn still_relevant(
        state: &LocalDictionary,
        suggestions: Vec<DictionarySuggestion>,
    ) -> Vec<DictionarySuggestion> {
        suggestions
            .into_iter()
            .filter(|s| !state.dismissed.contains(&s.key))
            .filter(|s| {
                !state.entries.iter().any(|e| {
                    !e.deleted
                        && e.kind == s.kind
                        && fold(&e.from) == fold(&s.from)
                        && fold(&e.to) == fold(&s.to)
                })
            })
            .collect()
    }

    pub fn suggestions_for_edit(&self, original: &str, edited: &str) -> Vec<DictionarySuggestion> {
        let suggestions = suggest::suggestions_from_edit(original, edited);
        Self::still_relevant(&self.lock(), suggestions)
    }

    pub fn recurring_suggestions(&self, texts: &[&str]) -> Vec<DictionarySuggestion> {
        let state = self.lock();
        let mut known: HashSet<String> = HashSet::new();
        for e in state.entries.iter().filter(|e| !e.deleted) {
            known.insert(fold(&e.from));
            known.insert(fold(e.written_form()));
        }
        for key in &state.dismissed {
            if let Some(word) = key.strip_prefix("word:") {
                known.insert(word.to_string());
            }
        }
        suggest::recurring_names(texts, &known, 3, 2, 5)
    }

    pub fn export(&self, path: &Path) -> Result<usize, String> {
        let state = self.lock();
        sync::export_to(path, &state.entries, &state.author, now_ms()).map_err(|e| e.to_string())
    }

    pub fn import(self: &Arc<Self>, path: &Path) -> Result<ImportSummary, SyncError> {
        let stats = {
            let mut state = self.lock();
            let author = state.author.clone();
            let stats = sync::import_from(path, &mut state.entries, &author, now_ms())?;
            self.persist(&state);
            stats
        };
        self.emit_changed(MergeStats::default());
        self.request_sync();
        Ok(ImportSummary {
            stats,
            snapshot: self.snapshot(),
        })
    }

    pub fn set_sync_folder(self: &Arc<Self>, folder: Option<String>) -> DictionarySnapshot {
        {
            let mut state = self.lock();
            state.sync_folder = folder.filter(|f| !f.trim().is_empty());
            state.last_sync_at = None;
            state.last_sync_error = None;
            self.persist(&state);
        }
        self.sync_now()
    }

    /// One sync round, now. Returns the resulting state either way; failures
    /// are recorded in it rather than raised.
    pub fn sync_now(&self) -> DictionarySnapshot {
        let _guard = self.sync_lock.lock().unwrap_or_else(|p| p.into_inner());
        let (folder, mut entries, author) = {
            let state = self.lock();
            match &state.sync_folder {
                None => return Self::snapshot_of(&state),
                Some(folder) => (
                    PathBuf::from(folder),
                    state.entries.clone(),
                    state.author.clone(),
                ),
            }
        };

        // File I/O happens without holding the state lock; the result is
        // merged back so edits made meanwhile are not lost.
        let result = sync::sync_with_folder(&folder, &mut entries, &author, now_ms());

        let (snapshot, received) = {
            let mut state = self.lock();
            let received = match result {
                Ok(outcome) => {
                    if outcome.stats.changed() {
                        info!("Dictionary sync received {:?}", outcome.stats);
                    }
                    debug!("Dictionary sync wrote shared file: {}", outcome.wrote);
                    entries::merge_entries(&mut state.entries, &entries, entries::MergeMode::Sync);
                    state.last_sync_at = Some(now_ms());
                    state.last_sync_error = None;
                    outcome.stats
                }
                Err(e) => {
                    if state.last_sync_error.as_ref() != Some(&e) {
                        warn!("Dictionary sync failed: {:?} ({})", e.kind, e.detail);
                    }
                    state.last_sync_error = Some(e);
                    MergeStats::default()
                }
            };
            self.persist(&state);
            (Self::snapshot_of(&state), received)
        };
        self.emit_changed(received);
        snapshot
    }

    /// Syncs in the background, if a shared folder is set.
    pub fn request_sync(self: &Arc<Self>) {
        if self.lock().sync_folder.is_none() {
            return;
        }
        let manager = Arc::clone(self);
        std::thread::spawn(move || {
            manager.sync_now();
        });
    }

    /// Periodic sync while the app runs (no-op without a shared folder).
    pub fn start_background_sync(self: &Arc<Self>) {
        let manager = Arc::clone(self);
        std::thread::Builder::new()
            .name("dictionary-sync".into())
            .spawn(move || loop {
                if manager.lock().sync_folder.is_some() {
                    manager.sync_now();
                }
                std::thread::sleep(SYNC_INTERVAL);
            })
            .map(|_| ())
            .unwrap_or_else(|e| error!("Failed to start dictionary sync: {}", e));
    }
}

/// The manager from Tauri state, for callers outside commands.
pub fn manager(app: &AppHandle) -> Option<Arc<DictionaryManager>> {
    app.try_state::<Arc<DictionaryManager>>()
        .map(|s| Arc::clone(s.inner()))
}
