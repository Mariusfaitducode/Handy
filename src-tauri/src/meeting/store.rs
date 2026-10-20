//! Local storage of meetings: `meetings.db` in the app data directory, apart
//! from the dictation history so the two evolve independently. Only text is
//! stored; meeting audio never touches the disk.

use anyhow::{anyhow, Result};
use rusqlite::{params, Connection, OptionalExtension};
use rusqlite_migration::{Migrations, M};
use serde::{Deserialize, Serialize};
use specta::Type;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use super::transcript::{MeetingSegment, Speaker};

static MIGRATIONS: &[M] = &[M::up(
    "CREATE TABLE meetings (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        title TEXT,
        source TEXT NOT NULL,
        calendar_event_id TEXT,
        started_at INTEGER NOT NULL,
        ended_at INTEGER,
        duration_ms INTEGER NOT NULL DEFAULT 0,
        status TEXT NOT NULL,
        system_audio TEXT NOT NULL,
        summary TEXT,
        summary_status TEXT NOT NULL DEFAULT 'none',
        summary_error TEXT,
        summary_model TEXT
    );
    CREATE TABLE meeting_segments (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        meeting_id INTEGER NOT NULL,
        speaker TEXT NOT NULL,
        start_ms INTEGER NOT NULL,
        end_ms INTEGER NOT NULL,
        text TEXT NOT NULL
    );
    CREATE INDEX meeting_segments_meeting ON meeting_segments (meeting_id, start_ms);
    CREATE INDEX meetings_started ON meetings (started_at);",
)];

/// Where a meeting is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum MeetingRecordStatus {
    /// Being captured right now.
    Recording,
    /// Capture ended; the last utterances and the summary are being produced.
    Processing,
    Done,
    /// Handy quit or crashed while recording; what was transcribed is kept.
    Interrupted,
}

impl MeetingRecordStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Recording => "recording",
            Self::Processing => "processing",
            Self::Done => "done",
            Self::Interrupted => "interrupted",
        }
    }

    fn parse(value: &str) -> Self {
        match value {
            "recording" => Self::Recording,
            "processing" => Self::Processing,
            "interrupted" => Self::Interrupted,
            _ => Self::Done,
        }
    }
}

/// State of a meeting's written summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum SummaryStatus {
    /// Not requested (yet).
    None,
    Pending,
    Done,
    Failed,
    /// No language model is configured; the transcript stands alone.
    NotConfigured,
    /// Too little was said to summarize.
    Empty,
}

impl SummaryStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Pending => "pending",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::NotConfigured => "not_configured",
            Self::Empty => "empty",
        }
    }

    fn parse(value: &str) -> Self {
        match value {
            "pending" => Self::Pending,
            "done" => Self::Done,
            "failed" => Self::Failed,
            "not_configured" => Self::NotConfigured,
            "empty" => Self::Empty,
            _ => Self::None,
        }
    }
}

/// Whether the other participants could be captured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum SystemAudioState {
    /// The computer's audio is captured on its own track.
    Captured,
    /// Not available on this OS (needs macOS 14.2+).
    Unsupported,
    /// The user refused "System Audio Recording" permission.
    Denied,
    /// Capture failed to start for another reason.
    Failed,
}

impl SystemAudioState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Captured => "captured",
            Self::Unsupported => "unsupported",
            Self::Denied => "denied",
            Self::Failed => "failed",
        }
    }

    pub fn parse(value: &str) -> Self {
        match value {
            "captured" => Self::Captured,
            "unsupported" => Self::Unsupported,
            "denied" => Self::Denied,
            _ => Self::Failed,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct MeetingRecord {
    pub id: i64,
    /// `None` until the user names it; the UI shows the date instead.
    pub title: Option<String>,
    /// What started it: "manual", "shortcut", "tray", "cli", "calendar".
    pub source: String,
    pub calendar_event_id: Option<String>,
    /// Unix milliseconds.
    pub started_at: i64,
    pub ended_at: Option<i64>,
    /// Recorded time, pauses excluded.
    pub duration_ms: i64,
    pub status: MeetingRecordStatus,
    pub system_audio: SystemAudioState,
    pub summary: Option<String>,
    pub summary_status: SummaryStatus,
    pub summary_error: Option<String>,
    /// "Provider · model" that wrote the summary.
    pub summary_model: Option<String>,
    pub segment_count: i64,
}

pub struct NewMeeting<'a> {
    pub title: Option<&'a str>,
    pub source: &'a str,
    pub calendar_event_id: Option<&'a str>,
    pub started_at: i64,
    pub system_audio: SystemAudioState,
}

pub struct MeetingStore {
    conn: Mutex<Connection>,
}

const RECORD_COLUMNS: &str = "m.id, m.title, m.source, m.calendar_event_id, m.started_at,
    m.ended_at, m.duration_ms, m.status, m.system_audio, m.summary, m.summary_status,
    m.summary_error, m.summary_model,
    (SELECT COUNT(*) FROM meeting_segments s WHERE s.meeting_id = m.id)";

impl MeetingStore {
    pub fn open(path: &Path) -> Result<Self> {
        let mut conn = Connection::open(path)?;
        Migrations::new(MIGRATIONS.to_vec()).to_latest(&mut conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    #[cfg(test)]
    pub fn in_memory() -> Result<Self> {
        let mut conn = Connection::open_in_memory()?;
        Migrations::new(MIGRATIONS.to_vec()).to_latest(&mut conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn default_path(app_data_dir: &Path) -> PathBuf {
        app_data_dir.join("meetings.db")
    }

    fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn create(&self, meeting: &NewMeeting<'_>) -> Result<i64> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO meetings (title, source, calendar_event_id, started_at, status, system_audio)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                meeting.title,
                meeting.source,
                meeting.calendar_event_id,
                meeting.started_at,
                MeetingRecordStatus::Recording.as_str(),
                meeting.system_audio.as_str()
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    fn map_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<MeetingRecord> {
        Ok(MeetingRecord {
            id: row.get(0)?,
            title: row.get(1)?,
            source: row.get(2)?,
            calendar_event_id: row.get(3)?,
            started_at: row.get(4)?,
            ended_at: row.get(5)?,
            duration_ms: row.get(6)?,
            status: MeetingRecordStatus::parse(&row.get::<_, String>(7)?),
            system_audio: SystemAudioState::parse(&row.get::<_, String>(8)?),
            summary: row.get(9)?,
            summary_status: SummaryStatus::parse(&row.get::<_, String>(10)?),
            summary_error: row.get(11)?,
            summary_model: row.get(12)?,
            segment_count: row.get(13)?,
        })
    }

    pub fn get(&self, id: i64) -> Result<Option<MeetingRecord>> {
        let conn = self.conn();
        let sql = format!("SELECT {RECORD_COLUMNS} FROM meetings m WHERE m.id = ?1");
        Ok(conn
            .query_row(&sql, params![id], Self::map_record)
            .optional()?)
    }

    /// Newest first.
    pub fn list(&self) -> Result<Vec<MeetingRecord>> {
        let conn = self.conn();
        let sql = format!("SELECT {RECORD_COLUMNS} FROM meetings m ORDER BY m.started_at DESC");
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map([], Self::map_record)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn segments(&self, meeting_id: i64) -> Result<Vec<MeetingSegment>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, speaker, start_ms, end_ms, text FROM meeting_segments
             WHERE meeting_id = ?1 ORDER BY start_ms, id",
        )?;
        let rows = stmt.query_map(params![meeting_id], |row| {
            Ok(MeetingSegment {
                id: row.get(0)?,
                speaker: Speaker::parse(&row.get::<_, String>(1)?),
                start_ms: row.get(2)?,
                end_ms: row.get(3)?,
                text: row.get(4)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn add_segment(
        &self,
        meeting_id: i64,
        speaker: Speaker,
        start_ms: i64,
        end_ms: i64,
        text: &str,
    ) -> Result<MeetingSegment> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO meeting_segments (meeting_id, speaker, start_ms, end_ms, text)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![meeting_id, speaker.as_str(), start_ms, end_ms, text],
        )?;
        Ok(MeetingSegment {
            id: conn.last_insert_rowid(),
            speaker,
            start_ms,
            end_ms,
            text: text.to_string(),
        })
    }

    pub fn delete_segment(&self, segment_id: i64) -> Result<()> {
        self.conn().execute(
            "DELETE FROM meeting_segments WHERE id = ?1",
            params![segment_id],
        )?;
        Ok(())
    }

    pub fn set_status(&self, id: i64, status: MeetingRecordStatus) -> Result<()> {
        self.conn().execute(
            "UPDATE meetings SET status = ?2 WHERE id = ?1",
            params![id, status.as_str()],
        )?;
        Ok(())
    }

    pub fn set_duration(&self, id: i64, duration_ms: i64) -> Result<()> {
        self.conn().execute(
            "UPDATE meetings SET duration_ms = ?2 WHERE id = ?1",
            params![id, duration_ms],
        )?;
        Ok(())
    }

    pub fn finish_capture(&self, id: i64, ended_at: i64, duration_ms: i64) -> Result<()> {
        self.conn().execute(
            "UPDATE meetings SET ended_at = ?2, duration_ms = ?3, status = ?4 WHERE id = ?1",
            params![
                id,
                ended_at,
                duration_ms,
                MeetingRecordStatus::Processing.as_str()
            ],
        )?;
        Ok(())
    }

    pub fn set_system_audio(&self, id: i64, state: SystemAudioState) -> Result<()> {
        self.conn().execute(
            "UPDATE meetings SET system_audio = ?2 WHERE id = ?1",
            params![id, state.as_str()],
        )?;
        Ok(())
    }

    pub fn rename(&self, id: i64, title: Option<&str>) -> Result<()> {
        let changed = self.conn().execute(
            "UPDATE meetings SET title = ?2 WHERE id = ?1",
            params![id, title],
        )?;
        if changed == 0 {
            return Err(anyhow!("meeting {id} not found"));
        }
        Ok(())
    }

    pub fn set_summary(
        &self,
        id: i64,
        status: SummaryStatus,
        summary: Option<&str>,
        error: Option<&str>,
        model: Option<&str>,
    ) -> Result<()> {
        self.conn().execute(
            "UPDATE meetings SET summary_status = ?2,
                summary = COALESCE(?3, summary),
                summary_error = ?4,
                summary_model = COALESCE(?5, summary_model)
             WHERE id = ?1",
            params![id, status.as_str(), summary, error, model],
        )?;
        Ok(())
    }

    pub fn delete(&self, id: i64) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM meeting_segments WHERE meeting_id = ?1",
            params![id],
        )?;
        tx.execute("DELETE FROM meetings WHERE id = ?1", params![id])?;
        tx.commit()?;
        Ok(())
    }

    /// Meetings left `recording` or `processing` by a previous run: Handy quit
    /// or crashed mid-meeting. Their transcript is intact up to the crash.
    pub fn mark_interrupted(&self) -> Result<Vec<i64>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT id FROM meetings WHERE status IN (?1, ?2)")?;
        let ids = stmt
            .query_map(
                params![
                    MeetingRecordStatus::Recording.as_str(),
                    MeetingRecordStatus::Processing.as_str()
                ],
                |row| row.get::<_, i64>(0),
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        conn.execute(
            "UPDATE meetings SET status = ?1,
                summary_status = CASE WHEN summary_status = ?2 THEN ?3 ELSE summary_status END
             WHERE status IN (?4, ?5)",
            params![
                MeetingRecordStatus::Interrupted.as_str(),
                SummaryStatus::Pending.as_str(),
                SummaryStatus::None.as_str(),
                MeetingRecordStatus::Recording.as_str(),
                MeetingRecordStatus::Processing.as_str()
            ],
        )?;
        Ok(ids)
    }

    /// Deletes finished meetings that started before `cutoff_ms`. Returns how
    /// many were deleted.
    pub fn delete_older_than(&self, cutoff_ms: i64) -> Result<usize> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM meeting_segments WHERE meeting_id IN
                (SELECT id FROM meetings WHERE started_at < ?1 AND status NOT IN (?2, ?3))",
            params![
                cutoff_ms,
                MeetingRecordStatus::Recording.as_str(),
                MeetingRecordStatus::Processing.as_str()
            ],
        )?;
        let deleted = tx.execute(
            "DELETE FROM meetings WHERE started_at < ?1 AND status NOT IN (?2, ?3)",
            params![
                cutoff_ms,
                MeetingRecordStatus::Recording.as_str(),
                MeetingRecordStatus::Processing.as_str()
            ],
        )?;
        tx.commit()?;
        Ok(deleted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_meeting(store: &MeetingStore, started_at: i64) -> i64 {
        store
            .create(&NewMeeting {
                title: None,
                source: "manual",
                calendar_event_id: None,
                started_at,
                system_audio: SystemAudioState::Captured,
            })
            .unwrap()
    }

    #[test]
    fn a_meeting_round_trips_with_its_segments() {
        let store = MeetingStore::in_memory().unwrap();
        let id = new_meeting(&store, 1_000);
        store
            .add_segment(id, Speaker::Others, 5_000, 7_000, "Bonjour")
            .unwrap();
        let first = store
            .add_segment(id, Speaker::Me, 1_000, 2_000, "Salut")
            .unwrap();

        let record = store.get(id).unwrap().unwrap();
        assert_eq!(record.status, MeetingRecordStatus::Recording);
        assert_eq!(record.segment_count, 2);
        assert_eq!(record.summary_status, SummaryStatus::None);

        let segments = store.segments(id).unwrap();
        assert_eq!(segments[0], first, "ordered by time");
        assert_eq!(segments[1].speaker, Speaker::Others);

        store.delete_segment(first.id).unwrap();
        assert_eq!(store.segments(id).unwrap().len(), 1);
    }

    #[test]
    fn finishing_and_summarizing_update_the_record() {
        let store = MeetingStore::in_memory().unwrap();
        let id = new_meeting(&store, 0);
        store.finish_capture(id, 60_000, 58_000).unwrap();
        assert_eq!(
            store.get(id).unwrap().unwrap().status,
            MeetingRecordStatus::Processing
        );
        store
            .set_summary(id, SummaryStatus::Pending, None, None, None)
            .unwrap();
        store
            .set_summary(
                id,
                SummaryStatus::Done,
                Some("## Résumé"),
                None,
                Some("OpenAI · gpt-4o-mini"),
            )
            .unwrap();
        store.set_status(id, MeetingRecordStatus::Done).unwrap();
        // A later failed regeneration keeps the previous summary text.
        store
            .set_summary(id, SummaryStatus::Failed, None, Some("timeout"), None)
            .unwrap();

        let record = store.get(id).unwrap().unwrap();
        assert_eq!(record.duration_ms, 58_000);
        assert_eq!(record.summary.as_deref(), Some("## Résumé"));
        assert_eq!(record.summary_status, SummaryStatus::Failed);
        assert_eq!(record.summary_error.as_deref(), Some("timeout"));
        assert_eq!(
            record.summary_model.as_deref(),
            Some("OpenAI · gpt-4o-mini")
        );
    }

    #[test]
    fn unfinished_meetings_are_marked_interrupted_on_restart() {
        let store = MeetingStore::in_memory().unwrap();
        let live = new_meeting(&store, 0);
        let done = new_meeting(&store, 0);
        store.set_status(done, MeetingRecordStatus::Done).unwrap();
        store
            .set_summary(live, SummaryStatus::Pending, None, None, None)
            .unwrap();

        assert_eq!(store.mark_interrupted().unwrap(), vec![live]);
        let record = store.get(live).unwrap().unwrap();
        assert_eq!(record.status, MeetingRecordStatus::Interrupted);
        assert_eq!(record.summary_status, SummaryStatus::None);
        assert_eq!(
            store.get(done).unwrap().unwrap().status,
            MeetingRecordStatus::Done
        );
    }

    #[test]
    fn retention_deletes_old_finished_meetings_only() {
        let store = MeetingStore::in_memory().unwrap();
        let old = new_meeting(&store, 100);
        store.set_status(old, MeetingRecordStatus::Done).unwrap();
        store.add_segment(old, Speaker::Me, 0, 1, "x").unwrap();
        let old_live = new_meeting(&store, 100);
        let recent = new_meeting(&store, 10_000);
        store.set_status(recent, MeetingRecordStatus::Done).unwrap();

        assert_eq!(store.delete_older_than(5_000).unwrap(), 1);
        assert!(store.get(old).unwrap().is_none());
        assert!(store.segments(old).unwrap().is_empty());
        assert!(store.get(old_live).unwrap().is_some());
        assert!(store.get(recent).unwrap().is_some());
    }

    #[test]
    fn rename_and_delete() {
        let store = MeetingStore::in_memory().unwrap();
        let id = new_meeting(&store, 0);
        store.rename(id, Some("Point hebdo")).unwrap();
        assert_eq!(
            store.get(id).unwrap().unwrap().title.as_deref(),
            Some("Point hebdo")
        );
        assert!(store.rename(999, Some("x")).is_err());
        store.delete(id).unwrap();
        assert!(store.list().unwrap().is_empty());
    }
}
