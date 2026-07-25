//! Zed editor collector — `~/Library/Application Support/Zed/threads/threads.db`.
//!
//! Thread blobs are zstd-compressed JSON (`data_type = "zstd"`) with
//! `request_token_usage` / `cumulative_token_usage`. Older builds may store
//! plain JSON (`data_type = "json"`).
//!
//! **Timestamps:** Zed does not export per-request message timestamps in current
//! thread blobs (User messages have `id`/`content` only). Every event therefore
//! uses the thread `updated_at`. Usage map keys do match User message ids — we
//! can join them for identity checks without storing message content.

use super::util::{event_id, parse_rfc3339_or};
use super::SpendCollector;
use crate::event::{Accuracy, CollectorResult, CollectorStatus, SpendEvent};
use crate::ledger::Ledger;
use chrono::{DateTime, Utc};
use rusqlite::Connection;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;

const EXPECTED_THREAD_COLS: &[&str] = &["id", "data", "data_type", "updated_at"];

pub struct ZedCollector {
    pub threads_db: PathBuf,
}

impl ZedCollector {
    pub fn new(threads_db: PathBuf) -> Self {
        Self { threads_db }
    }
}

impl SpendCollector for ZedCollector {
    fn id(&self) -> &str {
        "zed"
    }

    fn scan(&self, _ledger: &Ledger) -> anyhow::Result<CollectorResult> {
        if !self.threads_db.exists() {
            return Ok(CollectorResult {
                events: vec![],
                status: CollectorStatus::Ok,
                message: Some(format!(
                    "Zed threads.db not found: {}",
                    self.threads_db.display()
                )),
                schema_fingerprint: Some("zed_threads_v2".into()),
            });
        }

        let conn = Connection::open_with_flags(
            &self.threads_db,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        if !table_has_cols(&conn, "threads", EXPECTED_THREAD_COLS)? {
            return Ok(CollectorResult {
                events: vec![],
                status: CollectorStatus::Stale,
                message: Some("Zed threads.db schema changed".into()),
                schema_fingerprint: Some("zed_stale".into()),
            });
        }

        let mut stmt = conn.prepare("SELECT id, updated_at, data_type, data FROM threads")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Vec<u8>>(3)?,
            ))
        })?;

        let mut events = Vec::new();
        let mut threads = 0usize;
        let mut errors = 0usize;
        for row in rows {
            let (thread_id, updated_at, data_type, data) = row?;
            threads += 1;
            match decode_thread(&thread_id, &updated_at, &data_type, &data) {
                Ok(mut batch) => events.append(&mut batch),
                Err(e) => {
                    errors += 1;
                    eprintln!("warn: skip zed thread {thread_id}: {e}");
                }
            }
        }

        let status = if errors > 0 && events.is_empty() && threads > 0 {
            CollectorStatus::Stale
        } else if errors > 0 {
            CollectorStatus::Partial
        } else {
            CollectorStatus::Ok
        };

        Ok(CollectorResult {
            message: Some(format!(
                "Zed: {threads} threads, {} events, {errors} errors",
                events.len()
            )),
            events,
            status,
            schema_fingerprint: Some("zed_threads_v2".into()),
        })
    }
}

fn table_has_cols(conn: &Connection, table: &str, expected: &[&str]) -> anyhow::Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let cols: Vec<String> = stmt
        .query_map([], |row| row.get(1))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(expected.iter().all(|c| cols.iter().any(|col| col == c)))
}

#[derive(Debug, Deserialize)]
struct ZedThread {
    #[serde(default)]
    model: Option<ZedModel>,
    #[serde(default)]
    request_token_usage: HashMap<String, ZedUsage>,
    #[serde(default)]
    cumulative_token_usage: Option<ZedUsage>,
    /// Optional messages — only `User.id` is read for usage-key join tests.
    #[serde(default)]
    #[allow(dead_code)]
    messages: Vec<ZedMessage>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
#[allow(dead_code)]
enum ZedMessage {
    User {
        #[serde(rename = "User")]
        user: ZedUserMsg,
    },
    Other(serde_json::Value),
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct ZedUserMsg {
    id: String,
    /// Content intentionally ignored — never stored in ledger events.
    #[serde(default)]
    content: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct ZedModel {
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    provider: Option<String>,
}

#[derive(Debug, Default, Clone, Deserialize)]
struct ZedUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
}

impl ZedUsage {
    fn is_empty(&self) -> bool {
        self.input_tokens == 0
            && self.output_tokens == 0
            && self.cache_read_input_tokens == 0
            && self.cache_creation_input_tokens == 0
    }

    fn saturating_sub(&self, other: &Self) -> Self {
        Self {
            input_tokens: self.input_tokens.saturating_sub(other.input_tokens),
            output_tokens: self.output_tokens.saturating_sub(other.output_tokens),
            cache_read_input_tokens: self
                .cache_read_input_tokens
                .saturating_sub(other.cache_read_input_tokens),
            cache_creation_input_tokens: self
                .cache_creation_input_tokens
                .saturating_sub(other.cache_creation_input_tokens),
        }
    }
}

fn decode_thread(
    thread_id: &str,
    updated_at: &str,
    data_type: &str,
    data: &[u8],
) -> anyhow::Result<Vec<SpendEvent>> {
    let json = match data_type {
        "zstd" => {
            let decoded = zstd::decode_all(data)?;
            String::from_utf8(decoded)?
        }
        "json" => String::from_utf8(data.to_vec())?,
        other => anyhow::bail!("unsupported data_type: {other}"),
    };
    let thread: ZedThread = serde_json::from_str(&json)?;
    Ok(events_from_thread(thread_id, updated_at, &thread))
}

fn events_from_thread(thread_id: &str, updated_at: &str, thread: &ZedThread) -> Vec<SpendEvent> {
    let occurred_at = parse_rfc3339_or(Some(updated_at), Utc::now());
    let model = thread
        .model
        .as_ref()
        .and_then(|m| m.model.clone())
        .unwrap_or_else(|| "unknown".into());

    let mut events = Vec::new();
    let mut sum = ZedUsage::default();
    for (msg_id, usage) in &thread.request_token_usage {
        if usage.is_empty() {
            continue;
        }
        sum.input_tokens += usage.input_tokens;
        sum.output_tokens += usage.output_tokens;
        sum.cache_read_input_tokens += usage.cache_read_input_tokens;
        sum.cache_creation_input_tokens += usage.cache_creation_input_tokens;
        events.push(usage_event(
            &event_id(&["zed", thread_id, msg_id]),
            thread_id,
            &model,
            usage,
            occurred_at,
        ));
    }

    if let Some(cum) = &thread.cumulative_token_usage {
        if !cum.is_empty() {
            let rem = cum.saturating_sub(&sum);
            if !rem.is_empty() {
                events.push(usage_event(
                    &event_id(&["zed", thread_id, "cumulative-remainder"]),
                    thread_id,
                    &model,
                    &rem,
                    occurred_at,
                ));
            }
        }
    }

    events
}

fn usage_event(
    event_id: &str,
    thread_id: &str,
    model: &str,
    usage: &ZedUsage,
    occurred_at: DateTime<Utc>,
) -> SpendEvent {
    SpendEvent {
        event_id: event_id.to_string(),
        source: "zed".into(),
        client: "zed".into(),
        model: model.to_string(),
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cache_read_tokens: usage.cache_read_input_tokens,
        cache_write_tokens: usage.cache_creation_input_tokens,
        cost_usd: None,
        currency: "USD".into(),
        accuracy: Accuracy::Exact,
        occurred_at,
        session_id: Some(thread_id.to_string()),
        project_slug: None,
    }
}

/// Decode a zstd or JSON blob from a path (tests / tooling).
pub fn decode_thread_blob(
    thread_id: &str,
    updated_at: &str,
    data_type: &str,
    data: &[u8],
) -> anyhow::Result<Vec<SpendEvent>> {
    decode_thread(thread_id, updated_at, data_type, data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::Ledger;
    use rusqlite::Connection;
    use tempfile::TempDir;

    fn sample_thread_json() -> String {
        serde_json::json!({
            "version": "0.3.0",
            "model": { "provider": "openrouter", "model": "qwen/qwen3.6-plus:free" },
            "request_token_usage": {
                "msg-1": {
                    "input_tokens": 100,
                    "output_tokens": 20,
                    "cache_read_input_tokens": 50
                },
                "msg-2": {
                    "input_tokens": 10,
                    "output_tokens": 5
                }
            },
            "cumulative_token_usage": {
                "input_tokens": 200,
                "output_tokens": 30,
                "cache_read_input_tokens": 50,
                "cache_creation_input_tokens": 7
            }
        })
        .to_string()
    }

    #[test]
    fn request_map_and_cumulative_remainder() {
        let thread: ZedThread = serde_json::from_str(&sample_thread_json()).unwrap();
        let events = events_from_thread("t1", "2026-04-07T15:04:44.456738+00:00", &thread);
        assert_eq!(events.len(), 3); // 2 requests + remainder
                                     // remainder: in 200-110=90, out 30-25=5, cache_read 0, cache_write 7
        let rem = events
            .iter()
            .find(|e| e.input_tokens == 90 && e.cache_write_tokens == 7)
            .expect("remainder");
        assert_eq!(rem.output_tokens, 5);
        assert!(events.iter().all(|e| e.accuracy == Accuracy::Exact));
        assert!(events.iter().all(|e| e.model == "qwen/qwen3.6-plus:free"));
    }

    #[test]
    fn empty_cumulative_uses_requests_only() {
        let json = r#"{
            "model": {"model": "deepseek-v4-pro"},
            "request_token_usage": {"a": {"input_tokens": 1, "output_tokens": 2}},
            "cumulative_token_usage": {}
        }"#;
        let thread: ZedThread = serde_json::from_str(json).unwrap();
        let events = events_from_thread("t2", "2026-01-01T00:00:00Z", &thread);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].input_tokens, 1);
    }

    #[test]
    fn zstd_roundtrip_in_sqlite() {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("threads.db");
        let json = sample_thread_json();
        let compressed = zstd::encode_all(json.as_bytes(), 3).unwrap();
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE threads (
                id TEXT PRIMARY KEY,
                summary TEXT NOT NULL DEFAULT '',
                updated_at TEXT NOT NULL,
                data_type TEXT NOT NULL,
                data BLOB NOT NULL
            );",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO threads (id, updated_at, data_type, data) VALUES (?1, ?2, 'zstd', ?3)",
            rusqlite::params!["thread-1", "2026-04-07T15:04:44Z", compressed],
        )
        .unwrap();
        drop(conn);

        let ledger = Ledger::open_in_memory().unwrap();
        let r = ZedCollector::new(db).scan(&ledger).unwrap();
        assert_eq!(r.status, CollectorStatus::Ok);
        assert_eq!(r.events.len(), 3);
        assert_eq!(r.schema_fingerprint.as_deref(), Some("zed_threads_v2"));
    }

    #[test]
    fn missing_db_ok() {
        let ledger = Ledger::open_in_memory().unwrap();
        let r = ZedCollector::new(PathBuf::from("/no/threads.db"))
            .scan(&ledger)
            .unwrap();
        assert_eq!(r.status, CollectorStatus::Ok);
        assert!(r.events.is_empty());
    }

    #[test]
    fn decode_json_data_type() {
        let events = decode_thread_blob(
            "t",
            "2026-01-01T00:00:00Z",
            "json",
            sample_thread_json().as_bytes(),
        )
        .unwrap();
        assert_eq!(events.len(), 3);
    }

    #[test]
    fn usage_keys_match_user_message_ids_without_storing_content() {
        let json = r#"{
            "model": {"model": "m"},
            "messages": [
                {"User": {"id": "msg-a", "content": [{"Text": "secret prompt"}]}},
                {"Agent": {"id": "agent-1", "content": []}}
            ],
            "request_token_usage": {
                "msg-a": {"input_tokens": 5, "output_tokens": 1}
            },
            "cumulative_token_usage": {}
        }"#;
        let thread: ZedThread = serde_json::from_str(json).unwrap();
        let user_ids: Vec<_> = thread
            .messages
            .iter()
            .filter_map(|m| match m {
                ZedMessage::User { user } => Some(user.id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(user_ids, vec!["msg-a"]);
        assert!(thread.request_token_usage.contains_key("msg-a"));
        let events = events_from_thread("t", "2026-01-01T00:00:00Z", &thread);
        assert_eq!(events.len(), 1);
        // Ledger events must not carry prompt text.
        let dumped = serde_json::to_string(&events[0]).unwrap();
        assert!(!dumped.contains("secret prompt"));
    }
}
