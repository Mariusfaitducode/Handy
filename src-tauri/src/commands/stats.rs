use crate::managers::stats::{StatsManager, UsageStats};
use std::sync::Arc;
use tauri::State;

#[tauri::command]
#[specta::specta]
pub async fn get_usage_stats(
    stats_manager: State<'_, Arc<StatsManager>>,
) -> Result<UsageStats, String> {
    stats_manager.get_stats().map_err(|e| e.to_string())
}

/// Returns the speed actually stored (clamped to a sensible range).
#[tauri::command]
#[specta::specta]
pub async fn set_stats_typing_speed(
    stats_manager: State<'_, Arc<StatsManager>>,
    wpm: u32,
) -> Result<u32, String> {
    stats_manager.set_typing_wpm(wpm).map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn reset_usage_stats(stats_manager: State<'_, Arc<StatsManager>>) -> Result<(), String> {
    stats_manager.reset().map_err(|e| e.to_string())
}
