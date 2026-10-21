use crate::dictionary::suggest::DictionarySuggestion;
use crate::dictionary::sync::{SyncError, SyncErrorKind, SHARED_FILE_NAME};
use crate::dictionary::{
    DictionaryManager, DictionarySnapshot, ImportSummary, RECURRING_SCAN_LIMIT,
};
use crate::managers::history::HistoryManager;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{State, WebviewWindow, Wry};
use tauri_plugin_dialog::{DialogExt, FileDialogBuilder, FilePath};

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

/// Shows a native file dialog and returns the path the user picked, or
/// `None` if they cancelled.
///
/// Every path these commands touch comes from a dialog opened here, never
/// from the webview, so a compromised page cannot read or write arbitrary
/// files through them.
async fn pick_path<F>(window: WebviewWindow, show: F) -> Result<Option<PathBuf>, String>
where
    F: FnOnce(FileDialogBuilder<Wry>) -> Option<FilePath> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(move || {
        let builder = window.dialog().file().set_parent(&window);
        show(builder).map(FilePath::into_path).transpose()
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())
}

/// Lets the user pick the shared folder, then syncs with it. `None` if the
/// dialog was cancelled. `title` is the dialog's (translated) title.
#[tauri::command]
#[specta::specta]
pub async fn choose_dictionary_sync_folder(
    window: WebviewWindow,
    dictionary: Dictionary<'_>,
    title: String,
) -> Result<Option<DictionarySnapshot>, String> {
    let picked = pick_path(window, move |dialog| {
        dialog
            .set_title(title)
            .set_can_create_directories(true)
            .blocking_pick_folder()
    })
    .await?;
    let Some(folder) = picked else {
        return Ok(None);
    };
    let dictionary = Arc::clone(dictionary.inner());
    let folder = folder.to_string_lossy().into_owned();
    tauri::async_runtime::spawn_blocking(move || dictionary.set_sync_folder(Some(folder)))
        .await
        .map(Some)
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn stop_dictionary_sync(
    dictionary: Dictionary<'_>,
) -> Result<DictionarySnapshot, String> {
    let dictionary = Arc::clone(dictionary.inner());
    tauri::async_runtime::spawn_blocking(move || dictionary.set_sync_folder(None))
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

/// Asks where to save, then writes the live entries there. Returns how many
/// were exported, or `None` if the dialog was cancelled. Errors are codes the
/// UI translates: `not_json`, `not_a_dictionary` (refusing to replace a file
/// that is not an earlier export), otherwise `write_failed: …`.
#[tauri::command]
#[specta::specta]
pub async fn export_dictionary(
    window: WebviewWindow,
    dictionary: Dictionary<'_>,
    filter_name: String,
) -> Result<Option<u32>, String> {
    let picked = pick_path(window, move |dialog| {
        dialog
            .add_filter(filter_name, &["json"])
            .set_file_name(SHARED_FILE_NAME)
            .blocking_save_file()
    })
    .await?;
    let Some(path) = picked else {
        return Ok(None);
    };
    let dictionary = Arc::clone(dictionary.inner());
    tauri::async_runtime::spawn_blocking(move || dictionary.export(&path))
        .await
        .map_err(|e| e.to_string())?
        .map(|count| Some(count as u32))
}

/// Asks for a dictionary file and merges it into this one. Never removes
/// anything. `None` if the dialog was cancelled.
#[tauri::command]
#[specta::specta]
pub async fn import_dictionary(
    window: WebviewWindow,
    dictionary: Dictionary<'_>,
    filter_name: String,
) -> Result<Option<ImportSummary>, SyncError> {
    let unreadable = |e: String| SyncError {
        kind: SyncErrorKind::Unreadable,
        detail: e,
    };
    let picked = pick_path(window, move |dialog| {
        dialog
            .add_filter(filter_name, &["json"])
            .blocking_pick_file()
    })
    .await
    .map_err(unreadable)?;
    let Some(path) = picked else {
        return Ok(None);
    };
    let dictionary = Arc::clone(dictionary.inner());
    tauri::async_runtime::spawn_blocking(move || dictionary.import(&path))
        .await
        .map_err(|e| unreadable(e.to_string()))?
        .map(Some)
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
