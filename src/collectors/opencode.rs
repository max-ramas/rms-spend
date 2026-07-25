//! OpenCode collector — reads `~/.local/share/opencode/opencode.db` assistant messages.

use super::util::{event_id, parse_epoch_ms};
use super::SpendCollector;
use crate::event::{Accuracy, CollectorResult, CollectorStatus, SpendEvent};
use crate::ledger::Ledger;
use rusqlite::Connection;
use serde::Deserialize;
use std::path::PathBuf;

const EXPECTED_COLS: &[&str] = &["id", "session_id", "time_created", "data"];

pub struct OpenCodeCollector {
    pub db_path: PathBuf,
}

impl OpenCodeCollector {
    pub fn new(db_path: PathBuf) -> Self {
        Self { db_path }
    }
}

impl SpendCollector for OpenCodeCollector {
    fn id(&self) -> &str {
        "opencode"
    }

    fn scan(&self, _ledger: &Ledger) -> anyhow::Result<CollectorResult> {
        if !self.db_path.exists() {
            return Ok(CollectorResult {
                events: vec![],
                status: CollectorStatus::Ok,
                message: Some(format!("OpenCode DB not found: {}", self.db_path.display())),
                schema_fingerprint: Some("opencode_v1".into()),
            });
        }

        let conn =
            Connection::open_with_flags(&self.db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        if !table_has_cols(&conn, "message", EXPECTED_COLS)? {
            return Ok(CollectorResult {
                events: vec![],
                status: CollectorStatus::Stale,
                message: Some("OpenCode message table schema changed".into()),
                schema_fingerprint: Some("opencode_stale".into()),
            });
        }

        let mut stmt = conn.prepare(
            "SELECT m.id, m.session_id, m.time_created, m.data, s.directory
             FROM message m
             JOIN session s ON s.id = m.session_id
             WHERE m.data LIKE '%\"tokens\"%'",
        )?;

        let mut events = Vec::new();
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?;

        for row in rows {
            let (msg_id, session_id, created_ms, data, directory) = match row {
                Ok(r) => r,
                Err(_) => continue,
            };
            match parse_message(&msg_id, &session_id, created_ms, &data, &directory) {
                Ok(Some(ev)) => events.push(ev),
                Ok(None) => {}
                Err(e) => {
                    eprintln!("warn: skip opencode message {msg_id}: {e}");
                }
            }
        }

        Ok(CollectorResult {
            message: Some(format!("OpenCode: {} usage events", events.len())),
            events,
            status: CollectorStatus::Ok,
            schema_fingerprint: Some("opencode_v1".into()),
        })
    }
}

#[derive(Debug, Deserialize)]
struct MsgData {
    role: Option<String>,
    model_id: Option<String>,
    #[serde(rename = "modelID")]
    model_id_alt: Option<String>,
    cost: Option<f64>,
    tokens: Option<TokenBlock>,
    time: Option<TimeBlock>,
}

#[derive(Debug, Deserialize)]
struct TokenBlock {
    input: Option<i64>,
    output: Option<i64>,
    reasoning: Option<i64>,
    cache: Option<CacheBlock>,
}

#[derive(Debug, Deserialize)]
struct CacheBlock {
    read: Option<i64>,
    write: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct TimeBlock {
    completed: Option<i64>,
    created: Option<i64>,
}

fn parse_message(
    msg_id: &str,
    session_id: &str,
    created_ms: i64,
    data: &str,
    directory: &str,
) -> anyhow::Result<Option<SpendEvent>> {
    let parsed: MsgData = serde_json::from_str(data)?;
    if parsed.role.as_deref() != Some("assistant") {
        return Ok(None);
    }
    let tokens = match parsed.tokens {
        Some(t) => t,
        None => return Ok(None),
    };
    let input = tokens.input.unwrap_or(0).max(0) as u64;
    let output = (tokens.output.unwrap_or(0) + tokens.reasoning.unwrap_or(0)).max(0) as u64;
    if input == 0 && output == 0 {
        return Ok(None);
    }

    let occurred_ms = parsed
        .time
        .and_then(|t| t.completed.or(t.created))
        .unwrap_or(created_ms);
    let model = parsed
        .model_id
        .or(parsed.model_id_alt)
        .unwrap_or_else(|| "unknown".into());

    Ok(Some(SpendEvent {
        event_id: event_id(&["opencode", session_id, msg_id]),
        source: "opencode".into(),
        client: "opencode".into(),
        model,
        input_tokens: input,
        output_tokens: output,
        cache_read_tokens: tokens
            .cache
            .as_ref()
            .and_then(|c| c.read)
            .unwrap_or(0)
            .max(0) as u64,
        cache_write_tokens: tokens
            .cache
            .as_ref()
            .and_then(|c| c.write)
            .unwrap_or(0)
            .max(0) as u64,
        cost_usd: parsed.cost.filter(|c| *c > 0.0),
        currency: "USD".into(),
        accuracy: Accuracy::Exact,
        occurred_at: parse_epoch_ms(occurred_ms),
        session_id: Some(session_id.to_string()),
        project_slug: Some(directory.to_string()),
    }))
}

fn table_has_cols(conn: &Connection, table: &str, expected: &[&str]) -> anyhow::Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let cols: Vec<String> = stmt
        .query_map([], |row| row.get(1))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(expected.iter().all(|c| cols.iter().any(|col| col == c)))
}
