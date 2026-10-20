//! Tauri commands backing the Notes page.

use super::store::{DestinationState, VoiceNote};
use super::{
    apple_notes, default_markdown_folder, manager, markdown, markdown_folder, notify, notion,
    secrets, text, VoiceNotesSettings, LIST_LIMIT, VOICE_NOTE_BINDING,
};
use crate::settings::{get_settings, write_settings};
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::AppHandle;
use tauri_plugin_opener::OpenerExt;

/// Everything the Notes page needs to render its settings, besides the
/// persisted `voice_notes` settings themselves.
#[derive(Serialize, Deserialize, Debug, Clone, Type)]
pub struct VoiceNotesStatus {
    /// Label of the LLM provider used for titles/cleanup, if one is set up.
    pub llm_provider: Option<String>,
    /// Folder used when no custom folder is chosen.
    pub default_folder: Option<String>,
    /// Folder notes are written to right now.
    pub effective_folder: Option<String>,
    pub apple_notes_supported: bool,
}

#[tauri::command]
#[specta::specta]
pub fn get_voice_notes(app: AppHandle) -> Result<Vec<VoiceNote>, String> {
    let manager = manager(&app).ok_or("not_ready")?;
    manager.store().list(LIST_LIMIT).map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub fn get_voice_notes_status(app: AppHandle) -> VoiceNotesStatus {
    let settings = get_settings(&app);
    VoiceNotesStatus {
        llm_provider: super::llm::configured_provider(&settings).map(|(p, _)| p.label),
        default_folder: default_markdown_folder(&app).map(|p| p.to_string_lossy().into_owned()),
        effective_folder: markdown_folder(&app, &settings.voice_notes)
            .map(|p| p.to_string_lossy().into_owned()),
        apple_notes_supported: cfg!(target_os = "macos"),
    }
}

/// Save the user-editable part of the voice notes settings. Notion fields
/// are owned by `connect_notion` / `disconnect_notion` and preserved here.
#[tauri::command]
#[specta::specta]
pub fn update_voice_notes_settings(
    app: AppHandle,
    voice_notes: VoiceNotesSettings,
) -> Result<(), String> {
    let mut settings = get_settings(&app);
    let was_enabled = settings.voice_notes.enabled;
    let previous = settings.voice_notes.clone();

    settings.voice_notes = VoiceNotesSettings {
        notion_connected: previous.notion_connected,
        notion_parent_page_id: previous.notion_parent_page_id,
        notion_parent_title: previous.notion_parent_title,
        markdown_folder: voice_notes.markdown_folder.filter(|p| !p.trim().is_empty()),
        ..voice_notes
    };
    let now_enabled = settings.voice_notes.enabled;
    let binding = settings.bindings.get(VOICE_NOTE_BINDING).cloned();
    write_settings(&app, settings);

    if was_enabled != now_enabled {
        if let Some(binding) = binding {
            let result = if now_enabled {
                crate::shortcut::register_shortcut(&app, binding)
            } else {
                crate::shortcut::unregister_shortcut(&app, binding)
            };
            if let Err(e) = result {
                log::warn!("Voice note shortcut (un)registration failed: {e}");
            }
        }
        crate::secure_input::reconcile_fallback(&app);
    }
    Ok(())
}

/// Outcome of a delete for the Markdown copy, so the UI can tell the user
/// when a hand-edited journal was left untouched.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Type)]
#[serde(rename_all = "snake_case")]
pub enum MarkdownDeleteOutcome {
    None,
    Removed,
    LeftUntouched,
    Failed,
}

#[tauri::command]
#[specta::specta]
pub fn delete_voice_note(app: AppHandle, id: i64) -> Result<MarkdownDeleteOutcome, String> {
    let manager = manager(&app).ok_or("not_ready")?;
    let Some(note) = manager.store().get(id).map_err(|e| e.to_string())? else {
        return Ok(MarkdownDeleteOutcome::None);
    };

    let outcome = match note.markdown.location() {
        None => MarkdownDeleteOutcome::None,
        Some(path) => match markdown::remove_note(std::path::Path::new(path), &note.uid()) {
            Ok(markdown::RemoveOutcome::Removed | markdown::RemoveOutcome::Missing) => {
                MarkdownDeleteOutcome::Removed
            }
            Ok(markdown::RemoveOutcome::LeftUntouched) => MarkdownDeleteOutcome::LeftUntouched,
            Err(e) => {
                log::warn!("Voice note {id}: could not remove Markdown copy: {e}");
                MarkdownDeleteOutcome::Failed
            }
        },
    };

    manager.store().delete(id).map_err(|e| e.to_string())?;
    notify(&app, id);
    Ok(outcome)
}

/// Open a note where it lives: its Markdown file, else its Notion page,
/// else Apple Notes.
#[tauri::command]
#[specta::specta]
pub fn open_voice_note(app: AppHandle, id: i64) -> Result<(), String> {
    let manager = manager(&app).ok_or("not_ready")?;
    let note = manager
        .store()
        .get(id)
        .map_err(|e| e.to_string())?
        .ok_or("not_found")?;

    if let Some(path) = note.markdown.location() {
        if std::path::Path::new(path).exists() {
            return app
                .opener()
                .open_path(path, None::<String>)
                .map_err(|e| e.to_string());
        }
    }
    if let Some(url) = note.notion.location().filter(|u| !u.is_empty()) {
        return app
            .opener()
            .open_url(url, None::<String>)
            .map_err(|e| e.to_string());
    }
    if let Some(apple_id) = note.apple_notes.location() {
        return apple_notes::show_note(apple_id);
    }
    Err("not_found".into())
}

/// Open a specific destination of a note (used by the destination chips).
#[tauri::command]
#[specta::specta]
pub fn open_voice_note_destination(
    app: AppHandle,
    id: i64,
    destination: String,
) -> Result<(), String> {
    let manager = manager(&app).ok_or("not_ready")?;
    let note = manager
        .store()
        .get(id)
        .map_err(|e| e.to_string())?
        .ok_or("not_found")?;
    let state = match destination.as_str() {
        "markdown" => &note.markdown,
        "notion" => &note.notion,
        "apple_notes" => &note.apple_notes,
        _ => return Err("unknown_destination".into()),
    };
    let DestinationState::Done { location } = state else {
        return Err("not_found".into());
    };
    match destination.as_str() {
        "markdown" => app
            .opener()
            .reveal_item_in_dir(location)
            .map_err(|e| e.to_string()),
        "notion" => app
            .opener()
            .open_url(location, None::<String>)
            .map_err(|e| e.to_string()),
        _ => apple_notes::show_note(location),
    }
}

#[tauri::command]
#[specta::specta]
pub fn retry_voice_note_exports(app: AppHandle, id: i64) -> Result<(), String> {
    super::retry_exports(&app, id)
}

#[tauri::command]
#[specta::specta]
pub fn open_voice_notes_folder(app: AppHandle) -> Result<(), String> {
    let settings = get_settings(&app);
    let folder = markdown_folder(&app, &settings.voice_notes).ok_or("folder_unavailable")?;
    std::fs::create_dir_all(&folder).map_err(|e| match e.kind() {
        std::io::ErrorKind::PermissionDenied => markdown::ERR_PERMISSION.to_string(),
        _ => e.to_string(),
    })?;
    app.opener()
        .open_path(folder.to_string_lossy(), None::<String>)
        .map_err(|e| e.to_string())
}

#[derive(Serialize, Deserialize, Debug, Clone, Type)]
pub struct NotionConnection {
    pub page_title: Option<String>,
}

/// Verify the token against the parent page, then store the token in the
/// keychain and remember the page. Errors are stable codes the UI
/// translates (`notion_invalid_token`, `notion_page_not_found`, …).
#[tauri::command]
#[specta::specta]
pub async fn connect_notion(
    app: AppHandle,
    token: String,
    page: String,
) -> Result<NotionConnection, String> {
    let token = token.trim().to_string();
    if token.is_empty() {
        return Err(notion::ERR_INVALID_TOKEN.into());
    }
    let page_id = text::notion_page_id(&page).ok_or("notion_invalid_page_link")?;
    let page_title = notion::verify_parent(&token, &page_id).await?;
    secrets::store_token(&app, &token).map_err(|e| format!("notion_keychain: {e}"))?;

    let mut settings = get_settings(&app);
    settings.voice_notes.notion_connected = true;
    settings.voice_notes.notion_parent_page_id = Some(page_id);
    settings.voice_notes.notion_parent_title = page_title.clone();
    write_settings(&app, settings);
    Ok(NotionConnection { page_title })
}

#[tauri::command]
#[specta::specta]
pub fn disconnect_notion(app: AppHandle) -> Result<(), String> {
    secrets::delete_token(&app);
    let mut settings = get_settings(&app);
    settings.voice_notes.notion_connected = false;
    settings.voice_notes.notion_parent_page_id = None;
    settings.voice_notes.notion_parent_title = None;
    write_settings(&app, settings);
    Ok(())
}

/// Ask macOS for Automation access to Notes (shows the system prompt the
/// first time). Errors: `apple_notes_denied`, `apple_notes_failed: …`.
#[tauri::command]
#[specta::specta]
pub async fn check_apple_notes_access() -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(apple_notes::check_access)
        .await
        .map_err(|e| e.to_string())?
}

/// Open the macOS privacy pane a voice-note destination needs:
/// `automation` (Apple Notes) or `files` (Markdown folder).
#[tauri::command]
#[specta::specta]
pub fn open_voice_notes_privacy_settings(app: AppHandle, pane: String) -> Result<(), String> {
    let anchor = match pane.as_str() {
        "automation" => "Privacy_Automation",
        "files" => "Privacy_FilesAndFolders",
        _ => return Err("unknown_pane".into()),
    };
    if !cfg!(target_os = "macos") {
        return Err("unsupported".into());
    }
    app.opener()
        .open_url(
            format!("x-apple.systempreferences:com.apple.preference.security?{anchor}"),
            None::<String>,
        )
        .map_err(|e| e.to_string())
}
