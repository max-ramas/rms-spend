//! Windsurf local quota collector — reads `state.vscdb` cache only.
//!
//! No network / GetUserStatus RPC. Data updates when Windsurf is launched
//! (`windsurf.settings.cachedPlanInfo` and related ItemTable keys).

use super::SpendCollector;
use crate::event::{Accuracy, CollectorResult, CollectorStatus, SpendEvent};
use crate::ledger::Ledger;
use chrono::Utc;
use rusqlite::Connection;
use serde::Deserialize;
use serde_json::Value;
use std::path::{Path, PathBuf};

const KEY_HINTS: &[&str] = &["cachedPlan", "quotaUsage", "planInfo"];

pub struct WindsurfCollector {
    pub state_dbs: Vec<PathBuf>,
}

impl WindsurfCollector {
    pub fn new(state_dbs: Vec<PathBuf>) -> Self {
        Self { state_dbs }
    }

    pub fn from_defaults() -> Self {
        Self::new(vec![
            crate::paths::windsurf_state_vscdb(),
            crate::paths::windsurf_next_state_vscdb(),
        ])
    }
}

impl SpendCollector for WindsurfCollector {
    fn id(&self) -> &str {
        "windsurf"
    }

    fn scan(&self, _ledger: &Ledger) -> anyhow::Result<CollectorResult> {
        let existing: Vec<_> = self
            .state_dbs
            .iter()
            .filter(|p| p.exists())
            .cloned()
            .collect();
        if existing.is_empty() {
            return Ok(CollectorResult {
                events: vec![],
                status: CollectorStatus::Ok,
                message: Some(format!(
                    "Windsurf state.vscdb not found (tried {})",
                    self.state_dbs
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
                schema_fingerprint: Some("windsurf_cached_plan_v1".into()),
            });
        }

        let mut events = Vec::new();
        let mut matched_keys = 0usize;
        let mut seen = std::collections::HashSet::new();
        let mut open_errors = 0usize;

        for db_path in &existing {
            let conn = match open_readonly(db_path) {
                Ok(c) => c,
                Err(e) => {
                    open_errors += 1;
                    eprintln!("warn: windsurf open {}: {e}", db_path.display());
                    continue;
                }
            };
            for hint in KEY_HINTS {
                let pattern = format!("%{hint}%");
                let mut stmt =
                    conn.prepare("SELECT key, value FROM ItemTable WHERE key LIKE ?1 LIMIT 50")?;
                let rows = stmt.query_map([&pattern], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })?;
                for row in rows {
                    let (key, value) = row?;
                    if !seen.insert(key.clone()) {
                        continue;
                    }
                    matched_keys += 1;
                    for ev in parse_plan_value(&key, &value) {
                        events.push(ev);
                    }
                }
            }
        }

        let status = if events.is_empty() && matched_keys > 0 {
            CollectorStatus::Partial
        } else if events.is_empty() && open_errors > 0 && matched_keys == 0 {
            CollectorStatus::Stale
        } else {
            CollectorStatus::Ok
        };

        Ok(CollectorResult {
            message: Some(if events.is_empty() {
                format!(
                    "no parseable Windsurf plan cache ({matched_keys} candidate key(s); local-only, no RPC)"
                )
            } else {
                format!("{} Windsurf quota snapshot(s)", events.len())
            }),
            events,
            status,
            schema_fingerprint: Some("windsurf_cached_plan_v1".into()),
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CachedPlanInfo {
    #[serde(default)]
    quota_usage: Option<QuotaUsage>,
    #[serde(default)]
    usage: Option<UsageCounters>,
    #[serde(default)]
    plan_name: Option<String>,
    #[serde(default)]
    plan: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct QuotaUsage {
    #[serde(default)]
    daily_remaining_percent: Option<f64>,
    #[serde(default)]
    weekly_remaining_percent: Option<f64>,
    #[serde(default)]
    daily_quota_remaining_percent: Option<f64>,
    #[serde(default)]
    weekly_quota_remaining_percent: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsageCounters {
    #[serde(default)]
    used_messages: Option<u64>,
    #[serde(default)]
    messages: Option<u64>,
    #[serde(default)]
    used_flow_actions: Option<u64>,
    #[serde(default)]
    flow_actions: Option<u64>,
}

/// Parse cached plan JSON into one or more quota snapshot events.
pub fn parse_plan_value(key: &str, raw: &str) -> Vec<SpendEvent> {
    let mut out = Vec::new();
    // Value may be a raw object or a JSON-encoded string.
    let value: Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(_) => return out,
    };
    let obj = match value {
        Value::String(s) => match serde_json::from_str::<Value>(&s) {
            Ok(v) => v,
            Err(_) => return out,
        },
        other => other,
    };

    if let Ok(plan) = serde_json::from_value::<CachedPlanInfo>(obj.clone()) {
        let model = plan
            .plan_name
            .or(plan.plan)
            .unwrap_or_else(|| "windsurf-plan".into());

        if let Some(q) = &plan.quota_usage {
            let daily = q
                .daily_remaining_percent
                .or(q.daily_quota_remaining_percent);
            let weekly = q
                .weekly_remaining_percent
                .or(q.weekly_quota_remaining_percent);
            if let Some(pct) = daily {
                let used = percent_used_units(pct);
                out.push(quota_event(key, "daily", &model, used));
            }
            if let Some(pct) = weekly {
                let used = percent_used_units(pct);
                out.push(quota_event(key, "weekly", &model, used));
            }
        }

        if out.is_empty() {
            if let Some(u) = &plan.usage {
                if let (Some(used), Some(limit)) = (u.used_messages, u.messages) {
                    if limit > 0 {
                        out.push(quota_event(key, "messages", &model, used.min(limit)));
                    }
                }
                if let (Some(used), Some(limit)) = (u.used_flow_actions, u.flow_actions) {
                    if limit > 0 {
                        out.push(quota_event(key, "flow_actions", &model, used.min(limit)));
                    }
                }
            }
        }
    }

    // Flat GetUserStatus-ish credit counters (hundredths).
    if out.is_empty() {
        if let Some(map) = obj.as_object() {
            let used_prompt = map
                .get("usedPromptCredits")
                .or_else(|| map.get("used_prompt_credits"))
                .and_then(|v| v.as_u64());
            let used_flex = map
                .get("usedFlexCredits")
                .or_else(|| map.get("used_flex_credits"))
                .and_then(|v| v.as_u64());
            if let Some(u) = used_prompt {
                out.push(quota_event(key, "prompt_credits", "windsurf", u / 100));
            }
            if let Some(u) = used_flex {
                out.push(quota_event(key, "flex_credits", "windsurf", u / 100));
            }
        }
    }

    out
}

fn percent_used_units(remaining_percent: f64) -> u64 {
    let used = (100.0 - remaining_percent).clamp(0.0, 100.0);
    used.round() as u64
}

fn quota_event(key: &str, kind: &str, model: &str, used_units: u64) -> SpendEvent {
    let event_id = make_event_id(key, kind, used_units);
    SpendEvent {
        event_id,
        source: "windsurf".into(),
        client: "windsurf".into(),
        model: model.to_string(),
        input_tokens: 0,
        // Quota / credit units stored as output_tokens (exact, not API tokens).
        output_tokens: used_units,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
        cost_usd: None,
        currency: "USD".into(),
        accuracy: Accuracy::Exact,
        occurred_at: Utc::now(),
        session_id: Some(format!("{key}:{kind}")),
        project_slug: Some("quota".into()),
    }
}

pub fn make_event_id(key: &str, kind: &str, used: u64) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"windsurf\0");
    hasher.update(key.as_bytes());
    hasher.update(b"\0");
    hasher.update(kind.as_bytes());
    hasher.update(b"\0");
    hasher.update(&used.to_le_bytes());
    hasher.finalize().to_hex().to_string()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::Ledger;
    use rusqlite::Connection;
    use tempfile::TempDir;

    #[test]
    fn parses_quota_percent_shape() {
        let raw = r#"{
            "planName": "Pro",
            "quotaUsage": {
                "dailyRemainingPercent": 40.0,
                "weeklyRemainingPercent": 70.0
            }
        }"#;
        let events = parse_plan_value("windsurf.settings.cachedPlanInfo", raw);
        assert_eq!(events.len(), 2);
        let daily = events.iter().find(|e| e.session_id.as_deref() == Some("windsurf.settings.cachedPlanInfo:daily")).unwrap();
        assert_eq!(daily.output_tokens, 60);
        assert_eq!(daily.model, "Pro");
    }

    #[test]
    fn parses_usage_counters_shape() {
        let raw = r#"{
            "plan": "Free",
            "usage": {
                "usedMessages": 12,
                "messages": 50,
                "usedFlowActions": 3,
                "flowActions": 10
            }
        }"#;
        let events = parse_plan_value("windsurf.settings.cachedPlanInfo", raw);
        assert_eq!(events.len(), 2);
        assert!(events.iter().any(|e| e.output_tokens == 12));
        assert!(events.iter().any(|e| e.output_tokens == 3));
    }

    #[test]
    fn parses_credit_hundredths() {
        let raw = r#"{"usedPromptCredits":4700,"usedFlexCredits":175550}"#;
        let events = parse_plan_value("planInfo", raw);
        assert_eq!(events.len(), 2);
        assert!(events.iter().any(|e| e.output_tokens == 47));
        assert!(events.iter().any(|e| e.output_tokens == 1755));
    }

    #[test]
    fn missing_db_ok() {
        let ledger = Ledger::open_in_memory().unwrap();
        let c = WindsurfCollector::new(vec![PathBuf::from("/no/state.vscdb")]);
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
            r#"CREATE TABLE ItemTable (key TEXT PRIMARY KEY, value TEXT);
               INSERT INTO ItemTable VALUES (
                 'windsurf.settings.cachedPlanInfo',
                 '{"planName":"Teams","quotaUsage":{"dailyQuotaRemainingPercent":25.0}}'
               );"#,
        )
        .unwrap();
        drop(conn);
        let ledger = Ledger::open_in_memory().unwrap();
        let c = WindsurfCollector::new(vec![db]);
        let r = c.scan(&ledger).unwrap();
        assert_eq!(r.events.len(), 1);
        assert_eq!(r.events[0].output_tokens, 75);
        assert_eq!(r.events[0].model, "Teams");
    }
}
