//! Claude Code collector — JSONL is the canonical source.
//!
//! eventId = blake3("claude_code" || session_id || message_id)
//! stats-cache.json is NOT ingested (fallback/cross-check only, unused in v1).

use super::SpendCollector;
use crate::event::{Accuracy, CollectorResult, CollectorStatus, SpendEvent};
use crate::ledger::Ledger;
use chrono::{DateTime, TimeZone, Utc};
use serde::Deserialize;
use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

pub struct ClaudeCodeCollector {
    pub projects_dir: PathBuf,
}

impl ClaudeCodeCollector {
    pub fn new(projects_dir: PathBuf) -> Self {
        Self { projects_dir }
    }
}

impl SpendCollector for ClaudeCodeCollector {
    fn id(&self) -> &str {
        "claude_code"
    }

    fn scan(&self, ledger: &Ledger) -> anyhow::Result<CollectorResult> {
        if !self.projects_dir.exists() {
            return Ok(CollectorResult {
                events: vec![],
                status: CollectorStatus::Ok,
                message: Some(format!(
                    "Claude projects dir not found: {}",
                    self.projects_dir.display()
                )),
                schema_fingerprint: Some("claude_jsonl_v1".into()),
            });
        }

        let mut events = Vec::new();
        let mut files_scanned = 0usize;
        let mut parse_errors = 0usize;

        for entry in WalkDir::new(&self.projects_dir)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            files_scanned += 1;
            match scan_jsonl_file(path, ledger, &self.projects_dir) {
                Ok(mut batch) => events.append(&mut batch),
                Err(e) => {
                    parse_errors += 1;
                    eprintln!("warn: skip {}: {e}", path.display());
                }
            }
        }

        let status = if parse_errors > 0 && events.is_empty() && files_scanned > 0 {
            CollectorStatus::Stale
        } else if parse_errors > 0 {
            CollectorStatus::Partial
        } else {
            CollectorStatus::Ok
        };

        Ok(CollectorResult {
            message: Some(format!(
                "scanned {files_scanned} jsonl files, {} events, {parse_errors} file errors",
                events.len()
            )),
            events,
            status,
            schema_fingerprint: Some("claude_jsonl_v1".into()),
        })
    }
}

fn scan_jsonl_file(
    path: &Path,
    ledger: &Ledger,
    projects_root: &Path,
) -> anyhow::Result<Vec<SpendEvent>> {
    let meta = std::fs::metadata(path)?;
    let mtime_ms = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let path_key = path.to_string_lossy().to_string();

    let start_offset = match ledger.get_scan_cursor(&path_key)? {
        Some((prev_mtime, prev_off)) if prev_mtime == mtime_ms => prev_off,
        _ => 0,
    };

    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    reader.seek(SeekFrom::Start(start_offset as u64))?;

    let session_id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();
    let project_slug = path
        .strip_prefix(projects_root)
        .ok()
        .and_then(|p| p.components().next())
        .map(|c| c.as_os_str().to_string_lossy().to_string());

    let mut events = Vec::new();
    let mut offset = start_offset;

    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            break;
        }
        offset += n as i64;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match parse_assistant_line(trimmed, &session_id, project_slug.as_deref()) {
            Ok(Some(ev)) => events.push(ev),
            Ok(None) => {}
            Err(_) => {
                // Skip malformed lines; do not fail the whole file.
            }
        }
    }

    ledger.set_scan_cursor(&path_key, mtime_ms, offset)?;
    Ok(events)
}

#[derive(Debug, Deserialize)]
struct JsonlRecord {
    #[serde(rename = "type")]
    record_type: Option<String>,
    uuid: Option<String>,
    #[serde(alias = "messageId")]
    message_id: Option<String>,
    timestamp: Option<String>,
    message: Option<MessageBody>,
}

#[derive(Debug, Deserialize)]
struct MessageBody {
    id: Option<String>,
    model: Option<String>,
    role: Option<String>,
    usage: Option<UsageBlock>,
}

#[derive(Debug, Deserialize)]
struct UsageBlock {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
}

fn parse_assistant_line(
    line: &str,
    session_id: &str,
    project_slug: Option<&str>,
) -> anyhow::Result<Option<SpendEvent>> {
    let rec: JsonlRecord = serde_json::from_str(line)?;
    let msg = match rec.message {
        Some(m) => m,
        None => return Ok(None),
    };

    // Only assistant messages with usage.
    let role = msg.role.as_deref().unwrap_or("");
    let rtype = rec.record_type.as_deref().unwrap_or("");
    if role != "assistant" && rtype != "assistant" {
        return Ok(None);
    }
    let usage = match msg.usage {
        Some(u) => u,
        None => return Ok(None),
    };

    let message_id = msg.id.or(rec.message_id).or(rec.uuid).unwrap_or_else(|| {
        // Fallback: hash of the line itself (stable for same line).
        blake3::hash(line.as_bytes()).to_hex().to_string()
    });

    let event_id = make_event_id(session_id, &message_id);
    let model = msg.model.unwrap_or_else(|| "unknown".into());
    let occurred_at = parse_timestamp(rec.timestamp.as_deref());

    Ok(Some(SpendEvent {
        event_id,
        source: "claude_code".into(),
        client: "claude-code".into(),
        model,
        input_tokens: usage.input_tokens.unwrap_or(0),
        output_tokens: usage.output_tokens.unwrap_or(0),
        cache_read_tokens: usage.cache_read_input_tokens.unwrap_or(0),
        cache_write_tokens: usage.cache_creation_input_tokens.unwrap_or(0),
        cost_usd: None,
        currency: "USD".into(),
        accuracy: Accuracy::Exact,
        occurred_at,
        session_id: Some(session_id.to_string()),
        project_slug: project_slug.map(|s| s.to_string()),
    }))
}

pub fn make_event_id(session_id: &str, message_id: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"claude_code\0");
    hasher.update(session_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(message_id.as_bytes());
    hasher.finalize().to_hex().to_string()
}

fn parse_timestamp(raw: Option<&str>) -> DateTime<Utc> {
    if let Some(s) = raw {
        if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
            return dt.with_timezone(&Utc);
        }
        // millis
        if let Ok(ms) = s.parse::<i64>() {
            if let Some(dt) = Utc.timestamp_millis_opt(ms).single() {
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
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn parses_assistant_usage_and_dedupes() {
        let dir = TempDir::new().unwrap();
        let proj = dir.path().join("my-project");
        std::fs::create_dir_all(&proj).unwrap();
        let session = proj.join("sess-1.jsonl");
        let mut f = File::create(&session).unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","uuid":"msg-1","timestamp":"2026-07-20T10:00:00Z","message":{{"id":"msg-1","role":"assistant","model":"claude-sonnet-4-20250514","usage":{{"input_tokens":100,"output_tokens":50,"cache_read_input_tokens":10,"cache_creation_input_tokens":5}}}}}}"#
        )
        .unwrap();
        writeln!(f, r#"{{"type":"user","message":{{"role":"user"}}}}"#).unwrap();

        let ledger = Ledger::open_in_memory().unwrap();
        let c = ClaudeCodeCollector::new(dir.path().to_path_buf());
        let r1 = c.scan(&ledger).unwrap();
        assert_eq!(r1.events.len(), 1);
        assert_eq!(r1.events[0].input_tokens, 100);
        assert_eq!(r1.events[0].cache_read_tokens, 10);
        assert_eq!(r1.status, CollectorStatus::Ok);

        ledger.upsert_events(&r1.events).unwrap();
        // Second scan: cursor advanced → 0 new events
        let r2 = c.scan(&ledger).unwrap();
        assert_eq!(r2.events.len(), 0);
    }

    #[test]
    fn event_id_stable() {
        let a = make_event_id("s1", "m1");
        let b = make_event_id("s1", "m1");
        let c = make_event_id("s1", "m2");
        assert_eq!(a, b);
        assert_ne!(a, c);
    }
}
