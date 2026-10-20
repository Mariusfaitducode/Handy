//! Local usage statistics (words dictated, time saved, streaks, apps, languages).
//!
//! Stats live in their own `stats.db`, separate from `history.db`, on purpose:
//! history is pruned by the retention settings (by default only the last few
//! entries are kept), while stats must keep growing. Each dictation is stored
//! as one small row without any text, so the stats never hold what was said.
//!
//! On first launch the existing history is imported once so the page is not
//! empty for people who already used Handy.

use anyhow::Result;
use chrono::{DateTime, Duration, Local, NaiveDate, Utc};
use log::{debug, error, info, warn};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use rusqlite_migration::{Migrations, M};
use serde::{Deserialize, Serialize};
use specta::Type;
use std::path::PathBuf;
use tauri::{AppHandle, Emitter};

/// Event emitted (no payload) whenever the stats change, so an open
/// Statistics page can refresh itself.
pub const STATS_CHANGED_EVENT: &str = "usage-stats-changed";

/// Typing speed used to estimate time saved. 40 wpm is the commonly cited
/// average for adults typing on a keyboard.
pub const DEFAULT_TYPING_WPM: u32 = 40;
pub const MIN_TYPING_WPM: u32 = 10;
pub const MAX_TYPING_WPM: u32 = 200;

/// Speaking pace assumed for dictations whose duration is unknown (those
/// imported from history). Conversational speech is ~130-160 wpm.
const ASSUMED_SPEAKING_WPM: f64 = 150.0;

/// Below this much measured speech the average speaking pace is too noisy
/// to show.
const MIN_MEASURED_MS_FOR_PACE: i64 = 30_000;

/// Number of days shown in the activity chart (today included).
pub const CHART_DAYS: i64 = 30;

/// Number of apps / languages listed.
const TOP_LIMIT: i64 = 5;

const META_TYPING_WPM: &str = "typing_wpm";
const META_HISTORY_IMPORTED: &str = "history_imported";

static MIGRATIONS: &[M] = &[M::up(
    "CREATE TABLE IF NOT EXISTS usage_events (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        timestamp INTEGER NOT NULL,
        local_day TEXT NOT NULL,
        word_count INTEGER NOT NULL,
        duration_ms INTEGER,
        app_name TEXT,
        language TEXT,
        source TEXT NOT NULL DEFAULT 'live'
    );
    CREATE INDEX IF NOT EXISTS idx_usage_events_local_day ON usage_events(local_day);
    CREATE TABLE IF NOT EXISTS stats_meta (
        key TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );",
)];

#[derive(Clone, Debug, Serialize, Deserialize, Type, PartialEq)]
pub struct DailyUsage {
    /// Local calendar day, `YYYY-MM-DD`.
    pub day: String,
    pub words: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type, PartialEq)]
pub struct UsageShare {
    /// App name, or language code (ISO 639-1) for languages.
    pub name: String,
    pub words: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type, PartialEq)]
pub struct UsageStats {
    pub total_words: i64,
    pub total_dictations: i64,
    pub today_words: i64,
    /// Rolling last 7 days, today included.
    pub week_words: i64,
    /// Estimated, never negative.
    pub time_saved_seconds: i64,
    pub week_time_saved_seconds: i64,
    pub current_streak_days: i64,
    pub longest_streak_days: i64,
    /// Measured speaking pace, `None` until enough speech has been timed.
    pub speaking_wpm: Option<i64>,
    pub typing_wpm: i64,
    /// One entry per day for the last `CHART_DAYS` days, oldest first,
    /// zero-filled.
    pub daily: Vec<DailyUsage>,
    pub top_apps: Vec<UsageShare>,
    /// Words with a known app, to compute shares.
    pub app_words: i64,
    pub languages: Vec<UsageShare>,
    /// Words with a known language, to compute shares.
    pub language_words: i64,
    /// First day with any activity.
    pub first_day: Option<String>,
}

/// Aggregates the time-saved estimate needs.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
struct Totals {
    dictations: i64,
    words: i64,
    /// Words of dictations whose duration was measured.
    measured_words: i64,
    measured_ms: i64,
}

pub struct StatsManager {
    app_handle: AppHandle,
    db_path: PathBuf,
}

impl StatsManager {
    pub fn new(app_handle: &AppHandle) -> Result<Self> {
        let app_data_dir = crate::portable::app_data_dir(app_handle)?;
        let db_path = app_data_dir.join("stats.db");
        let history_db_path = app_data_dir.join("history.db");

        let manager = Self {
            app_handle: app_handle.clone(),
            db_path,
        };
        {
            let mut conn = manager.get_connection()?;
            run_migrations(&mut conn)?;
        }

        // Import past dictations once, off the startup path. Only history
        // entries older than now are imported, so a dictation recorded while
        // the import runs is never counted twice.
        let cutoff = Utc::now().timestamp();
        let db_path = manager.db_path.clone();
        let app = app_handle.clone();
        std::thread::spawn(
            move || match import_history_once(&db_path, &history_db_path, cutoff) {
                Ok(Some(count)) => {
                    info!("Imported {} past dictations into usage stats", count);
                    let _ = app.emit(STATS_CHANGED_EVENT, ());
                }
                Ok(None) => {}
                Err(e) => warn!("Failed to import history into usage stats: {}", e),
            },
        );

        Ok(manager)
    }

    fn get_connection(&self) -> Result<Connection> {
        open_connection(&self.db_path)
    }

    /// Record one finished dictation. Never fails the caller: stats are a
    /// side feature and must not break the transcription pipeline.
    pub fn record_dictation(
        &self,
        text: &str,
        duration_ms: Option<i64>,
        app_name: Option<String>,
        selected_language: &str,
        translated_to_english: bool,
    ) {
        let words = count_words(text);
        if words == 0 {
            return;
        }
        let language = dictation_language(text, selected_language, translated_to_english);
        let now = Local::now();
        let result = self.get_connection().and_then(|conn| {
            insert_event(
                &conn,
                now.timestamp(),
                now.date_naive(),
                words,
                duration_ms,
                app_name.as_deref(),
                language.as_deref(),
                "live",
            )
        });
        match result {
            Ok(()) => {
                debug!(
                    "Recorded dictation stats: {} words, app={:?}, lang={:?}",
                    words, app_name, language
                );
                let _ = self.app_handle.emit(STATS_CHANGED_EVENT, ());
            }
            Err(e) => error!("Failed to record dictation stats: {}", e),
        }
    }

    pub fn get_stats(&self) -> Result<UsageStats> {
        let conn = self.get_connection()?;
        compute_stats(&conn, Local::now().date_naive())
    }

    pub fn set_typing_wpm(&self, wpm: u32) -> Result<u32> {
        let wpm = wpm.clamp(MIN_TYPING_WPM, MAX_TYPING_WPM);
        let conn = self.get_connection()?;
        set_meta(&conn, META_TYPING_WPM, &wpm.to_string())?;
        let _ = self.app_handle.emit(STATS_CHANGED_EVENT, ());
        Ok(wpm)
    }

    /// Delete every recorded dictation. Settings (typing speed) are kept, and
    /// history is not re-imported afterwards.
    pub fn reset(&self) -> Result<()> {
        let conn = self.get_connection()?;
        conn.execute("DELETE FROM usage_events", [])?;
        set_meta(&conn, META_HISTORY_IMPORTED, "1")?;
        let _ = self.app_handle.emit(STATS_CHANGED_EVENT, ());
        Ok(())
    }
}

fn open_connection(path: &std::path::Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.busy_timeout(std::time::Duration::from_secs(2))?;
    Ok(conn)
}

fn run_migrations(conn: &mut Connection) -> Result<()> {
    let migrations = Migrations::new(MIGRATIONS.to_vec());
    #[cfg(debug_assertions)]
    migrations.validate().expect("Invalid stats migrations");
    migrations.to_latest(conn)?;
    Ok(())
}

fn get_meta(conn: &Connection, key: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT value FROM stats_meta WHERE key = ?1",
            params![key],
            |row| row.get(0),
        )
        .optional()?)
}

fn set_meta(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO stats_meta (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn insert_event(
    conn: &Connection,
    timestamp: i64,
    local_day: NaiveDate,
    words: i64,
    duration_ms: Option<i64>,
    app_name: Option<&str>,
    language: Option<&str>,
    source: &str,
) -> Result<()> {
    conn.execute(
        "INSERT INTO usage_events (timestamp, local_day, word_count, duration_ms, app_name, language, source)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            timestamp,
            local_day.format("%Y-%m-%d").to_string(),
            words,
            duration_ms,
            app_name,
            language,
            source
        ],
    )?;
    Ok(())
}

/// Import past dictations from history.db once. Returns `Some(count)` when an
/// import ran, `None` when it had already been done.
fn import_history_once(
    stats_db: &std::path::Path,
    history_db: &std::path::Path,
    cutoff: i64,
) -> Result<Option<usize>> {
    let mut conn = open_connection(stats_db)?;
    if get_meta(&conn, META_HISTORY_IMPORTED)?.is_some() {
        return Ok(None);
    }

    let rows = if history_db.exists() {
        let history = Connection::open_with_flags(history_db, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        read_history_rows(&history, cutoff)?
    } else {
        Vec::new()
    };

    let tx = conn.transaction()?;
    let mut imported = 0;
    for (timestamp, text) in rows {
        let words = count_words(&text);
        if words == 0 {
            continue;
        }
        let Some(day) = local_day_for(timestamp) else {
            continue;
        };
        let language = dictation_language(&text, "auto", false);
        insert_event(
            &tx,
            timestamp,
            day,
            words,
            None,
            None,
            language.as_deref(),
            "history",
        )?;
        imported += 1;
    }
    set_meta(&tx, META_HISTORY_IMPORTED, "1")?;
    tx.commit()?;
    Ok(Some(imported))
}

/// The text that was actually inserted for each past dictation.
fn read_history_rows(history: &Connection, cutoff: i64) -> Result<Vec<(i64, String)>> {
    let has_table: bool = history.query_row(
        "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='transcription_history'",
        [],
        |row| row.get(0),
    )?;
    if !has_table {
        return Ok(Vec::new());
    }
    let mut stmt = history.prepare(
        "SELECT timestamp,
                CASE WHEN post_processed_text IS NOT NULL AND post_processed_text != ''
                     THEN post_processed_text ELSE transcription_text END
         FROM transcription_history
         WHERE timestamp < ?1",
    )?;
    let rows = stmt
        .query_map(params![cutoff], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn local_day_for(timestamp: i64) -> Option<NaiveDate> {
    DateTime::from_timestamp(timestamp, 0).map(|utc| utc.with_timezone(&Local).date_naive())
}

fn day_string(day: NaiveDate) -> String {
    day.format("%Y-%m-%d").to_string()
}

fn query_totals(conn: &Connection, since_day: Option<&str>) -> Result<Totals> {
    // `local_day >= ''` matches every row, which keeps a single statement.
    let since = since_day.unwrap_or("");
    let totals = conn.query_row(
        "SELECT COUNT(*),
                COALESCE(SUM(word_count), 0),
                COALESCE(SUM(CASE WHEN duration_ms IS NOT NULL THEN word_count END), 0),
                COALESCE(SUM(duration_ms), 0)
         FROM usage_events
         WHERE local_day >= ?1",
        params![since],
        |row| {
            Ok(Totals {
                dictations: row.get(0)?,
                words: row.get(1)?,
                measured_words: row.get(2)?,
                measured_ms: row.get(3)?,
            })
        },
    )?;
    Ok(totals)
}

fn query_shares(conn: &Connection, column: &str) -> Result<(Vec<UsageShare>, i64)> {
    // `column` is one of two fixed identifiers, never user input.
    debug_assert!(column == "app_name" || column == "language");
    let mut stmt = conn.prepare(&format!(
        "SELECT {column}, SUM(word_count) AS words
         FROM usage_events
         WHERE {column} IS NOT NULL AND {column} != ''
         GROUP BY {column}
         ORDER BY words DESC, {column} ASC
         LIMIT ?1"
    ))?;
    let top = stmt
        .query_map(params![TOP_LIMIT], |row| {
            Ok(UsageShare {
                name: row.get(0)?,
                words: row.get(1)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let known: i64 = conn.query_row(
        &format!(
            "SELECT COALESCE(SUM(word_count), 0) FROM usage_events
             WHERE {column} IS NOT NULL AND {column} != ''"
        ),
        [],
        |row| row.get(0),
    )?;
    Ok((top, known))
}

fn compute_stats(conn: &Connection, today: NaiveDate) -> Result<UsageStats> {
    let typing_wpm = get_meta(conn, META_TYPING_WPM)?
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(DEFAULT_TYPING_WPM)
        .clamp(MIN_TYPING_WPM, MAX_TYPING_WPM);

    let all = query_totals(conn, None)?;
    let week_start = day_string(today - Duration::days(6));
    let week = query_totals(conn, Some(&week_start))?;

    let chart_start = today - Duration::days(CHART_DAYS - 1);
    let mut stmt = conn.prepare(
        "SELECT local_day, SUM(word_count) FROM usage_events
         WHERE local_day >= ?1 GROUP BY local_day",
    )?;
    let recent: Vec<(String, i64)> = stmt
        .query_map(params![day_string(chart_start)], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let daily = fill_days(chart_start, today, &recent);
    let today_words = daily.last().map(|d| d.words).unwrap_or(0);

    let mut stmt =
        conn.prepare("SELECT DISTINCT local_day FROM usage_events ORDER BY local_day")?;
    let active_days: Vec<NaiveDate> = stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .filter_map(|r| r.ok())
        .filter_map(|s| NaiveDate::parse_from_str(&s, "%Y-%m-%d").ok())
        .collect();
    let (current_streak_days, longest_streak_days) = streaks(&active_days, today);

    let (top_apps, app_words) = query_shares(conn, "app_name")?;
    let (languages, language_words) = query_shares(conn, "language")?;

    Ok(UsageStats {
        total_words: all.words,
        total_dictations: all.dictations,
        today_words,
        week_words: week.words,
        time_saved_seconds: time_saved_seconds(&all, typing_wpm),
        week_time_saved_seconds: time_saved_seconds(&week, typing_wpm),
        current_streak_days,
        longest_streak_days,
        speaking_wpm: speaking_wpm(&all),
        typing_wpm: typing_wpm as i64,
        daily,
        top_apps,
        app_words,
        languages,
        language_words,
        first_day: active_days.first().map(|d| day_string(*d)),
    })
}

/// One entry per day in `[start, end]`, zero-filled.
fn fill_days(start: NaiveDate, end: NaiveDate, rows: &[(String, i64)]) -> Vec<DailyUsage> {
    let mut out = Vec::new();
    let mut day = start;
    while day <= end {
        let key = day_string(day);
        let words = rows
            .iter()
            .find(|(d, _)| *d == key)
            .map(|(_, w)| *w)
            .unwrap_or(0);
        out.push(DailyUsage { day: key, words });
        day += Duration::days(1);
    }
    out
}

/// Time it would have taken to type the words, minus the time spent
/// speaking them. Speech that was not timed is assumed at a typical pace.
fn time_saved_seconds(totals: &Totals, typing_wpm: u32) -> i64 {
    if totals.words <= 0 || typing_wpm == 0 {
        return 0;
    }
    let typing = totals.words as f64 * 60.0 / typing_wpm as f64;
    let unmeasured_words = (totals.words - totals.measured_words).max(0) as f64;
    let speaking =
        totals.measured_ms as f64 / 1000.0 + unmeasured_words * 60.0 / ASSUMED_SPEAKING_WPM;
    (typing - speaking).max(0.0).round() as i64
}

fn speaking_wpm(totals: &Totals) -> Option<i64> {
    if totals.measured_ms < MIN_MEASURED_MS_FOR_PACE || totals.measured_words <= 0 {
        return None;
    }
    Some((totals.measured_words as f64 * 60_000.0 / totals.measured_ms as f64).round() as i64)
}

/// `(current, longest)` streak of consecutive active days. `days` must be
/// sorted ascending and unique. The current streak stays alive through today
/// if yesterday was active, so it does not drop to zero every morning.
fn streaks(days: &[NaiveDate], today: NaiveDate) -> (i64, i64) {
    let mut longest = 0;
    let mut run = 0;
    let mut prev: Option<NaiveDate> = None;
    for &day in days {
        if day > today {
            continue;
        }
        run = match prev {
            Some(p) if day == p + Duration::days(1) => run + 1,
            Some(p) if day == p => run,
            _ => 1,
        };
        longest = longest.max(run);
        prev = Some(day);
    }
    let current = match prev {
        Some(last) if last == today || last == today - Duration::days(1) => run,
        _ => 0,
    };
    (current, longest)
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3040..=0x30FF   // Hiragana, Katakana
        | 0x3400..=0x4DBF // CJK Extension A
        | 0x4E00..=0x9FFF // CJK Unified Ideographs
        | 0xF900..=0xFAFF // CJK Compatibility Ideographs
        | 0x20000..=0x2FA1F)
}

/// Count words the way a person would. Whitespace-separated tokens count when
/// they hold at least one letter or digit (so a lone "—" does not), and each
/// Chinese/Japanese character counts as one word since those scripts have no
/// spaces.
pub fn count_words(text: &str) -> i64 {
    let mut count = 0;
    for token in text.split_whitespace() {
        let mut cjk = 0;
        let mut has_other_word_char = false;
        let mut in_run = false;
        let mut runs = 0;
        for c in token.chars() {
            if is_cjk(c) {
                cjk += 1;
                in_run = false;
            } else if c.is_alphanumeric() {
                if !in_run {
                    runs += 1;
                    in_run = true;
                }
                has_other_word_char = true;
            } else {
                // Punctuation inside a word ("l'heure", "e-mail") keeps it one
                // word; only a CJK char breaks a mixed run.
            }
        }
        if cjk > 0 {
            count += cjk + runs;
        } else if has_other_word_char {
            count += 1;
        }
    }
    count
}

/// `pt-BR` → `pt`, `auto` / empty → `None`.
fn normalize_language(code: &str) -> Option<String> {
    let base = code
        .split(['-', '_'])
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if base.is_empty() || base == "auto" || !base.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    Some(base)
}

/// Language of a dictation: the one the user picked, English when the output
/// was translated, otherwise detected from the text (left unknown when the
/// text is too short to tell).
fn dictation_language(
    text: &str,
    selected_language: &str,
    translated_to_english: bool,
) -> Option<String> {
    if translated_to_english {
        return Some("en".to_string());
    }
    normalize_language(selected_language).or_else(|| {
        crate::audio_toolkit::detect_output_language(text, &[])
            .and_then(|code| normalize_language(&code))
    })
}

/// Localized name of the app in front, where the dictation is going.
/// `None` when unknown or when Handy itself is in front.
#[cfg(target_os = "macos")]
pub fn frontmost_app_name() -> Option<String> {
    use objc2_app_kit::NSWorkspace;
    let workspace = NSWorkspace::sharedWorkspace();
    let app = workspace.frontmostApplication()?;
    if app.processIdentifier() == std::process::id() as i32 {
        return None;
    }
    app.localizedName()
        .map(|name| name.to_string())
        .filter(|name| !name.trim().is_empty())
}

#[cfg(not(target_os = "macos"))]
pub fn frontmost_app_name() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn mem_db() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        run_migrations(&mut conn).unwrap();
        conn
    }

    #[test]
    fn counts_words_in_french_and_english() {
        assert_eq!(count_words(""), 0);
        assert_eq!(count_words("   "), 0);
        assert_eq!(count_words("Bonjour, ça va ?"), 3);
        assert_eq!(count_words("Il est l'heure d'envoyer l'e-mail."), 5);
        assert_eq!(count_words("Hello world — this is a test"), 6);
        assert_eq!(count_words("We have 3 options: A, B and C."), 8);
    }

    #[test]
    fn counts_cjk_characters_as_words() {
        assert_eq!(count_words("你好世界"), 4);
        assert_eq!(count_words("今日は"), 3);
        assert_eq!(count_words("我用Handy"), 3);
    }

    #[test]
    fn streaks_handle_gaps_and_today() {
        let today = d("2026-10-21");
        assert_eq!(streaks(&[], today), (0, 0));
        // Active today and the two days before.
        let days = [d("2026-10-19"), d("2026-10-20"), d("2026-10-21")];
        assert_eq!(streaks(&days, today), (3, 3));
        // Yesterday was the last active day: streak still alive.
        let days = [d("2026-10-19"), d("2026-10-20")];
        assert_eq!(streaks(&days, today), (2, 2));
        // Two days without activity: streak broken, longest kept.
        let days = [
            d("2026-10-01"),
            d("2026-10-02"),
            d("2026-10-03"),
            d("2026-10-04"),
            d("2026-10-19"),
        ];
        assert_eq!(streaks(&days, today), (0, 4));
        // Crossing a month boundary.
        let days = [d("2026-09-30"), d("2026-10-01")];
        assert_eq!(streaks(&days, d("2026-10-01")), (2, 2));
    }

    #[test]
    fn time_saved_uses_measured_and_assumed_speech() {
        // 400 words typed at 40 wpm = 600 s; spoken in 160 s measured.
        let t = Totals {
            dictations: 4,
            words: 400,
            measured_words: 400,
            measured_ms: 160_000,
        };
        assert_eq!(time_saved_seconds(&t, 40), 440);
        // Unmeasured words are assumed spoken at 150 wpm: 150 words = 60 s.
        let t = Totals {
            dictations: 1,
            words: 150,
            measured_words: 0,
            measured_ms: 0,
        };
        assert_eq!(time_saved_seconds(&t, 40), 225 - 60);
        // Slow dictation never yields negative savings.
        let t = Totals {
            dictations: 1,
            words: 2,
            measured_words: 2,
            measured_ms: 20_000,
        };
        assert_eq!(time_saved_seconds(&t, 40), 0);
        assert_eq!(time_saved_seconds(&Totals::default(), 40), 0);
    }

    #[test]
    fn speaking_pace_needs_enough_measured_speech() {
        let short = Totals {
            dictations: 1,
            words: 10,
            measured_words: 10,
            measured_ms: 5_000,
        };
        assert_eq!(speaking_wpm(&short), None);
        let long = Totals {
            dictations: 3,
            words: 260,
            measured_words: 260,
            measured_ms: 120_000,
        };
        assert_eq!(speaking_wpm(&long), Some(130));
    }

    #[test]
    fn normalizes_language_codes() {
        assert_eq!(normalize_language("fr"), Some("fr".into()));
        assert_eq!(normalize_language("pt-BR"), Some("pt".into()));
        assert_eq!(normalize_language("zh_Hant"), Some("zh".into()));
        assert_eq!(normalize_language("auto"), None);
        assert_eq!(normalize_language(""), None);
    }

    #[test]
    fn dictation_language_prefers_explicit_choice() {
        assert_eq!(
            dictation_language("bonjour tout le monde", "fr", false),
            Some("fr".into())
        );
        assert_eq!(
            dictation_language("bonjour tout le monde", "fr", true),
            Some("en".into())
        );
        assert_eq!(
            dictation_language(
                "Je voudrais réserver une table pour quatre personnes demain soir au restaurant",
                "auto",
                false
            ),
            Some("fr".into())
        );
    }

    #[test]
    fn computes_stats_from_events() {
        let conn = mem_db();
        let today = d("2026-10-21");
        let ins = |day: &str, words, dur: Option<i64>, app: Option<&str>, lang: Option<&str>| {
            insert_event(&conn, 0, d(day), words, dur, app, lang, "live").unwrap();
        };
        ins("2026-10-21", 100, Some(40_000), Some("Slack"), Some("fr"));
        ins("2026-10-21", 50, Some(20_000), Some("Notes"), Some("en"));
        ins("2026-10-20", 30, None, Some("Slack"), Some("fr"));
        ins("2026-10-10", 20, None, None, None);
        // Outside the 30-day chart but part of the totals.
        ins("2026-08-01", 200, None, None, Some("fr"));

        let s = compute_stats(&conn, today).unwrap();
        assert_eq!(s.total_words, 400);
        assert_eq!(s.total_dictations, 5);
        assert_eq!(s.today_words, 150);
        assert_eq!(s.week_words, 180);
        assert_eq!(s.current_streak_days, 2);
        assert_eq!(s.longest_streak_days, 2);
        assert_eq!(s.daily.len(), CHART_DAYS as usize);
        assert_eq!(s.daily.first().unwrap().day, "2026-09-22");
        assert_eq!(s.daily.last().unwrap().day, "2026-10-21");
        assert_eq!(s.daily.iter().map(|d| d.words).sum::<i64>(), 200);
        assert_eq!(s.typing_wpm, DEFAULT_TYPING_WPM as i64);
        assert_eq!(s.speaking_wpm, Some(150));
        assert_eq!(
            s.top_apps,
            vec![
                UsageShare {
                    name: "Slack".into(),
                    words: 130
                },
                UsageShare {
                    name: "Notes".into(),
                    words: 50
                }
            ]
        );
        assert_eq!(s.app_words, 180);
        assert_eq!(s.languages[0].name, "fr");
        assert_eq!(s.languages[0].words, 330);
        assert_eq!(s.language_words, 380);
        assert_eq!(s.first_day.as_deref(), Some("2026-08-01"));
        // 400 words at 40 wpm = 600 s; 60 s measured + 250 words at 150 wpm = 100 s.
        assert_eq!(s.time_saved_seconds, 440);
    }

    #[test]
    fn empty_database_yields_zeroed_stats() {
        let conn = mem_db();
        let s = compute_stats(&conn, d("2026-10-21")).unwrap();
        assert_eq!(s.total_words, 0);
        assert_eq!(s.current_streak_days, 0);
        assert_eq!(s.daily.len(), CHART_DAYS as usize);
        assert!(s.daily.iter().all(|d| d.words == 0));
        assert!(s.top_apps.is_empty());
        assert_eq!(s.first_day, None);
        assert_eq!(s.speaking_wpm, None);
    }

    #[test]
    fn typing_speed_is_read_from_meta_and_clamped() {
        let conn = mem_db();
        set_meta(&conn, META_TYPING_WPM, "60").unwrap();
        assert_eq!(
            compute_stats(&conn, d("2026-10-21")).unwrap().typing_wpm,
            60
        );
        set_meta(&conn, META_TYPING_WPM, "5000").unwrap();
        assert_eq!(
            compute_stats(&conn, d("2026-10-21")).unwrap().typing_wpm,
            MAX_TYPING_WPM as i64
        );
    }

    #[test]
    fn history_import_runs_once_and_respects_cutoff() {
        let dir = std::env::temp_dir().join(format!(
            "handy-stats-test-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let stats_db = dir.join("stats.db");
        let history_db = dir.join("history.db");
        {
            let h = Connection::open(&history_db).unwrap();
            h.execute_batch(
                "CREATE TABLE transcription_history (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    file_name TEXT NOT NULL,
                    timestamp INTEGER NOT NULL,
                    saved BOOLEAN NOT NULL DEFAULT 0,
                    title TEXT NOT NULL,
                    transcription_text TEXT NOT NULL,
                    post_processed_text TEXT,
                    post_process_prompt TEXT,
                    post_process_requested BOOLEAN NOT NULL DEFAULT 0
                );
                INSERT INTO transcription_history (file_name, timestamp, title, transcription_text, post_processed_text)
                VALUES ('a.wav', 1000, 't', 'one two three', NULL),
                       ('b.wav', 2000, 't', 'raw text', 'polished final text here'),
                       ('c.wav', 3000, 't', '', NULL),
                       ('d.wav', 9999999999, 't', 'after cutoff', NULL);",
            )
            .unwrap();
        }
        {
            let mut c = open_connection(&stats_db).unwrap();
            run_migrations(&mut c).unwrap();
        }
        let cutoff = 5_000_000_000;
        assert_eq!(
            import_history_once(&stats_db, &history_db, cutoff).unwrap(),
            Some(2)
        );
        assert_eq!(
            import_history_once(&stats_db, &history_db, cutoff).unwrap(),
            None
        );
        let c = open_connection(&stats_db).unwrap();
        let t = query_totals(&c, None).unwrap();
        assert_eq!(t.dictations, 2);
        assert_eq!(t.words, 3 + 4);
        assert_eq!(t.measured_ms, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
