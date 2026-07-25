//! RMS Threads Assistant collector — reads local `app.db` usage columns.
//!
//! Canonical tables: `generated_replies`, `generated_posts`.
//! `tokens_used` is a pipeline total (stored as `input_tokens`); `cost_usd` kept when present.
//! Schema drift → `Stale` (does not fail the whole scan).

use super::SpendCollector;
use crate::event::{Accuracy, CollectorResult, CollectorStatus, SpendEvent};
use crate::ledger::Ledger;
use chrono::{DateTime, NaiveDateTime, Utc};
use rusqlite::Connection;
use std::path::{Path, PathBuf};

const EXPECTED_REPLY_COLS: &[&str] = &[
    "id",
    "tokens_used",
    "cost_usd",
    "created_at",
    "reply_config_id",
];
const EXPECTED_POST_COLS: &[&str] = &[
    "id",
    "tokens_used",
    "cost_usd",
    "created_at",
    "post_config_id",
];

pub struct RmsThreadsCollector {
    pub db_path: PathBuf,
}

impl RmsThreadsCollector {
    pub fn new(db_path: PathBuf) -> Self {
        Self { db_path }
    }

    pub fn from_defaults() -> Self {
        Self::new(crate::paths::rms_threads_app_db())
    }
}

impl SpendCollector for RmsThreadsCollector {
    fn id(&self) -> &str {
        "rms_threads"
    }

    fn scan(&self, _ledger: &Ledger) -> anyhow::Result<CollectorResult> {
        if !self.db_path.exists() {
            return Ok(CollectorResult {
                events: vec![],
                status: CollectorStatus::Ok,
                message: Some(format!(
                    "Threads app.db not found: {}",
                    self.db_path.display()
                )),
                schema_fingerprint: Some("rms_threads_v1".into()),
            });
        }

        let conn = match open_readonly(&self.db_path) {
            Ok(c) => c,
            Err(e) => {
                return Ok(CollectorResult {
                    events: vec![],
                    status: CollectorStatus::Stale,
                    message: Some(format!("Cannot open Threads DB: {e}")),
                    schema_fingerprint: None,
                });
            }
        };

        let replies_ok = table_has_cols(&conn, "generated_replies", EXPECTED_REPLY_COLS)?;
        let posts_ok = table_has_cols(&conn, "generated_posts", EXPECTED_POST_COLS)?;
        if !replies_ok && !posts_ok {
            return Ok(CollectorResult {
                events: vec![],
                status: CollectorStatus::Stale,
                message: Some(
                    "Threads app.db schema changed (generated_replies/posts). Skipping ingest."
                        .into(),
                ),
                schema_fingerprint: Some(fingerprint_tables(&conn)),
            });
        }

        let mut events = Vec::new();
        let mut partial_msgs = Vec::new();

        if replies_ok {
            match load_replies(&conn) {
                Ok(batch) => events.extend(batch),
                Err(e) => partial_msgs.push(format!("replies: {e}")),
            }
        } else {
            partial_msgs.push("generated_replies schema mismatch".into());
        }

        if posts_ok {
            match load_posts(&conn) {
                Ok(batch) => events.extend(batch),
                Err(e) => partial_msgs.push(format!("posts: {e}")),
            }
        } else {
            partial_msgs.push("generated_posts schema mismatch".into());
        }

        let status = if !partial_msgs.is_empty() && events.is_empty() {
            CollectorStatus::Stale
        } else if !partial_msgs.is_empty() {
            CollectorStatus::Partial
        } else {
            CollectorStatus::Ok
        };

        Ok(CollectorResult {
            message: Some(format!(
                "{} event(s){}",
                events.len(),
                if partial_msgs.is_empty() {
                    String::new()
                } else {
                    format!("; {}", partial_msgs.join("; "))
                }
            )),
            events,
            status,
            schema_fingerprint: Some(fingerprint_tables(&conn)),
        })
    }
}

fn open_readonly(path: &Path) -> anyhow::Result<Connection> {
    let uri = format!("file:{}?mode=ro", path.display());
    let conn = Connection::open_with_flags(
        &uri,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )?;
    let _ = conn.busy_timeout(std::time::Duration::from_millis(500));
    Ok(conn)
}

fn table_has_cols(conn: &Connection, table: &str, expected: &[&str]) -> anyhow::Result<bool> {
    let sql = format!("PRAGMA table_info({table})");
    let mut stmt = conn.prepare(&sql)?;
    let cols: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(1))?
        .filter_map(|c| c.ok())
        .collect();
    if cols.is_empty() {
        return Ok(false);
    }
    Ok(expected.iter().all(|need| cols.iter().any(|c| c == need)))
}

fn fingerprint_tables(conn: &Connection) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"rms_threads\0");
    for table in ["generated_replies", "generated_posts"] {
        hasher.update(table.as_bytes());
        if let Ok(mut stmt) = conn.prepare(&format!("PRAGMA table_info({table})")) {
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
    hasher.finalize().to_hex().to_string()
}

fn load_replies(conn: &Connection) -> anyhow::Result<Vec<SpendEvent>> {
    // Prefer model from reply_configs when available.
    let sql = r#"
        SELECT r.id, r.tokens_used, r.cost_usd, r.created_at,
               COALESCE(c.model, c.provider, 'unknown')
        FROM generated_replies r
        LEFT JOIN reply_configs c ON c.id = r.reply_config_id
        WHERE COALESCE(r.tokens_used, 0) > 0 OR r.cost_usd IS NOT NULL
    "#;
    let mut stmt = match conn.prepare(sql) {
        Ok(s) => s,
        Err(_) => {
            // Older schema without join — fall back.
            let mut stmt = conn.prepare(
                "SELECT id, tokens_used, cost_usd, created_at, 'unknown'
                 FROM generated_replies
                 WHERE COALESCE(tokens_used, 0) > 0 OR cost_usd IS NOT NULL",
            )?;
            return map_rows(&mut stmt, "reply");
        }
    };
    map_rows(&mut stmt, "reply")
}

fn load_posts(conn: &Connection) -> anyhow::Result<Vec<SpendEvent>> {
    let sql = r#"
        SELECT p.id, p.tokens_used, p.cost_usd, p.created_at,
               COALESCE(c.model, c.provider, 'unknown')
        FROM generated_posts p
        LEFT JOIN post_configs c ON c.id = p.post_config_id
        WHERE COALESCE(p.tokens_used, 0) > 0 OR p.cost_usd IS NOT NULL
    "#;
    let mut stmt = match conn.prepare(sql) {
        Ok(s) => s,
        Err(_) => {
            let mut stmt = conn.prepare(
                "SELECT id, tokens_used, cost_usd, created_at, 'unknown'
                 FROM generated_posts
                 WHERE COALESCE(tokens_used, 0) > 0 OR cost_usd IS NOT NULL",
            )?;
            return map_rows(&mut stmt, "post");
        }
    };
    map_rows(&mut stmt, "post")
}

fn map_rows(stmt: &mut rusqlite::Statement<'_>, kind: &str) -> anyhow::Result<Vec<SpendEvent>> {
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, Option<i64>>(1)?,
            r.get::<_, Option<f64>>(2)?,
            r.get::<_, Option<String>>(3)?,
            r.get::<_, String>(4)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (id, tokens, cost, created, model) = row?;
        let tokens = tokens.unwrap_or(0).max(0) as u64;
        let event_id = make_event_id(kind, id);
        out.push(SpendEvent {
            event_id,
            source: "rms_threads".into(),
            client: "rms-threads-assistant".into(),
            model,
            input_tokens: tokens,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            cost_usd: cost,
            currency: "USD".into(),
            accuracy: Accuracy::Exact,
            occurred_at: parse_sqlite_ts(created.as_deref()),
            session_id: Some(format!("{kind}-{id}")),
            project_slug: Some(kind.into()),
        });
    }
    Ok(out)
}

pub fn make_event_id(kind: &str, id: i64) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"rms_threads\0");
    hasher.update(kind.as_bytes());
    hasher.update(b"\0");
    hasher.update(&id.to_le_bytes());
    hasher.finalize().to_hex().to_string()
}

fn parse_sqlite_ts(raw: Option<&str>) -> DateTime<Utc> {
    let Some(s) = raw else {
        return Utc::now();
    };
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return dt.with_timezone(&Utc);
    }
    // SQLite CURRENT_TIMESTAMP: "YYYY-MM-DD HH:MM:SS"
    if let Ok(naive) = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S") {
        return DateTime::from_naive_utc_and_offset(naive, Utc);
    }
    if let Ok(naive) = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%SZ") {
        return DateTime::from_naive_utc_and_offset(naive, Utc);
    }
    Utc::now()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::Ledger;
    use tempfile::TempDir;

    fn fixture_db(dir: &Path) -> PathBuf {
        let path = dir.join("app.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE reply_configs (id INTEGER PRIMARY KEY, model TEXT, provider TEXT);
            CREATE TABLE post_configs (id INTEGER PRIMARY KEY, model TEXT, provider TEXT);
            CREATE TABLE generated_replies (
                id INTEGER PRIMARY KEY,
                account_id INTEGER NOT NULL,
                thread_id TEXT NOT NULL,
                draft_text TEXT NOT NULL,
                tokens_used INTEGER,
                cost_usd REAL,
                status TEXT NOT NULL,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                reply_config_id INTEGER
            );
            CREATE TABLE generated_posts (
                id INTEGER PRIMARY KEY,
                account_id INTEGER NOT NULL,
                project_id INTEGER NOT NULL,
                draft_text TEXT NOT NULL,
                tokens_used INTEGER,
                cost_usd REAL,
                status TEXT NOT NULL,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                post_config_id INTEGER
            );
            INSERT INTO reply_configs VALUES (1, 'gpt-4o-mini', 'openai');
            INSERT INTO generated_replies (id, account_id, thread_id, draft_text, tokens_used, cost_usd, status, created_at, reply_config_id)
            VALUES (10, 1, 't1', 'hi', 420, 0.002, 'draft', '2026-07-20 12:00:00', 1);
            INSERT INTO generated_posts (id, account_id, project_id, draft_text, tokens_used, cost_usd, status, created_at, post_config_id)
            VALUES (20, 1, 1, 'post', 100, NULL, 'draft', '2026-07-20 13:00:00', NULL);
            "#,
        )
        .unwrap();
        path
    }

    #[test]
    fn loads_replies_and_posts() {
        let dir = TempDir::new().unwrap();
        let db = fixture_db(dir.path());
        let ledger = Ledger::open_in_memory().unwrap();
        let c = RmsThreadsCollector::new(db);
        let r = c.scan(&ledger).unwrap();
        assert_eq!(r.status, CollectorStatus::Ok);
        assert_eq!(r.events.len(), 2);
        let reply = r
            .events
            .iter()
            .find(|e| e.session_id.as_deref() == Some("reply-10"))
            .unwrap();
        assert_eq!(reply.input_tokens, 420);
        assert!((reply.cost_usd.unwrap() - 0.002).abs() < 1e-9);
        assert_eq!(reply.model, "gpt-4o-mini");
    }

    #[test]
    fn missing_db_ok() {
        let ledger = Ledger::open_in_memory().unwrap();
        let c = RmsThreadsCollector::new(PathBuf::from("/no/such/app.db"));
        let r = c.scan(&ledger).unwrap();
        assert_eq!(r.status, CollectorStatus::Ok);
        assert!(r.events.is_empty());
    }
}
