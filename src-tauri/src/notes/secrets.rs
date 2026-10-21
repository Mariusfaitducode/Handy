//! Storage for the Notion integration token, kept out of the settings file.
//!
//! macOS: the login Keychain (generic password). If the Keychain refuses the
//! write, and on other platforms, the token goes to a file in the app data
//! directory that is readable only by the current user.
//!
//! The file is never written in place: it is created as a fresh temporary
//! file with mode 0600 from the start (no window where it is readable by
//! others), filled, synced, then atomically renamed over the final path.
//!
//! Migration: a token left in the file by an earlier build is moved into the
//! Keychain on the next read (macOS), and its permissions are tightened back
//! to 0600 if they had drifted (Unix).

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

#[cfg(target_os = "macos")]
const SERVICE: &str = "computer.handy.voice-notes";
const ACCOUNT: &str = "notion-token";

fn token_path(app: &tauri::AppHandle) -> Option<PathBuf> {
    crate::portable::app_data_dir(app)
        .ok()
        .map(|d| d.join(ACCOUNT))
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
pub fn store_token(app: &tauri::AppHandle, token: &str) -> Result<(), String> {
    match keychain::store(token) {
        Ok(()) => {
            // The Keychain now holds the token: drop any fallback copy.
            if let Some(path) = token_path(app) {
                remove_token_file(&path);
            }
            Ok(())
        }
        Err(keychain_err) => {
            log::warn!("Keychain refused the Notion token ({keychain_err}); using a private file");
            let path = token_path(app).ok_or("no app data directory")?;
            write_token_file(&path, token)
                .map_err(|e| format!("keychain: {keychain_err}; file: {e}"))
        }
    }
}

#[cfg(target_os = "macos")]
pub fn load_token(app: &tauri::AppHandle) -> Option<String> {
    let path = token_path(app);
    if let Some(token) = keychain::load() {
        // A stale fallback file would otherwise outlive the Keychain entry.
        if let Some(path) = &path {
            remove_token_file(path);
        }
        return Some(token);
    }
    let path = path?;
    let token = read_token_file(&path)?;
    // Transparent migration: move a file-stored token into the Keychain.
    if keychain::store(&token).is_ok() {
        remove_token_file(&path);
    }
    Some(token)
}

#[cfg(target_os = "macos")]
pub fn delete_token(app: &tauri::AppHandle) {
    keychain::delete();
    if let Some(path) = token_path(app) {
        remove_token_file(&path);
    }
}

#[cfg(not(target_os = "macos"))]
pub fn store_token(app: &tauri::AppHandle, token: &str) -> Result<(), String> {
    let path = token_path(app).ok_or("no app data directory")?;
    write_token_file(&path, token).map_err(|e| e.to_string())
}

#[cfg(not(target_os = "macos"))]
pub fn load_token(app: &tauri::AppHandle) -> Option<String> {
    read_token_file(&token_path(app)?)
}

#[cfg(not(target_os = "macos"))]
pub fn delete_token(app: &tauri::AppHandle) {
    if let Some(path) = token_path(app) {
        remove_token_file(&path);
    }
}

// ---------------------------------------------------------------------------
// Keychain (macOS)
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
mod keychain {
    use super::{ACCOUNT, SERVICE};
    use security_framework::passwords;

    pub fn store(token: &str) -> Result<(), String> {
        passwords::set_generic_password(SERVICE, ACCOUNT, token.as_bytes())
            .map_err(|e| e.to_string())
    }

    pub fn load() -> Option<String> {
        passwords::get_generic_password(SERVICE, ACCOUNT)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
    }

    pub fn delete() {
        let _ = passwords::delete_generic_password(SERVICE, ACCOUNT);
    }
}

// ---------------------------------------------------------------------------
// Private file fallback
// ---------------------------------------------------------------------------

/// Open a brand-new file that only the current user can read or write.
/// `create_new` refuses to follow a pre-existing file or symlink, and the
/// mode is applied by `open(2)` itself, so the file is never more permissive
/// than 0600, even for an instant.
fn create_private(path: &Path) -> std::io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

/// Write `token` to `path` atomically: temp file (0600) in the same
/// directory, fsync, then rename over the destination.
fn write_token_file(path: &Path, token: &str) -> std::io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| std::io::Error::other("token path has no parent"))?;
    fs::create_dir_all(dir)?;
    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or(ACCOUNT);

    // A unique name per attempt; retry on the (unlikely) collision.
    let mut attempt = 0u32;
    let (tmp_path, mut file) = loop {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let tmp = dir.join(format!(
            ".{file_name}.{}.{nanos}.{attempt}.tmp",
            std::process::id()
        ));
        match create_private(&tmp) {
            Ok(f) => break (tmp, f),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && attempt < 16 => {
                attempt += 1;
            }
            Err(e) => return Err(e),
        }
    };

    let result = file
        .write_all(token.as_bytes())
        .and_then(|_| file.sync_all())
        .and_then(|_| {
            drop(file);
            fs::rename(&tmp_path, path)
        });
    if result.is_err() {
        let _ = fs::remove_file(&tmp_path);
    }
    result
}

/// Read a file-stored token. On Unix, tighten permissions that are looser
/// than 0600 (a file written by an earlier build).
fn read_token_file(path: &Path) -> Option<String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = fs::symlink_metadata(path) {
            if !meta.file_type().is_file() {
                return None;
            }
            if meta.permissions().mode() & 0o077 != 0 {
                let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
            }
        }
    }
    fs::read_to_string(path)
        .ok()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

fn remove_token_file(path: &Path) {
    let _ = fs::remove_file(path);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn write_then_read_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(ACCOUNT);
        write_token_file(&path, "secret_abc").unwrap();
        assert_eq!(read_token_file(&path).as_deref(), Some("secret_abc"));
    }

    #[test]
    fn overwrite_replaces_token_and_leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(ACCOUNT);
        write_token_file(&path, "first").unwrap();
        write_token_file(&path, "second").unwrap();
        assert_eq!(read_token_file(&path).as_deref(), Some("second"));
        let entries: Vec<_> = fs::read_dir(dir.path()).unwrap().collect();
        assert_eq!(entries.len(), 1, "temp files must not be left behind");
    }

    #[test]
    fn creates_missing_parent_directory() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join(ACCOUNT);
        write_token_file(&path, "tok").unwrap();
        assert_eq!(read_token_file(&path).as_deref(), Some("tok"));
    }

    #[test]
    fn empty_or_missing_file_reads_as_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(ACCOUNT);
        assert_eq!(read_token_file(&path), None);
        write_token_file(&path, "  \n").unwrap();
        assert_eq!(read_token_file(&path), None);
    }

    #[cfg(unix)]
    #[test]
    fn file_is_private_from_creation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(ACCOUNT);
        write_token_file(&path, "tok").unwrap();
        assert_eq!(mode(&path), 0o600);

        // The temp file itself is opened with 0600, not chmod-ed afterwards.
        let tmp = dir.path().join("probe");
        let _f = create_private(&tmp).unwrap();
        assert_eq!(mode(&tmp), 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn create_private_refuses_existing_path() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = dir.path().join("probe");
        fs::write(&tmp, "x").unwrap();
        assert!(create_private(&tmp).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn legacy_world_readable_file_is_tightened_on_read() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(ACCOUNT);
        fs::write(&path, "legacy").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(read_token_file(&path).as_deref(), Some("legacy"));
        assert_eq!(mode(&path), 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_is_not_followed_on_read() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("elsewhere");
        fs::write(&target, "not-a-token").unwrap();
        let path = dir.path().join(ACCOUNT);
        std::os::unix::fs::symlink(&target, &path).unwrap();
        assert_eq!(read_token_file(&path), None);
    }

    #[test]
    fn remove_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(ACCOUNT);
        write_token_file(&path, "tok").unwrap();
        remove_token_file(&path);
        remove_token_file(&path);
        assert_eq!(read_token_file(&path), None);
    }
}
