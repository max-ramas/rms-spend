//! RMS Memory GUI ai-runs collector.
//!
//! Preference order per run directory:
//! 1. `usage.jsonl` (append-only audit lines — survives proposal edits)
//! 2. `manifest.json` usage fields
//! 3. `proposal.json` usage / estimatedUsage
//!
//! Also scans `~/.rms-memory/spend-raw/rms_memory.jsonl` for orphaned audit lines.
//! eventId = blake3("rms_memory" || run_id || "usage")

use super::SpendCollector;
use crate::event::{Accuracy, CollectorResult, CollectorStatus, SpendEvent};
use crate::ledger::Ledger;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub struct RmsMemoryCollector {
    pub ai_runs_dir: PathBuf,
    pub raw_audit_path: Option<PathBuf>,
}

impl RmsMemoryCollector {
    pub fn new(ai_runs_dir: PathBuf) -> Self {
        let raw = dirs::home_dir().map(|h| h.join(".rms-memory/spend-raw/rms_memory.jsonl"));
        Self {
            ai_runs_dir,
            raw_audit_path: raw,
        }
    }

    pub fn with_raw_audit(ai_runs_dir: PathBuf, raw_audit_path: PathBuf) -> Self {
        Self {
            ai_runs_dir,
            raw_audit_path: Some(raw_audit_path),
        }
    }
}

impl SpendCollector for RmsMemoryCollector {
    fn id(&self) -> &str {
        "rms_memory"
    }

    fn scan(&self, _ledger: &Ledger) -> anyhow::Result<CollectorResult> {
        let mut events = Vec::new();
        let mut errors = 0usize;
        let mut seen_runs: HashSet<String> = HashSet::new();

        if self.ai_runs_dir.exists() {
            let entries = match std::fs::read_dir(&self.ai_runs_dir) {
                Ok(e) => e,
                Err(e) => {
                    return Ok(CollectorResult {
                        events: vec![],
                        status: CollectorStatus::Stale,
                        message: Some(format!("cannot read ai-runs: {e}")),
                        schema_fingerprint: Some("rms_memory_usage_v2".into()),
                    });
                }
            };

            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                match parse_run_dir(&path) {
                    Ok(Some(ev)) => {
                        seen_runs
                            .insert(ev.session_id.clone().unwrap_or_else(|| ev.event_id.clone()));
                        events.push(ev);
                    }
                    Ok(None) => {}
                    Err(e) => {
                        errors += 1;
                        eprintln!("warn: {}: {e}", path.display());
                    }
                }
            }
        }

        if let Some(raw) = &self.raw_audit_path {
            if raw.exists() {
                match parse_raw_audit(raw, &seen_runs) {
                    Ok(extra) => events.extend(extra),
                    Err(e) => {
                        errors += 1;
                        eprintln!("warn: {}: {e}", raw.display());
                    }
                }
            }
        }

        let status = if !self.ai_runs_dir.exists() && events.is_empty() {
            CollectorStatus::Ok
        } else if errors > 0 {
            CollectorStatus::Partial
        } else {
            CollectorStatus::Ok
        };

        let message = if !self.ai_runs_dir.exists() && events.is_empty() {
            Some(format!(
                "ai-runs dir not found: {}",
                self.ai_runs_dir.display()
            ))
        } else {
            Some(format!(
                "scanned ai-runs, {} events, {errors} errors",
                events.len()
            ))
        };

        Ok(CollectorResult {
            message,
            events,
            status,
            schema_fingerprint: Some("rms_memory_usage_v2".into()),
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Proposal {
    run_id: Option<String>,
    created_at: Option<String>,
    provider_id: Option<String>,
    model_id: Option<String>,
    scope: Option<String>,
    usage: Option<Usage>,
    estimated_usage: Option<Usage>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManifestUsage {
    run_id: Option<String>,
    created_at: Option<String>,
    provider_id: Option<String>,
    model_id: Option<String>,
    scope: Option<String>,
    usage: Option<Usage>,
    estimated_usage: Option<Usage>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Usage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuditLine {
    run_id: String,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    provider_id: Option<String>,
    #[serde(default)]
    model_id: Option<String>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    usage: Option<Usage>,
    #[serde(default)]
    estimated_usage: Option<Usage>,
    #[serde(default)]
    accuracy: Option<String>,
}

fn parse_run_dir(dir: &Path) -> anyhow::Result<Option<SpendEvent>> {
    let usage_jsonl = dir.join("usage.jsonl");
    if usage_jsonl.exists() {
        if let Some(ev) = parse_usage_jsonl_latest(&usage_jsonl)? {
            return Ok(Some(ev));
        }
    }

    let manifest = dir.join("manifest.json");
    if manifest.exists() {
        if let Some(ev) = parse_manifest(&manifest)? {
            return Ok(Some(ev));
        }
    }

    let proposal = dir.join("proposal.json");
    if proposal.exists() {
        return parse_proposal(&proposal);
    }
    Ok(None)
}

fn parse_usage_jsonl_latest(path: &Path) -> anyhow::Result<Option<SpendEvent>> {
    let raw = std::fs::read_to_string(path)?;
    let mut last: Option<SpendEvent> = None;
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let row: AuditLine = serde_json::from_str(line)?;
        if let Some(ev) = audit_to_event(&row)? {
            last = Some(ev);
        }
    }
    Ok(last)
}

fn parse_raw_audit(path: &Path, seen: &HashSet<String>) -> anyhow::Result<Vec<SpendEvent>> {
    let raw = std::fs::read_to_string(path)?;
    let mut out = Vec::new();
    let mut best: std::collections::HashMap<String, SpendEvent> = std::collections::HashMap::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let row: AuditLine = match serde_json::from_str(line) {
            Ok(r) => r,
            Err(_) => continue,
        };
        if seen.contains(&row.run_id) {
            continue;
        }
        if let Some(ev) = audit_to_event(&row)? {
            best.insert(row.run_id.clone(), ev);
        }
    }
    out.extend(best.into_values());
    Ok(out)
}

fn audit_to_event(row: &AuditLine) -> anyhow::Result<Option<SpendEvent>> {
    let (usage, accuracy) = match &row.usage {
        Some(u) if u.input_tokens.unwrap_or(0) > 0 || u.output_tokens.unwrap_or(0) > 0 => {
            let acc = match row.accuracy.as_deref() {
                Some("estimated") => Accuracy::Estimated,
                _ => Accuracy::Exact,
            };
            (u, acc)
        }
        _ => match &row.estimated_usage {
            Some(u) => (u, Accuracy::Estimated),
            None => return Ok(None),
        },
    };
    Ok(Some(SpendEvent {
        event_id: make_event_id(&row.run_id),
        source: "rms_memory".into(),
        client: "rms-memory-gui".into(),
        model: row.model_id.clone().unwrap_or_else(|| "unknown".into()),
        input_tokens: usage.input_tokens.unwrap_or(0),
        output_tokens: usage.output_tokens.unwrap_or(0),
        cache_read_tokens: 0,
        cache_write_tokens: 0,
        cost_usd: None,
        currency: "USD".into(),
        accuracy,
        occurred_at: parse_ts(row.created_at.as_deref()),
        session_id: Some(row.run_id.clone()),
        project_slug: row.scope.clone().or_else(|| row.provider_id.clone()),
    }))
}

fn parse_manifest(path: &Path) -> anyhow::Result<Option<SpendEvent>> {
    let raw = std::fs::read_to_string(path)?;
    let m: ManifestUsage = serde_json::from_str(&raw)?;
    let run_id = m
        .run_id
        .or_else(|| {
            path.parent()
                .and_then(|d| d.file_name())
                .map(|n| n.to_string_lossy().to_string())
        })
        .unwrap_or_else(|| "unknown".into());

    let (usage, accuracy) = match m.usage {
        Some(ref u) if u.input_tokens.unwrap_or(0) > 0 || u.output_tokens.unwrap_or(0) > 0 => {
            (u.clone(), Accuracy::Exact)
        }
        _ => match m.estimated_usage {
            Some(u) => (u, Accuracy::Estimated),
            None => return Ok(None),
        },
    };

    Ok(Some(SpendEvent {
        event_id: make_event_id(&run_id),
        source: "rms_memory".into(),
        client: "rms-memory-gui".into(),
        model: m.model_id.unwrap_or_else(|| "unknown".into()),
        input_tokens: usage.input_tokens.unwrap_or(0),
        output_tokens: usage.output_tokens.unwrap_or(0),
        cache_read_tokens: 0,
        cache_write_tokens: 0,
        cost_usd: None,
        currency: "USD".into(),
        accuracy,
        occurred_at: parse_ts(m.created_at.as_deref()),
        session_id: Some(run_id),
        project_slug: m.scope.or(m.provider_id),
    }))
}

fn parse_proposal(path: &Path) -> anyhow::Result<Option<SpendEvent>> {
    let raw = std::fs::read_to_string(path)?;
    let p: Proposal = serde_json::from_str(&raw)?;
    let run_id = p
        .run_id
        .or_else(|| {
            path.parent()
                .and_then(|d| d.file_name())
                .map(|n| n.to_string_lossy().to_string())
        })
        .unwrap_or_else(|| "unknown".into());

    let (usage, accuracy) = match p.usage {
        Some(u) if u.input_tokens.unwrap_or(0) > 0 || u.output_tokens.unwrap_or(0) > 0 => {
            (u, Accuracy::Exact)
        }
        _ => match p.estimated_usage {
            Some(u) => (u, Accuracy::Estimated),
            None => return Ok(None),
        },
    };

    Ok(Some(SpendEvent {
        event_id: make_event_id(&run_id),
        source: "rms_memory".into(),
        client: "rms-memory-gui".into(),
        model: p.model_id.unwrap_or_else(|| "unknown".into()),
        input_tokens: usage.input_tokens.unwrap_or(0),
        output_tokens: usage.output_tokens.unwrap_or(0),
        cache_read_tokens: 0,
        cache_write_tokens: 0,
        cost_usd: None,
        currency: "USD".into(),
        accuracy,
        occurred_at: parse_ts(p.created_at.as_deref()),
        session_id: Some(run_id),
        project_slug: p.scope.or(p.provider_id),
    }))
}

fn parse_ts(raw: Option<&str>) -> DateTime<Utc> {
    raw.and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(Utc::now)
}

pub fn make_event_id(run_id: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"rms_memory\0");
    hasher.update(run_id.as_bytes());
    hasher.update(b"\0usage");
    hasher.finalize().to_hex().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::Ledger;
    use tempfile::TempDir;

    #[test]
    fn prefers_usage_jsonl() {
        let dir = TempDir::new().unwrap();
        let run = dir.path().join("run-abc");
        std::fs::create_dir_all(&run).unwrap();
        std::fs::write(
            run.join("proposal.json"),
            r#"{"runId":"run-abc","usage":{"inputTokens":1,"outputTokens":1}}"#,
        )
        .unwrap();
        std::fs::write(
            run.join("usage.jsonl"),
            r#"{"runId":"run-abc","createdAt":"2026-07-20T12:00:00Z","modelId":"claude-sonnet-4","usage":{"inputTokens":200,"outputTokens":40},"accuracy":"exact"}
"#,
        )
        .unwrap();

        let ledger = Ledger::open_in_memory().unwrap();
        let c = RmsMemoryCollector {
            ai_runs_dir: dir.path().to_path_buf(),
            raw_audit_path: None,
        };
        let r = c.scan(&ledger).unwrap();
        assert_eq!(r.events.len(), 1);
        assert_eq!(r.events[0].input_tokens, 200);
        assert_eq!(r.events[0].accuracy, Accuracy::Exact);
    }

    #[test]
    fn parses_proposal_usage() {
        let dir = TempDir::new().unwrap();
        let run = dir.path().join("run-abc");
        std::fs::create_dir_all(&run).unwrap();
        std::fs::write(
            run.join("proposal.json"),
            r#"{"runId":"run-abc","createdAt":"2026-07-20T12:00:00Z","providerId":"anthropic","modelId":"claude-sonnet-4","usage":{"inputTokens":200,"outputTokens":40}}"#,
        )
        .unwrap();

        let ledger = Ledger::open_in_memory().unwrap();
        let c = RmsMemoryCollector {
            ai_runs_dir: dir.path().to_path_buf(),
            raw_audit_path: None,
        };
        let r = c.scan(&ledger).unwrap();
        assert_eq!(r.events.len(), 1);
        assert_eq!(r.events[0].input_tokens, 200);
        assert_eq!(r.events[0].accuracy, Accuracy::Exact);
    }
}
