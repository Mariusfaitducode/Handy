//! Tauri commands behind the Meetings section and the recording indicator.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{Local, TimeZone};
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{AppHandle, Manager};
use tauri_plugin_opener::OpenerExt;

use super::summary::{self, SummaryLanguage};
use super::transcript::{
    export_file_name, plain_transcript, render_markdown, MarkdownMeeting, MarkdownStrings,
};
use super::{
    system_audio, MeetingManager, MeetingRecord, MeetingSegment, MeetingSettings, MeetingSource,
    MeetingStatus, SystemAudioPermission,
};
use crate::settings::{get_settings, write_settings};

fn manager(app: &AppHandle) -> Result<Arc<MeetingManager>, String> {
    app.try_state::<Arc<MeetingManager>>()
        .map(|m| m.inner().clone())
        .ok_or_else(|| "meeting manager not initialized".to_string())
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct MeetingDetail {
    pub record: MeetingRecord,
    pub segments: Vec<MeetingSegment>,
}

/// What the Meetings section needs to explain before anything is recorded.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct MeetingCapabilities {
    /// The other participants can be captured (macOS 14.2+).
    pub system_audio_supported: bool,
    pub system_audio_permission: SystemAudioPermission,
    /// "Provider · model" summaries are sent to, or `None` when none is set up.
    pub summary_model: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum MeetingTextPart {
    Summary,
    Transcript,
    Markdown,
}

#[tauri::command]
#[specta::specta]
pub fn start_meeting(app: AppHandle, title: Option<String>) -> Result<MeetingStatus, String> {
    super::start_meeting(&app, title, MeetingSource::Manual).map_err(|e| e.code())
}

#[tauri::command]
#[specta::specta]
pub fn stop_meeting(app: AppHandle) -> Result<i64, String> {
    super::stop_meeting(&app).map_err(|e| e.code())
}

#[tauri::command]
#[specta::specta]
pub fn pause_meeting(app: AppHandle) -> Result<(), String> {
    super::pause_meeting(&app).map_err(|e| e.code())
}

#[tauri::command]
#[specta::specta]
pub fn resume_meeting(app: AppHandle) -> Result<(), String> {
    super::resume_meeting(&app).map_err(|e| e.code())
}

#[tauri::command]
#[specta::specta]
pub fn get_meeting_status(app: AppHandle) -> MeetingStatus {
    super::current_status(&app)
}

#[tauri::command]
#[specta::specta]
pub fn list_meetings(app: AppHandle) -> Result<Vec<MeetingRecord>, String> {
    manager(&app)?.store().list().map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub fn get_meeting(app: AppHandle, id: i64) -> Result<MeetingDetail, String> {
    let manager = manager(&app)?;
    let record = manager
        .store()
        .get(id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "not_found".to_string())?;
    let segments = manager.store().segments(id).map_err(|e| e.to_string())?;
    Ok(MeetingDetail { record, segments })
}

#[tauri::command]
#[specta::specta]
pub fn rename_meeting(app: AppHandle, id: i64, title: Option<String>) -> Result<(), String> {
    manager(&app)?.rename(id, title)
}

#[tauri::command]
#[specta::specta]
pub fn delete_meeting(app: AppHandle, id: i64) -> Result<(), String> {
    manager(&app)?.delete(id)
}

#[tauri::command]
#[specta::specta]
pub fn regenerate_meeting_summary(app: AppHandle, id: i64) -> Result<(), String> {
    manager(&app)?.regenerate_summary(id)
}

#[tauri::command]
#[specta::specta]
pub fn get_meeting_capabilities(app: AppHandle) -> MeetingCapabilities {
    let settings = get_settings(&app);
    MeetingCapabilities {
        system_audio_supported: system_audio::is_supported(),
        system_audio_permission: system_audio::permission(),
        summary_model: summary::resolve_model(&settings).map(|m| m.label()),
    }
}

#[tauri::command]
#[specta::specta]
pub fn update_meeting_settings(app: AppHandle, settings: MeetingSettings) -> Result<(), String> {
    let mut all = get_settings(&app);
    let retention_changed = all.meeting.retention != settings.retention;
    all.meeting = settings;
    write_settings(&app, all);
    if retention_changed {
        if let Ok(manager) = manager(&app) {
            manager.apply_retention();
        }
    }
    let status = super::current_status(&app);
    super::indicator::sync(&app, &status);
    Ok(())
}

/// Opens the pane where "System Audio Recording" access is granted.
#[tauri::command]
#[specta::specta]
pub fn open_system_audio_settings(app: AppHandle) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        app.opener()
            .open_url(
                "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture",
                None::<&str>,
            )
            .map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        Err("unsupported".to_string())
    }
}

#[tauri::command]
#[specta::specta]
pub fn get_meeting_text(app: AppHandle, id: i64, part: MeetingTextPart) -> Result<String, String> {
    let manager = manager(&app)?;
    let language = SummaryLanguage::from_app_language(&get_settings(&app).app_language);
    match part {
        MeetingTextPart::Summary => manager
            .store()
            .get(id)
            .map_err(|e| e.to_string())?
            .and_then(|r| r.summary)
            .ok_or_else(|| "no_summary".to_string()),
        MeetingTextPart::Transcript => {
            let segments = manager.store().segments(id).map_err(|e| e.to_string())?;
            Ok(plain_transcript(&segments, language.labels()))
        }
        MeetingTextPart::Markdown => markdown(&manager, id, language).map(|(_, md)| md),
    }
}

/// Saves the meeting as Markdown in `dir` (remembered as the export folder),
/// or in the saved export folder. Returns the file's path.
#[tauri::command]
#[specta::specta]
pub fn export_meeting_markdown(
    app: AppHandle,
    id: i64,
    dir: Option<String>,
) -> Result<String, String> {
    let manager = manager(&app)?;
    let dir = match dir {
        Some(dir) => {
            let mut settings = get_settings(&app);
            settings.meeting.export_dir = Some(dir.clone());
            write_settings(&app, settings);
            dir
        }
        None => get_settings(&app)
            .meeting
            .export_dir
            .ok_or_else(|| "no_export_dir".to_string())?,
    };
    export_to_dir(&app, &manager, id, &dir).map(|p| p.to_string_lossy().into_owned())
}

#[tauri::command]
#[specta::specta]
pub fn reveal_meeting_export(app: AppHandle, path: String) -> Result<(), String> {
    app.opener()
        .reveal_item_in_dir(PathBuf::from(path))
        .map_err(|e| e.to_string())
}

fn meeting_title(record: &MeetingRecord, language: SummaryLanguage) -> String {
    if let Some(title) = record.title.as_deref().filter(|t| !t.trim().is_empty()) {
        return title.to_string();
    }
    let when = Local.timestamp_millis_opt(record.started_at).single();
    match (language, when) {
        (SummaryLanguage::French, Some(t)) => {
            format!("Réunion du {}", t.format("%d/%m/%Y à %H:%M"))
        }
        (SummaryLanguage::English, Some(t)) => format!("Meeting {}", t.format("%Y-%m-%d %H:%M")),
        (SummaryLanguage::French, None) => "Réunion".to_string(),
        (SummaryLanguage::English, None) => "Meeting".to_string(),
    }
}

/// (file name, Markdown) of a meeting.
fn markdown(
    manager: &MeetingManager,
    id: i64,
    language: SummaryLanguage,
) -> Result<(String, String), String> {
    let record = manager
        .store()
        .get(id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "not_found".to_string())?;
    let segments = manager.store().segments(id).map_err(|e| e.to_string())?;
    let started = Local.timestamp_millis_opt(record.started_at).single();
    let when = started
        .map(|t| match language {
            SummaryLanguage::French => t.format("%d/%m/%Y %H:%M").to_string(),
            SummaryLanguage::English => t.format("%Y-%m-%d %H:%M").to_string(),
        })
        .unwrap_or_default();
    let title = meeting_title(&record, language);
    let strings = match language {
        SummaryLanguage::French => MarkdownStrings {
            date: "Date",
            duration: "Durée",
            summary: "Compte rendu",
            transcript: "Transcription",
            labels: language.labels(),
        },
        SummaryLanguage::English => MarkdownStrings {
            date: "Date",
            duration: "Duration",
            summary: "Minutes",
            transcript: "Transcript",
            labels: language.labels(),
        },
    };
    let summary = record
        .summary
        .as_deref()
        .filter(|_| record.summary_status == super::SummaryStatus::Done);
    let md = render_markdown(
        &MarkdownMeeting {
            title: &title,
            when: &when,
            duration_ms: record.duration_ms,
            summary,
            segments: &segments,
        },
        &strings,
    );
    let prefix = started
        .map(|t| t.format("%Y-%m-%d %Hh%M").to_string())
        .unwrap_or_else(|| "meeting".to_string());
    let name_title = record.title.as_deref().unwrap_or("");
    Ok((export_file_name(&prefix, name_title), md))
}

/// Writes the meeting's Markdown into `dir`, replacing an earlier export of
/// the same meeting.
pub(super) fn export_to_dir(
    app: &AppHandle,
    manager: &MeetingManager,
    id: i64,
    dir: &str,
) -> Result<PathBuf, String> {
    let language = SummaryLanguage::from_app_language(&get_settings(app).app_language);
    let (file_name, md) = markdown(manager, id, language)?;
    let dir = Path::new(dir);
    if !dir.is_dir() {
        return Err("export_dir_missing".to_string());
    }
    let path = dir.join(file_name);
    std::fs::write(&path, md).map_err(|e| format!("export_failed:{e}"))?;
    log::info!("Exported meeting {id} to {}", path.display());
    Ok(path)
}
