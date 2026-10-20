//! Persisted configuration for voice notes. Lives in the main settings store
//! under a single `voice_notes` key so the feature touches `AppSettings` in
//! one place only.

use serde::{Deserialize, Serialize};
use specta::Type;

/// How notes are laid out in the Markdown folder.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Type, Default)]
#[serde(rename_all = "snake_case")]
pub enum MarkdownLayout {
    /// One journal file per day (`2026-10-21.md`), one section per note.
    #[default]
    Daily,
    /// One file per note (`2026-10-21 14-32 Title.md`).
    PerNote,
}

#[derive(Serialize, Deserialize, Debug, Clone, Type)]
#[serde(default)]
pub struct VoiceNotesSettings {
    /// Whether the voice-note shortcut is registered at all.
    pub enabled: bool,
    /// Title + light cleanup through the post-processing LLM, when one is
    /// configured. Without an LLM a title is derived from the first words.
    pub ai_cleanup: bool,
    /// Also write each note into a local Markdown folder.
    pub markdown_enabled: bool,
    /// `None` means the default folder (`~/Documents/Handy Notes`).
    pub markdown_folder: Option<String>,
    pub markdown_layout: MarkdownLayout,
    /// Set once a Notion token was verified and stored in the keychain. The
    /// token itself never lives in this file.
    pub notion_connected: bool,
    pub notion_parent_page_id: Option<String>,
    /// Display name of the parent page, captured when connecting.
    pub notion_parent_title: Option<String>,
    /// Also add each note to Apple Notes (macOS only), in a "Handy" folder.
    pub apple_notes_enabled: bool,
}

impl Default for VoiceNotesSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            ai_cleanup: true,
            markdown_enabled: true,
            markdown_folder: None,
            markdown_layout: MarkdownLayout::Daily,
            notion_connected: false,
            notion_parent_page_id: None,
            notion_parent_title: None,
            apple_notes_enabled: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_store_falls_back_to_defaults() {
        let parsed: VoiceNotesSettings =
            serde_json::from_value(serde_json::json!({ "markdown_layout": "per_note" })).unwrap();
        assert!(parsed.enabled);
        assert!(parsed.markdown_enabled);
        assert_eq!(parsed.markdown_layout, MarkdownLayout::PerNote);
        assert!(!parsed.notion_connected);
    }
}
