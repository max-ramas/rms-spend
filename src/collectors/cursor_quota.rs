//! Cursor quota collector (`cursor_quota`) — separate from line-proxy `cursor_proxy`.
//!
//! Scans Cursor `state.vscdb` for internal usage/quota keys. When a stable schema is
//! found, emits `accuracy=exact` quota snapshots (not token counts). Until then returns
//! Ok with zero events — never blocks other collectors.

use super::SpendCollector;
use crate::event::{Accuracy, CollectorResult, CollectorStatus, SpendEvent};
use crate::ledger::Ledger;
use chrono::Utc;
use rusqlite::Connection;
use serde::Deserialize;
use std::path::PathBuf;

/// Key substrings that may carry quota-ish payloads in current Cursor builds.
const QUOTA_KEY_HINTS: &[&str] = &[
    "premium",
    "quota",
    "usageLimit",
    "fastRequests",
    "subscription",
];

pub struct CursorQuotaCollector {
    pub state_vscdb: PathBuf,
}

impl CursorQuotaCollector {
    pub fn new(state_vscdb: PathBuf) -> Self {
        Self { state_vscdb }
    }

    pub fn from_defaults() -> Self {
        Self::new(crate::paths::cursor_state_vscdb())
    }
}

impl SpendCollector for CursorQuotaCollector {
    fn id(&self) -> &str {
        "cursor_quota"
    }

    fn scan(&self, _ledger: &Ledger) -> anyhow::Result<CollectorResult> {
        if !self.state_vscdb.exists() {
            return Ok(CollectorResult {
                events: vec![],
                status: CollectorStatus::Ok,
                message: Some(format!(
                    "Cursor state.vscdb not found: {}",
                    self.state_vscdb.display()
                )),
                schema_fingerprint: Some("cursor_quota_v0".into()),
            });
        }

        let conn = match open_readonly(&self.state_vscdb) {
            Ok(c) => c,
            Err(e) => {
                return Ok(CollectorResult {
                    events: vec![],
                    status: CollectorStatus::Stale,
                    message: Some(format!("Cannot open Cursor state DB: {e}")),
                    schema_fingerprint: None,
                });
            }
        };

        let mut events = Vec::new();
        let mut matched_keys = 0usize;
        let mut seen_keys = std::collections::HashSet::new();
        for hint in QUOTA_KEY_HINTS {
            let pattern = format!("%{hint}%");
            let mut stmt =
                conn.prepare("SELECT key, value FROM ItemTable WHERE key LIKE ?1 LIMIT 50")?;
            let rows = stmt.query_map([&pattern], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (key, value) = row?;
                if !seen_keys.insert(key.clone()) {
                    continue;
                }
                matched_keys += 1;
                if let Some(ev) = parse_quota_value(&key, &value) {
                    events.push(ev);
                }
            }
        }

        let status = if events.is_empty() && matched_keys > 0 {
            CollectorStatus::Partial
        } else {
            CollectorStatus::Ok
        };

        Ok(CollectorResult {
            message: Some(if events.is_empty() {
                format!("no parseable quota snapshots ({matched_keys} candidate key(s) scanned)")
            } else {
                format!("{} quota snapshot(s)", events.len())
            }),
            events,
            status,
            schema_fingerprint: Some("cursor_quota_v0".into()),
        })
    }
}

#[derive(Debug, Deserialize)]
struct QuotaPayload {
    #[serde(default)]
    remaining: Option<u64>,
    #[serde(default)]
    used: Option<u64>,
    #[serde(default)]
    limit: Option<u64>,
    #[serde(rename = "fastRequestsRemaining", default)]
    fast_requests_remaining: Option<u64>,
    #[serde(rename = "premiumRequestsRemaining", default)]
    premium_requests_remaining: Option<u64>,
}

fn parse_quota_value(key: &str, raw: &str) -> Option<SpendEvent> {
    let payload: QuotaPayload = serde_json::from_str(raw).ok()?;
    let remaining = payload
        .remaining
        .or(payload.fast_requests_remaining)
        .or(payload.premium_requests_remaining)
        .or(match (payload.limit, payload.used) {
            (Some(l), Some(u)) if l >= u => Some(l - u),
            _ => None,
        })?;

    let event_id = make_event_id(key, remaining);
    Some(SpendEvent {
        event_id,
        source: "cursor_quota".into(),
        client: "cursor".into(),
        model: "cursor-quota".into(),
        // Quota remainder stored as output_tokens (exact quota units, not API tokens).
        input_tokens: 0,
        output_tokens: remaining,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
        cost_usd: None,
        currency: "USD".into(),
        accuracy: Accuracy::Exact,
        occurred_at: Utc::now(),
        session_id: Some(key.to_string()),
        project_slug: Some("quota".into()),
    })
}

pub fn make_event_id(key: &str, remaining: u64) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"cursor_quota\0");
    hasher.update(key.as_bytes());
    hasher.update(b"\0");
    hasher.update(&remaining.to_le_bytes());
    hasher.finalize().to_hex().to_string()
}

fn open_readonly(path: &std::path::Path) -> anyhow::Result<Connection> {
    let uri = format!("file:{}?mode=ro", path.display());
    let conn = Connection::open_with_flags(
        &uri,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )?;
    let _ = conn.busy_timeout(std::time::Duration::from_millis(500));
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::Ledger;
    use rusqlite::Connection;
    use tempfile::TempDir;

    #[test]
    fn parses_quota_json() {
        let ev = parse_quota_value("cursor.premium", r#"{"premiumRequestsRemaining":42}"#).unwrap();
        assert_eq!(ev.output_tokens, 42);
        assert_eq!(ev.accuracy, Accuracy::Exact);
    }

    #[test]
    fn missing_db_ok() {
        let ledger = Ledger::open_in_memory().unwrap();
        let c = CursorQuotaCollector::new(PathBuf::from("/no/state.vscdb"));
        let r = c.scan(&ledger).unwrap();
        assert_eq!(r.status, CollectorStatus::Ok);
        assert!(r.events.is_empty());
    }

    #[test]
    fn scans_item_table() {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("state.vscdb");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE ItemTable (key TEXT PRIMARY KEY, value TEXT);
             INSERT INTO ItemTable VALUES ('cursor.subscription.quota', '{\"remaining\":99}');",
        )
        .unwrap();
        drop(conn);
        let ledger = Ledger::open_in_memory().unwrap();
        let c = CursorQuotaCollector::new(db);
        let r = c.scan(&ledger).unwrap();
        assert_eq!(r.events.len(), 1);
        assert_eq!(r.events[0].output_tokens, 99);
    }
}
