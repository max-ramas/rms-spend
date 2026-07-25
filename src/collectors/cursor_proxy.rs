//! Cursor IDE proxy collector (`cursor_proxy`).
//!
//! Reads **internal** Cursor stores (no API contract). Uses schema fingerprinting:
//! if expected columns/keys disappear → `CollectorStatus::Stale` and skip ingest
//! without failing the whole `rms-spend scan` run.
//!
//! Metrics are **line proxies**, not provider tokens:
//! - `input_tokens`  ← suggested lines (tab + composer)
//! - `output_tokens` ← accepted lines (tab + composer)
//! - `accuracy`      = `proxy`
//! - `cost_usd`      = always `None` (never invent a price from lines)

use super::SpendCollector;
use crate::event::{Accuracy, CollectorResult, CollectorStatus, SpendEvent};
use crate::ledger::Ledger;
use chrono::{TimeZone, Utc};
use rusqlite::Connection;
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// Expected columns on `ai_code_hashes` (order-independent fingerprint).
const EXPECTED_HASH_COLS: &[&str] = &[
    "hash",
    "source",
    "fileName",
    "requestId",
    "conversationId",
    "timestamp",
    "model",
];

/// Daily-stats key prefix used by current Cursor builds.
const DAILY_STATS_PREFIX: &str = "aiCodeTracking.dailyStats.v1.5.";

pub struct CursorProxyCollector {
    pub tracking_db: PathBuf,
    pub state_vscdb: PathBuf,
}

impl CursorProxyCollector {
    pub fn new(tracking_db: PathBuf, state_vscdb: PathBuf) -> Self {
        Self {
            tracking_db,
            state_vscdb,
        }
    }

    pub fn from_defaults() -> Self {
        Self::new(
            crate::paths::cursor_tracking_db(),
            crate::paths::cursor_state_vscdb(),
        )
    }
}

impl SpendCollector for CursorProxyCollector {
    fn id(&self) -> &str {
        "cursor_proxy"
    }

    fn scan(&self, _ledger: &Ledger) -> anyhow::Result<CollectorResult> {
        let fingerprint = schema_fingerprint(&self.tracking_db, &self.state_vscdb);

        if !self.tracking_db.exists() && !self.state_vscdb.exists() {
            return Ok(CollectorResult {
                events: vec![],
                status: CollectorStatus::Ok,
                message: Some(
                    "Cursor data not found (ai-code-tracking.db / state.vscdb missing)".into(),
                ),
                schema_fingerprint: fingerprint,
            });
        }

        // Version gate on tracking DB when present.
        if self.tracking_db.exists() {
            match validate_tracking_schema(&self.tracking_db) {
                Ok(true) => {}
                Ok(false) => {
                    return Ok(CollectorResult {
                        events: vec![],
                        status: CollectorStatus::Stale,
                        message: Some(format!(
                            "Cursor ai-code-tracking.db schema changed (expected {:?}). Skipping ingest — update rms-spend cursor_proxy.",
                            EXPECTED_HASH_COLS
                        )),
                        schema_fingerprint: fingerprint,
                    });
                }
                Err(e) => {
                    return Ok(CollectorResult {
                        events: vec![],
                        status: CollectorStatus::Stale,
                        message: Some(format!(
                            "Cannot open Cursor tracking DB (is Cursor locking it?): {e}"
                        )),
                        schema_fingerprint: fingerprint,
                    });
                }
            }
        }

        let mut events = Vec::new();
        let mut message_parts = Vec::new();

        match ingest_daily_stats(&self.state_vscdb) {
            Ok(batch) => {
                message_parts.push(format!("{} daily-stat event(s)", batch.len()));
                events.extend(batch);
            }
            Err(e) => {
                message_parts.push(format!("dailyStats skipped: {e}"));
                // Tracking DB alone is not enough for line proxies; mark partial.
                if events.is_empty() {
                    return Ok(CollectorResult {
                        events: vec![],
                        status: CollectorStatus::Partial,
                        message: Some(message_parts.join("; ")),
                        schema_fingerprint: fingerprint,
                    });
                }
            }
        }

        Ok(CollectorResult {
            message: Some(message_parts.join("; ")),
            events,
            status: CollectorStatus::Ok,
            schema_fingerprint: fingerprint,
        })
    }
}

fn validate_tracking_schema(path: &Path) -> anyhow::Result<bool> {
    let conn = open_readonly(path)?;
    let mut stmt = conn.prepare("PRAGMA table_info(ai_code_hashes)")?;
    let cols: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(1))?
        .filter_map(|c| c.ok())
        .collect();
    if cols.is_empty() {
        return Ok(false);
    }
    Ok(EXPECTED_HASH_COLS
        .iter()
        .all(|need| cols.iter().any(|c| c == need)))
}

fn schema_fingerprint(tracking: &Path, state: &Path) -> Option<String> {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"cursor_proxy\0");
    hasher.update(DAILY_STATS_PREFIX.as_bytes());
    if let Ok(conn) = open_readonly(tracking) {
        if let Ok(mut stmt) = conn.prepare("PRAGMA table_info(ai_code_hashes)") {
            let mut cols: Vec<String> = stmt
                .query_map([], |r| r.get::<_, String>(1))
                .into_iter()
                .flatten()
                .filter_map(|c| c.ok())
                .collect();
            cols.sort();
            for c in cols {
                hasher.update(c.as_bytes());
                hasher.update(b",");
            }
        }
    }
    if state.exists() {
        hasher.update(b"\0state");
    }
    Some(hasher.finalize().to_hex().to_string())
}

fn open_readonly(path: &Path) -> anyhow::Result<Connection> {
    // URI mode + immutable reduces lock fights with a live Cursor process.
    let uri = format!("file:{}?mode=ro", path.display());
    let conn = Connection::open_with_flags(
        &uri,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )?;
    let _ = conn.busy_timeout(std::time::Duration::from_millis(500));
    Ok(conn)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DailyStat {
    date: String,
    #[serde(default)]
    tab_suggested_lines: u64,
    #[serde(default)]
    tab_accepted_lines: u64,
    #[serde(default)]
    composer_suggested_lines: u64,
    #[serde(default)]
    composer_accepted_lines: u64,
}

fn ingest_daily_stats(state_db: &Path) -> anyhow::Result<Vec<SpendEvent>> {
    if !state_db.exists() {
        anyhow::bail!("state.vscdb missing");
    }
    let conn = open_readonly(state_db)?;
    let mut stmt =
        conn.prepare("SELECT key, value FROM ItemTable WHERE key LIKE ?1 ORDER BY key")?;
    let pattern = format!("{DAILY_STATS_PREFIX}%");
    let rows = stmt.query_map([&pattern], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })?;

    let mut events = Vec::new();
    let mut seen_keys = 0usize;
    for row in rows {
        let (key, value) = row?;
        seen_keys += 1;
        let stat: DailyStat = match serde_json::from_str(&value) {
            Ok(s) => s,
            Err(_) => continue,
        };
        // Require known shape: must have a date field matching key suffix roughly.
        if stat.date.is_empty() {
            continue;
        }
        let suggested = stat.tab_suggested_lines + stat.composer_suggested_lines;
        let accepted = stat.tab_accepted_lines + stat.composer_accepted_lines;
        if suggested == 0 && accepted == 0 {
            continue;
        }
        let event_id = make_daily_event_id(&stat.date);
        let occurred_at = parse_day_utc(&stat.date);
        events.push(SpendEvent {
            event_id,
            source: "cursor_proxy".into(),
            client: "cursor".into(),
            model: "cursor-lines".into(),
            input_tokens: suggested,
            output_tokens: accepted,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            cost_usd: None,
            currency: "USD".into(),
            accuracy: Accuracy::Proxy,
            occurred_at,
            session_id: Some(stat.date.clone()),
            project_slug: Some(key),
        });
    }

    if seen_keys == 0 {
        // Prefix changed → treat as schema drift.
        anyhow::bail!(
            "no keys matching '{DAILY_STATS_PREFIX}*' — Cursor stats schema may have changed"
        );
    }
    Ok(events)
}

pub fn make_daily_event_id(date: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"cursor_proxy\0");
    hasher.update(date.as_bytes());
    hasher.update(b"\0daily_stats");
    hasher.finalize().to_hex().to_string()
}

fn parse_day_utc(date: &str) -> chrono::DateTime<Utc> {
    // YYYY-MM-DD → noon UTC to avoid TZ edge when displaying local days.
    let parts: Vec<_> = date.split('-').collect();
    if parts.len() == 3 {
        if let (Ok(y), Ok(m), Ok(d)) = (
            parts[0].parse::<i32>(),
            parts[1].parse::<u32>(),
            parts[2].parse::<u32>(),
        ) {
            if let Some(dt) = Utc.with_ymd_and_hms(y, m, d, 12, 0, 0).single() {
                return dt;
            }
        }
    }
    Utc::now()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::Ledger;
    use tempfile::TempDir;

    fn write_fixture_state(dir: &Path) -> PathBuf {
        let db = dir.join("state.vscdb");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE ItemTable (key TEXT PRIMARY KEY, value TEXT);
             INSERT INTO ItemTable VALUES (
               'aiCodeTracking.dailyStats.v1.5.2026-07-20',
               '{\"date\":\"2026-07-20\",\"tabSuggestedLines\":1,\"tabAcceptedLines\":2,\"composerSuggestedLines\":10,\"composerAcceptedLines\":20}'
             );",
        )
        .unwrap();
        db
    }

    fn write_fixture_tracking(dir: &Path, good: bool) -> PathBuf {
        let db = dir.join("ai-code-tracking.db");
        let conn = Connection::open(&db).unwrap();
        if good {
            conn.execute_batch(
                "CREATE TABLE ai_code_hashes (
                    hash TEXT PRIMARY KEY,
                    source TEXT NOT NULL,
                    fileExtension TEXT,
                    fileName TEXT,
                    requestId TEXT,
                    conversationId TEXT,
                    timestamp INTEGER,
                    model TEXT,
                    createdAt INTEGER NOT NULL
                );",
            )
            .unwrap();
        } else {
            conn.execute_batch("CREATE TABLE ai_code_hashes (id INTEGER PRIMARY KEY, junk TEXT);")
                .unwrap();
        }
        db
    }

    #[test]
    fn daily_stats_become_proxy_events() {
        let dir = TempDir::new().unwrap();
        let state = write_fixture_state(dir.path());
        let tracking = write_fixture_tracking(dir.path(), true);
        let ledger = Ledger::open_in_memory().unwrap();
        let c = CursorProxyCollector::new(tracking, state);
        let r = c.scan(&ledger).unwrap();
        assert_eq!(r.status, CollectorStatus::Ok);
        assert_eq!(r.events.len(), 1);
        assert_eq!(r.events[0].accuracy, Accuracy::Proxy);
        assert_eq!(r.events[0].input_tokens, 11);
        assert_eq!(r.events[0].output_tokens, 22);
        assert!(r.events[0].cost_usd.is_none());
    }

    #[test]
    fn schema_drift_is_stale_not_panic() {
        let dir = TempDir::new().unwrap();
        let state = write_fixture_state(dir.path());
        let tracking = write_fixture_tracking(dir.path(), false);
        let ledger = Ledger::open_in_memory().unwrap();
        let c = CursorProxyCollector::new(tracking, state);
        let r = c.scan(&ledger).unwrap();
        assert_eq!(r.status, CollectorStatus::Stale);
        assert!(r.events.is_empty());
    }

    #[test]
    fn event_id_stable() {
        assert_eq!(
            make_daily_event_id("2026-07-20"),
            make_daily_event_id("2026-07-20")
        );
    }

    #[test]
    fn missing_files_ok() {
        let ledger = Ledger::open_in_memory().unwrap();
        let c = CursorProxyCollector::new(
            PathBuf::from("/no/such/tracking.db"),
            PathBuf::from("/no/such/state.vscdb"),
        );
        let r = c.scan(&ledger).unwrap();
        assert_eq!(r.status, CollectorStatus::Ok);
        assert!(r.events.is_empty());
    }
}
