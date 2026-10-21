//! Voice notes: a dedicated shortcut records a thought that is *not* pasted
//! into the active app but saved as a note — in Handy's own list, and
//! optionally in a Markdown folder, Notion and Apple Notes.
//!
//! Flow: shortcut → record → transcribe (shared pipeline in `actions.rs`) →
//! [`capture`] (title + cleanup, local save, overlay confirmation) → exports
//! to Notion / Apple Notes in the background.

pub mod apple_notes;
pub mod commands;
mod llm;
pub mod markdown;
pub mod notion;
mod secrets;
pub mod settings;
pub mod store;
pub mod text;

pub use settings::VoiceNotesSettings;

use crate::settings::get_settings;
use anyhow::Result;
use chrono::{Local, TimeZone};
use log::{debug, error, info, warn};
use serde::{Deserialize, Serialize};
use specta::Type;
use std::path::PathBuf;
use std::sync::Arc;
use store::{Destination, DestinationState, NewNote, NotesStore, VoiceNote};
use tauri::{AppHandle, Manager};
use tauri_specta::Event;
use text::NoteDraft;

/// Shortcut binding id for "record a voice note".
pub const VOICE_NOTE_BINDING: &str = "voice_note";

/// How many notes the Notes page loads. The Markdown folder is the archive.
const LIST_LIMIT: usize = 2000;

/// Emitted whenever a note is added, updated (export finished) or deleted,
/// so the Notes page can refresh.
#[derive(Clone, Debug, Serialize, Deserialize, Type, tauri_specta::Event)]
pub struct VoiceNotesChanged {
    pub id: i64,
}

/// True when the voice-note binding should be registered right now.
pub fn binding_enabled(settings: &crate::settings::AppSettings) -> bool {
    settings.voice_notes.enabled
}

pub struct NotesManager {
    store: NotesStore,
}

impl NotesManager {
    pub fn new(app: &AppHandle) -> Result<Self> {
        let dir = crate::portable::app_data_dir(app)?;
        std::fs::create_dir_all(&dir)?;
        Ok(Self {
            store: NotesStore::open(&dir.join("notes.db"))?,
        })
    }

    pub fn store(&self) -> &NotesStore {
        &self.store
    }
}

fn manager(app: &AppHandle) -> Option<Arc<NotesManager>> {
    app.try_state::<Arc<NotesManager>>().map(|s| Arc::clone(&s))
}

fn notify(app: &AppHandle, id: i64) {
    if let Err(e) = (VoiceNotesChanged { id }).emit(app) {
        warn!("Failed to emit voice note change: {e}");
    }
}

/// `~/Documents/Handy Notes`.
pub fn default_markdown_folder(app: &AppHandle) -> Option<PathBuf> {
    app.path()
        .document_dir()
        .ok()
        .map(|d| d.join("Handy Notes"))
}

pub fn markdown_folder(app: &AppHandle, settings: &VoiceNotesSettings) -> Option<PathBuf> {
    settings
        .markdown_folder
        .as_ref()
        .filter(|p| !p.trim().is_empty())
        .map(PathBuf::from)
        .or_else(|| default_markdown_folder(app))
}

/// Result of turning a transcript into a stored note.
pub enum CaptureOutcome {
    Saved,
    /// Nothing was said; no note was created.
    Empty,
    Failed,
}

/// Title + cleanup step. Runs the LLM when the user enabled it and one is
/// configured; otherwise (or on any failure) keeps the transcript verbatim.
pub async fn prepare_draft(app: &AppHandle, raw: &str) -> (NoteDraft, bool) {
    let settings = get_settings(app);
    if settings.voice_notes.ai_cleanup {
        if let Some(draft) = llm::draft_with_llm(&settings, raw).await {
            return (draft, true);
        }
    }
    (text::plain_draft(raw), false)
}

/// Whether [`prepare_draft`] will call an LLM, so the overlay can show a
/// "processing" state only when there is something to wait for.
pub fn will_use_llm(app: &AppHandle) -> bool {
    let settings = get_settings(app);
    settings.voice_notes.ai_cleanup && llm::configured_provider(&settings).is_some()
}

/// Store the note locally (database + Markdown), then export to the remote
/// destinations in the background. Local storage is what the overlay
/// confirms; a failed remote export is surfaced on the Notes page with a
/// retry button instead of losing the note.
pub fn save(app: &AppHandle, raw: &str, draft: NoteDraft, ai_processed: bool) -> CaptureOutcome {
    if draft.body.trim().is_empty() {
        return CaptureOutcome::Empty;
    }
    let Some(manager) = manager(app) else {
        error!("Voice notes manager is not initialised");
        return CaptureOutcome::Failed;
    };
    let settings = get_settings(app).voice_notes;
    let pending_if = |enabled: bool| {
        if enabled {
            DestinationState::Pending
        } else {
            DestinationState::Off
        }
    };

    let created_at = chrono::Utc::now().timestamp();
    let note = match manager.store().insert(NewNote {
        created_at,
        title: &draft.title,
        body: &draft.body,
        raw_text: raw.trim(),
        ai_processed,
        markdown: pending_if(settings.markdown_enabled),
        notion: pending_if(settings.notion_connected),
        apple_notes: pending_if(settings.apple_notes_enabled && cfg!(target_os = "macos")),
    }) {
        Ok(note) => note,
        Err(e) => {
            error!("Failed to store voice note: {e}");
            return CaptureOutcome::Failed;
        }
    };
    info!("Voice note {} saved", note.id);

    if settings.markdown_enabled {
        export_markdown(app, &manager, &note);
    }
    notify(app, note.id);
    spawn_remote_exports(app.clone(), note.id);
    CaptureOutcome::Saved
}

fn local_time(note: &VoiceNote) -> chrono::DateTime<Local> {
    Local
        .timestamp_opt(note.created_at, 0)
        .single()
        .unwrap_or_else(Local::now)
}

fn draft_of(note: &VoiceNote) -> NoteDraft {
    NoteDraft {
        title: note.title.clone(),
        body: note.body.clone(),
    }
}

fn export_markdown(app: &AppHandle, manager: &NotesManager, note: &VoiceNote) {
    let settings = get_settings(app).voice_notes;
    let state = match markdown_folder(app, &settings) {
        None => DestinationState::Failed {
            error: markdown::ERR_IO.to_string(),
        },
        Some(folder) => match markdown::write_note(
            &folder,
            settings.markdown_layout,
            &note.uid(),
            &local_time(note),
            &draft_of(note),
        ) {
            Ok(path) => DestinationState::Done {
                location: path.to_string_lossy().into_owned(),
            },
            Err(error) => {
                warn!("Voice note {}: Markdown export failed: {error}", note.id);
                DestinationState::Failed { error }
            }
        },
    };
    if let Err(e) = manager
        .store()
        .set_destination(note.id, Destination::Markdown, &state)
    {
        error!("Failed to record Markdown export state: {e}");
    }
}

/// Run every pending Notion / Apple Notes export for a note.
fn spawn_remote_exports(app: AppHandle, id: i64) {
    tauri::async_runtime::spawn(async move {
        let Some(manager) = manager(&app) else { return };
        let Ok(Some(note)) = manager.store().get(id) else {
            return;
        };
        let mut changed = false;

        if note.notion == DestinationState::Pending {
            let state = export_notion(&app, &note).await;
            let _ = manager
                .store()
                .set_destination(id, Destination::Notion, &state);
            changed = true;
        }

        if note.apple_notes == DestinationState::Pending {
            let html = text::apple_notes_html(&draft_of(&note));
            let state =
                match tauri::async_runtime::spawn_blocking(move || apple_notes::add_note(&html))
                    .await
                {
                    Ok(Ok(note_id)) => DestinationState::Done { location: note_id },
                    Ok(Err(error)) => {
                        warn!("Voice note {id}: Apple Notes export failed: {error}");
                        DestinationState::Failed { error }
                    }
                    Err(e) => DestinationState::Failed {
                        error: format!("{}: {e}", apple_notes::ERR_FAILED),
                    },
                };
            let _ = manager
                .store()
                .set_destination(id, Destination::AppleNotes, &state);
            changed = true;
        }

        if changed {
            // The note may have been deleted while exporting.
            if matches!(manager.store().get(id), Ok(Some(_))) {
                notify(&app, id);
            }
        }
    });
}

async fn export_notion(app: &AppHandle, note: &VoiceNote) -> DestinationState {
    let settings = get_settings(app).voice_notes;
    let Some(parent) = settings.notion_parent_page_id.clone() else {
        return DestinationState::Failed {
            error: notion::ERR_PAGE_NOT_FOUND.to_string(),
        };
    };
    let Some(token) = secrets::load_token(app) else {
        return DestinationState::Failed {
            error: notion::ERR_TOKEN_MISSING.to_string(),
        };
    };
    match notion::create_page(&token, &parent, &draft_of(note)).await {
        Ok(page) => {
            debug!("Voice note {} exported to Notion", note.id);
            DestinationState::Done { location: page.url }
        }
        Err(error) => {
            warn!("Voice note {}: Notion export failed: {error}", note.id);
            DestinationState::Failed { error }
        }
    }
}

/// Re-run every failed export of a note with the current settings.
pub fn retry_exports(app: &AppHandle, id: i64) -> Result<(), String> {
    let manager = manager(app).ok_or("not_ready")?;
    let note = manager
        .store()
        .get(id)
        .map_err(|e| e.to_string())?
        .ok_or("not_found")?;

    if note.markdown.is_failed() {
        export_markdown(app, &manager, &note);
    }
    for (state, destination) in [
        (&note.notion, Destination::Notion),
        (&note.apple_notes, Destination::AppleNotes),
    ] {
        if state.is_failed() {
            manager
                .store()
                .set_destination(id, destination, &DestinationState::Pending)
                .map_err(|e| e.to_string())?;
        }
    }
    notify(app, id);
    spawn_remote_exports(app.clone(), id);
    Ok(())
}
