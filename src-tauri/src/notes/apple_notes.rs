//! Apple Notes destination (macOS), driven through AppleScript.
//!
//! Text is passed as `osascript` arguments, never spliced into the script,
//! so a note can contain any character without breaking or injecting code.

pub const ERR_DENIED: &str = "apple_notes_denied";
pub const ERR_FAILED: &str = "apple_notes_failed";
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub const ERR_UNSUPPORTED: &str = "apple_notes_unsupported";

/// Folder notes are filed into, created on first use.
pub const FOLDER: &str = "Handy";

#[cfg(target_os = "macos")]
const ADD_SCRIPT: &str = r#"on run argv
  set folderName to item 1 of argv
  set noteBody to item 2 of argv
  tell application "Notes"
    set targetAccount to default account
    if not (exists folder folderName of targetAccount) then
      make new folder at targetAccount with properties {name:folderName}
    end if
    set newNote to make new note at folder folderName of targetAccount with properties {body:noteBody}
    return id of newNote
  end tell
end run"#;

/// Touches Notes without changing anything, to trigger (or check) the
/// Automation permission prompt.
#[cfg(target_os = "macos")]
const PROBE_SCRIPT: &str = r#"tell application "Notes" to get name of default account"#;

#[cfg(target_os = "macos")]
const SHOW_SCRIPT: &str = r#"on run argv
  tell application "Notes"
    show note id (item 1 of argv)
    activate
  end tell
end run"#;

#[cfg(target_os = "macos")]
fn run(script: &str, args: &[&str]) -> Result<String, String> {
    let output = std::process::Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg(script)
        .args(args)
        .output()
        .map_err(|e| format!("{ERR_FAILED}: {e}"))?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).trim().to_string());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    // -1743: the user (or MDM) refused Automation access to Notes.
    if stderr.contains("-1743") || stderr.contains("Not authorized") {
        Err(ERR_DENIED.to_string())
    } else {
        Err(format!("{ERR_FAILED}: {stderr}"))
    }
}

/// Add the note and return its Apple Notes ID.
#[cfg(target_os = "macos")]
pub fn add_note(html_body: &str) -> Result<String, String> {
    run(ADD_SCRIPT, &[FOLDER, html_body])
}

#[cfg(target_os = "macos")]
pub fn check_access() -> Result<(), String> {
    run(PROBE_SCRIPT, &[]).map(|_| ())
}

#[cfg(target_os = "macos")]
pub fn show_note(id: &str) -> Result<(), String> {
    run(SHOW_SCRIPT, &[id]).map(|_| ())
}

#[cfg(not(target_os = "macos"))]
pub fn add_note(_html_body: &str) -> Result<String, String> {
    Err(ERR_UNSUPPORTED.to_string())
}

#[cfg(not(target_os = "macos"))]
pub fn check_access() -> Result<(), String> {
    Err(ERR_UNSUPPORTED.to_string())
}

#[cfg(not(target_os = "macos"))]
pub fn show_note(_id: &str) -> Result<(), String> {
    Err(ERR_UNSUPPORTED.to_string())
}
