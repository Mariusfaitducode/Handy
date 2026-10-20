//! SQLite index of voice notes (`notes.db` in the app data directory).
//!
//! Handy keeps its own copy of every note so the Notes page can list, search
//! and copy them regardless of which destinations are enabled, and so a
//! failed export (offline, permission refused…) can be retried later.

use anyhow::{anyhow, Result};
use rusqlite::{params, Connection, OptionalExtension};
use rusqlite_migration::{Migrations, M};
use serde::{Deserialize, Serialize};
use specta::Type;
use std::path::{Path, PathBuf};

static MIGRATIONS: &[M] = &[M::up(
    "CREATE TABLE IF NOT EXISTS voice_notes (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        created_at INTEGER NOT NULL,
        title TEXT NOT NULL,
        body TEXT NOT NULL,
        raw_text TEXT NOT NULL,
        ai_processed BOOLEAN NOT NULL DEFAULT 0,
        markdown TEXT NOT NULL DEFAULT '{\"status\":\"off\"}',
        notion TEXT NOT NULL DEFAULT '{\"status\":\"off\"}',
        apple_notes TEXT NOT NULL DEFAULT '{\"status\":\"off\"}'
    );
    CREATE INDEX IF NOT EXISTS idx_voice_notes_created_at ON voice_notes(created_at DESC);",
)];

/// Where a note stands for one destination.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Type, Default)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum DestinationState {
    /// Destination was not enabled when the note was taken.
    #[default]
    Off,
    /// Export in flight.
    Pending,
    /// Exported. `location` is a file path, a URL or an Apple Notes ID.
    Done { location: String },
    /// Export failed; `error` is a stable code (the `ERR_*` constants of each
    /// destination module) the UI translates, optionally followed by `: detail`.
    Failed { error: String },
}

impl DestinationState {
    fn to_column(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{\"status\":\"off\"}".into())
    }

    fn from_column(value: &str) -> Self {
        serde_json::from_str(value).unwrap_or_default()
    }

    pub fn location(&self) -> Option<&str> {
        match self {
            DestinationState::Done { location } => Some(location),
            _ => None,
        }
    }

    pub fn is_failed(&self) -> bool {
        matches!(self, DestinationState::Failed { .. })
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Type)]
pub struct VoiceNote {
    pub id: i64,
    /// Unix seconds.
    pub created_at: i64,
    pub title: String,
    pub body: String,
    /// The transcript before any cleanup, kept so nothing said is ever lost.
    pub raw_text: String,
    pub ai_processed: bool,
    pub markdown: DestinationState,
    pub notion: DestinationState,
    pub apple_notes: DestinationState,
}

impl VoiceNote {
    /// Stable identifier used in Markdown section markers. Includes the
    /// creation time so a reset database can never collide with old files.
    pub fn uid(&self) -> String {
        format!("{}-{}", self.created_at, self.id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Destination {
    Markdown,
    Notion,
    AppleNotes,
}

impl Destination {
    fn column(self) -> &'static str {
        match self {
            Destination::Markdown => "markdown",
            Destination::Notion => "notion",
            Destination::AppleNotes => "apple_notes",
        }
    }
}

pub struct NewNote<'a> {
    pub created_at: i64,
    pub title: &'a str,
    pub body: &'a str,
    pub raw_text: &'a str,
    pub ai_processed: bool,
    pub markdown: DestinationState,
    pub notion: DestinationState,
    pub apple_notes: DestinationState,
}

pub struct NotesStore {
    db_path: PathBuf,
}

const SELECT_COLUMNS: &str =
    "id, created_at, title, body, raw_text, ai_processed, markdown, notion, apple_notes";

impl NotesStore {
    pub fn open(db_path: &Path) -> Result<Self> {
        let mut conn = Connection::open(db_path)?;
        let migrations = Migrations::new(MIGRATIONS.to_vec());
        #[cfg(debug_assertions)]
        migrations
            .validate()
            .expect("Invalid voice notes migrations");
        migrations.to_latest(&mut conn)?;
        Ok(Self {
            db_path: db_path.to_path_buf(),
        })
    }

    fn conn(&self) -> Result<Connection> {
        Ok(Connection::open(&self.db_path)?)
    }

    fn map_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<VoiceNote> {
        Ok(VoiceNote {
            id: row.get(0)?,
            created_at: row.get(1)?,
            title: row.get(2)?,
            body: row.get(3)?,
            raw_text: row.get(4)?,
            ai_processed: row.get(5)?,
            markdown: DestinationState::from_column(&row.get::<_, String>(6)?),
            notion: DestinationState::from_column(&row.get::<_, String>(7)?),
            apple_notes: DestinationState::from_column(&row.get::<_, String>(8)?),
        })
    }

    pub fn insert(&self, note: NewNote<'_>) -> Result<VoiceNote> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO voice_notes (created_at, title, body, raw_text, ai_processed, markdown, notion, apple_notes)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                note.created_at,
                note.title,
                note.body,
                note.raw_text,
                note.ai_processed,
                note.markdown.to_column(),
                note.notion.to_column(),
                note.apple_notes.to_column(),
            ],
        )?;
        let id = conn.last_insert_rowid();
        self.get(id)?
            .ok_or_else(|| anyhow!("voice note {id} vanished after insert"))
    }

    pub fn get(&self, id: i64) -> Result<Option<VoiceNote>> {
        let conn = self.conn()?;
        Ok(conn
            .query_row(
                &format!("SELECT {SELECT_COLUMNS} FROM voice_notes WHERE id = ?1"),
                params![id],
                Self::map_row,
            )
            .optional()?)
    }

    /// Newest first. Bounded so a years-long journal can't stall the UI; the
    /// Markdown folder remains the complete archive.
    pub fn list(&self, limit: usize) -> Result<Vec<VoiceNote>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {SELECT_COLUMNS} FROM voice_notes ORDER BY created_at DESC, id DESC LIMIT ?1"
        ))?;
        let rows = stmt.query_map(params![limit as i64], Self::map_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn set_destination(
        &self,
        id: i64,
        destination: Destination,
        state: &DestinationState,
    ) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            &format!(
                "UPDATE voice_notes SET {} = ?1 WHERE id = ?2",
                destination.column()
            ),
            params![state.to_column(), id],
        )?;
        Ok(())
    }

    pub fn delete(&self, id: i64) -> Result<()> {
        let conn = self.conn()?;
        conn.execute("DELETE FROM voice_notes WHERE id = ?1", params![id])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store() -> (NotesStore, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "handy-notes-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        (NotesStore::open(&dir.join("notes.db")).unwrap(), dir)
    }

    fn new_note(created_at: i64, title: &str) -> NewNote<'_> {
        NewNote {
            created_at,
            title,
            body: "body",
            raw_text: "raw",
            ai_processed: false,
            markdown: DestinationState::Pending,
            notion: DestinationState::Off,
            apple_notes: DestinationState::Off,
        }
    }

    #[test]
    fn insert_list_update_delete_roundtrip() {
        let (store, dir) = temp_store();
        let a = store.insert(new_note(100, "older")).unwrap();
        let b = store.insert(new_note(200, "newer")).unwrap();

        let listed = store.list(10).unwrap();
        assert_eq!(
            listed.iter().map(|n| n.id).collect::<Vec<_>>(),
            vec![b.id, a.id]
        );
        assert_eq!(listed[0].markdown, DestinationState::Pending);

        let done = DestinationState::Done {
            location: "/tmp/x.md".into(),
        };
        store
            .set_destination(a.id, Destination::Markdown, &done)
            .unwrap();
        let failed = DestinationState::Failed {
            error: "network".into(),
        };
        store
            .set_destination(a.id, Destination::Notion, &failed)
            .unwrap();
        let reloaded = store.get(a.id).unwrap().unwrap();
        assert_eq!(reloaded.markdown.location(), Some("/tmp/x.md"));
        assert!(reloaded.notion.is_failed());
        assert_eq!(reloaded.uid(), format!("100-{}", a.id));

        store.delete(a.id).unwrap();
        assert!(store.get(a.id).unwrap().is_none());
        assert_eq!(store.list(1).unwrap().len(), 1);

        // Reopening runs migrations idempotently.
        drop(store);
        let reopened = NotesStore::open(&dir.join("notes.db")).unwrap();
        assert_eq!(reopened.list(10).unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn unknown_destination_column_reads_as_off() {
        assert_eq!(
            DestinationState::from_column("garbage"),
            DestinationState::Off
        );
    }
}
