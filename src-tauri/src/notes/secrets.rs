//! Storage for the Notion integration token, kept out of the settings file.
//!
//! macOS: the login Keychain (generic password). Elsewhere: a file in the
//! app data directory readable only by the current user.

#[cfg(target_os = "macos")]
const SERVICE: &str = "computer.handy.voice-notes";
const ACCOUNT: &str = "notion-token";

#[cfg(target_os = "macos")]
pub fn store_token(_app: &tauri::AppHandle, token: &str) -> Result<(), String> {
    security_framework::passwords::set_generic_password(SERVICE, ACCOUNT, token.as_bytes())
        .map_err(|e| format!("keychain: {e}"))
}

#[cfg(target_os = "macos")]
pub fn load_token(_app: &tauri::AppHandle) -> Option<String> {
    security_framework::passwords::get_generic_password(SERVICE, ACCOUNT)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .filter(|t| !t.trim().is_empty())
}

#[cfg(target_os = "macos")]
pub fn delete_token(_app: &tauri::AppHandle) {
    let _ = security_framework::passwords::delete_generic_password(SERVICE, ACCOUNT);
}

#[cfg(not(target_os = "macos"))]
fn token_path(app: &tauri::AppHandle) -> Option<std::path::PathBuf> {
    crate::portable::app_data_dir(app)
        .ok()
        .map(|d| d.join(ACCOUNT))
}

#[cfg(not(target_os = "macos"))]
pub fn store_token(app: &tauri::AppHandle, token: &str) -> Result<(), String> {
    let path = token_path(app).ok_or("no app data directory")?;
    std::fs::write(&path, token).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn load_token(app: &tauri::AppHandle) -> Option<String> {
    let path = token_path(app)?;
    std::fs::read_to_string(path)
        .ok()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

#[cfg(not(target_os = "macos"))]
pub fn delete_token(app: &tauri::AppHandle) {
    if let Some(path) = token_path(app) {
        let _ = std::fs::remove_file(path);
    }
}
