//! Tauri commands behind "Upcoming meetings" and the calendar settings.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{AppHandle, Manager};
use tauri_plugin_opener::OpenerExt;

use super::bridge;
use super::scheduler::{self, CalendarState};
use super::{
    CalendarAccess, CalendarAnswer, CalendarInfo, CalendarMode, CalendarOverview, CalendarSettings,
    NotificationAccess,
};
use crate::settings::get_settings;

fn overview(app: &AppHandle) -> CalendarOverview {
    let mode = get_settings(app).calendar.mode;
    let access = bridge::access();
    let upcoming = match (access, mode, app.try_state::<Arc<CalendarState>>()) {
        (CalendarAccess::Granted, CalendarMode::Ask | CalendarMode::Auto, Some(state)) => {
            state.upcoming()
        }
        _ => Vec::new(),
    };
    CalendarOverview {
        access,
        notifications: bridge::notification_access(),
        mode,
        upcoming,
    }
}

/// EventKit and notification status calls wait on the system: keep them off
/// the main thread.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn get_calendar_overview(app: AppHandle) -> Result<CalendarOverview, String> {
    blocking(move || overview(&app)).await
}

/// Turns the feature on: asks for calendar access (and then for
/// notifications) if needed, and switches to "Ask" when access is granted.
#[tauri::command]
#[specta::specta]
pub async fn enable_calendar(app: AppHandle) -> Result<CalendarOverview, String> {
    blocking(move || {
        let mut access = bridge::access();
        if access == CalendarAccess::NotDetermined {
            access = bridge::request_access();
        }
        if access == CalendarAccess::Granted {
            if bridge::notification_access() == NotificationAccess::NotDetermined {
                bridge::request_notifications();
            }
            let mut settings = get_settings(&app);
            if settings.calendar.mode == CalendarMode::Off {
                settings.calendar.mode = CalendarMode::Ask;
                scheduler::update_settings(&app, settings.calendar);
            }
        }
        scheduler::wake();
        overview(&app)
    })
    .await
}

/// Asks for notification access again (after the feature was set up with
/// it undetermined).
#[tauri::command]
#[specta::specta]
pub async fn request_calendar_notifications(app: AppHandle) -> Result<CalendarOverview, String> {
    blocking(move || {
        bridge::request_notifications();
        overview(&app)
    })
    .await
}

#[tauri::command]
#[specta::specta]
pub async fn list_calendars(app: AppHandle) -> Result<Vec<CalendarInfo>, String> {
    blocking(move || {
        let excluded = get_settings(&app).calendar.excluded_calendars;
        let mut calendars: Vec<CalendarInfo> = bridge::calendars()?
            .into_iter()
            .map(|c| CalendarInfo {
                followed: !excluded.contains(&c.id),
                id: c.id,
                title: c.title,
                source: c.source,
                color: c.color,
            })
            .collect();
        calendars.sort_by(|a, b| {
            a.source
                .to_lowercase()
                .cmp(&b.source.to_lowercase())
                .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
        });
        Ok(calendars)
    })
    .await?
}

#[tauri::command]
#[specta::specta]
pub fn update_calendar_settings(app: AppHandle, settings: CalendarSettings) -> Result<(), String> {
    scheduler::update_settings(&app, settings);
    Ok(())
}

/// Record (`true`), don't record (`false`) or follow the rules (`None`) for
/// one occurrence.
#[tauri::command]
#[specta::specta]
pub fn set_calendar_event_choice(
    app: AppHandle,
    key: String,
    record: Option<bool>,
) -> Result<(), String> {
    scheduler::set_choice(&app, &key, record);
    Ok(())
}

/// The answer to a prompt shown in the window (when notifications are off).
#[tauri::command]
#[specta::specta]
pub async fn answer_calendar_prompt(
    app: AppHandle,
    key: String,
    answer: CalendarAnswer,
) -> Result<(), String> {
    blocking(move || {
        if let Some(state) = app.try_state::<Arc<CalendarState>>() {
            scheduler::answer_prompt(&app, &state, &key, answer);
        }
        scheduler::wake();
    })
    .await
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum CalendarSettingsPane {
    Calendars,
    Notifications,
}

/// Opens the System Settings pane where access is granted.
#[tauri::command]
#[specta::specta]
pub fn open_calendar_system_settings(
    app: AppHandle,
    pane: CalendarSettingsPane,
) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let url = match pane {
            CalendarSettingsPane::Calendars => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Calendars"
            }
            CalendarSettingsPane::Notifications => {
                "x-apple.systempreferences:com.apple.Notifications-Settings.extension"
            }
        };
        app.opener()
            .open_url(url, None::<&str>)
            .map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, pane);
        Err("unsupported".to_string())
    }
}

/// Opens an event's call link in the browser (or the Zoom/Teams app).
#[tauri::command]
#[specta::specta]
pub fn open_meeting_link(app: AppHandle, url: String) -> Result<(), String> {
    let lower = url.to_ascii_lowercase();
    if !(lower.starts_with("https://") || lower.starts_with("http://")) {
        return Err("invalid_url".to_string());
    }
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| e.to_string())
}
