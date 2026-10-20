use crate::dictionary::suggest::DictionarySuggestion;
use crate::dictionary::sync::SyncError;
use crate::dictionary::{
    DictionaryManager, DictionarySnapshot, ImportSummary, RECURRING_SCAN_LIMIT,
};
use crate::managers::history::HistoryManager;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::State;

type Dictionary<'a> = State<'a, Arc<DictionaryManager>>;

#[tauri::command]
#[specta::specta]
pub fn get_dictionary(dictionary: Dictionary<'_>) -> DictionarySnapshot {
    dictionary.snapshot()
}

/// Adds a vocabulary word (`to` empty) or a "heard → written" replacement.
/// Errors are codes the UI translates: `empty`, `too_long`, `duplicate`.
#[tauri::command]
#[specta::specta]
pub fn add_dictionary_entry(
    dictionary: Dictionary<'_>,
    from: String,
    to: Option<String>,
) -> Result<DictionarySnapshot, String> {
    dictionary.add(&from, to.as_deref().unwrap_or(""))
}

#[tauri::command]
#[specta::specta]
pub fn remove_dictionary_entry(
    dictionary: Dictionary<'_>,
    id: String,
) -> Result<DictionarySnapshot, String> {
    dictionary.remove(&id)
}

#[tauri::command]
#[specta::specta]
pub fn set_dictionary_author(
    dictionary: Dictionary<'_>,
    name: String,
) -> Result<DictionarySnapshot, String> {
    dictionary.set_author(&name)
}

#[tauri::command]
#[specta::specta]
pub async fn set_dictionary_sync_folder(
    dictionary: Dictionary<'_>,
    folder: Option<String>,
) -> Result<DictionarySnapshot, String> {
    let dictionary = Arc::clone(dictionary.inner());
    tauri::async_runtime::spawn_blocking(move || dictionary.set_sync_folder(folder))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn sync_dictionary_now(dictionary: Dictionary<'_>) -> Result<DictionarySnapshot, String> {
    let dictionary = Arc::clone(dictionary.inner());
    tauri::async_runtime::spawn_blocking(move || dictionary.sync_now())
        .await
        .map_err(|e| e.to_string())
}

/// Writes the live entries to `path`; returns how many were exported.
#[tauri::command]
#[specta::specta]
pub fn export_dictionary(dictionary: Dictionary<'_>, path: String) -> Result<u32, String> {
    dictionary
        .export(&PathBuf::from(path))
        .map(|count| count as u32)
}

/// Merges a dictionary file into this one. Never removes anything.
#[tauri::command]
#[specta::specta]
pub fn import_dictionary(
    dictionary: Dictionary<'_>,
    path: String,
) -> Result<ImportSummary, SyncError> {
    dictionary.import(&PathBuf::from(path))
}

/// Proper nouns that keep coming back in recent dictations.
#[tauri::command]
#[specta::specta]
pub async fn get_dictionary_suggestions(
    dictionary: Dictionary<'_>,
    history_manager: State<'_, Arc<HistoryManager>>,
) -> Result<Vec<DictionarySuggestion>, String> {
    let history = Arc::clone(history_manager.inner());
    let texts = tauri::async_runtime::spawn_blocking(move || {
        history.recent_transcriptions(RECURRING_SCAN_LIMIT)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;
    let texts: Vec<&str> = texts.iter().map(String::as_str).collect();
    Ok(dictionary.recurring_suggestions(&texts))
}

#[tauri::command]
#[specta::specta]
pub fn dismiss_dictionary_suggestion(
    dictionary: Dictionary<'_>,
    key: String,
) -> Result<(), String> {
    dictionary.dismiss(&key)
}
