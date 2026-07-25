//! Qwen Code collector — `~/.qwen/projects/**/chats/*.jsonl` + `usage_record.jsonl`.

use super::util::{event_id, parse_epoch_ms, parse_rfc3339, scan_jsonl_incremental};
use super::SpendCollector;
use crate::event::{Accuracy, CollectorResult, CollectorStatus, SpendEvent};
use crate::ledger::Ledger;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

pub struct QwenCodeCollector {
    pub qwen_home: PathBuf,
}

impl QwenCodeCollector {
    pub fn new(qwen_home: PathBuf) -> Self {
        Self { qwen_home }
    }
}

impl SpendCollector for QwenCodeCollector {
    fn id(&self) -> &str {
        "qwen_code"
    }

    fn scan(&self, ledger: &Ledger) -> anyhow::Result<CollectorResult> {
        if !self.qwen_home.exists() {
            return Ok(CollectorResult {
                events: vec![],
                status: CollectorStatus::Ok,
                message: Some(format!("Qwen home not found: {}", self.qwen_home.display())),
                schema_fingerprint: Some("qwen_jsonl_v1".into()),
            });
        }

        let mut events = Vec::new();
        let mut files = 0usize;
        let mut errors = 0usize;

        let usage_record = self.qwen_home.join("usage_record.jsonl");
        if usage_record.is_file() {
            files += 1;
            match scan_usage_record(&usage_record, ledger) {
                Ok(mut batch) => events.append(&mut batch),
                Err(e) => {
                    errors += 1;
                    eprintln!("warn: skip {}: {e}", usage_record.display());
                }
            }
        }

        for entry in WalkDir::new(self.qwen_home.join("projects"))
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
            if !path.to_string_lossy().contains("/chats/") {
                continue;
            }
            files += 1;
            match scan_chat_jsonl(path, ledger) {
                Ok(mut batch) => events.append(&mut batch),
                Err(e) => {
                    errors += 1;
                    eprintln!("warn: skip {}: {e}", path.display());
                }
            }
        }

        let status = if errors > 0 && events.is_empty() && files > 0 {
            CollectorStatus::Stale
        } else if errors > 0 {
            CollectorStatus::Partial
        } else {
            CollectorStatus::Ok
        };

        Ok(CollectorResult {
            message: Some(format!(
                "Qwen Code: {files} files, {} events, {errors} errors",
                events.len()
            )),
            events,
            status,
            schema_fingerprint: Some("qwen_jsonl_v1".into()),
        })
    }
}

fn scan_chat_jsonl(path: &Path, ledger: &Ledger) -> anyhow::Result<Vec<SpendEvent>> {
    let session_id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();

    scan_jsonl_incremental(path, ledger, |line| {
        let rec: QwenLine = match serde_json::from_str(line) {
            Ok(r) => r,
            Err(_) => return Ok(None),
        };

        if rec.record_type.as_deref() != Some("system")
            || rec.subtype.as_deref() != Some("ui_telemetry")
        {
            return Ok(None);
        }
        let payload = match rec.system_payload {
            Some(p) => p,
            None => return Ok(None),
        };
        let ui = match payload.ui_event {
            Some(u) => u,
            None => return Ok(None),
        };
        if ui.event_name.as_deref() != Some("qwen-code.api_response") {
            return Ok(None);
        }
        let input = ui.input_token_count.unwrap_or(0);
        let output = ui.output_token_count.unwrap_or(0) + ui.thoughts_token_count.unwrap_or(0);
        if input == 0 && output == 0 {
            return Ok(None);
        }

        let message_id = ui
            .response_id
            .or(rec.uuid)
            .unwrap_or_else(|| blake3::hash(line.as_bytes()).to_hex().to_string());
        let model = ui.model.unwrap_or_else(|| "qwen".into());

        Ok(Some(SpendEvent {
            event_id: event_id(&["qwen_code", &session_id, &message_id]),
            source: "qwen_code".into(),
            client: "qwen-code".into(),
            model,
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: ui.cached_content_token_count.unwrap_or(0),
            cache_write_tokens: 0,
            cost_usd: None,
            currency: "USD".into(),
            accuracy: Accuracy::Exact,
            occurred_at: parse_rfc3339(rec.timestamp.as_deref()),
            session_id: rec.session_id.or(Some(session_id.clone())),
            project_slug: rec.cwd,
        }))
    })
}

fn scan_usage_record(path: &Path, ledger: &Ledger) -> anyhow::Result<Vec<SpendEvent>> {
    use std::fs::File;
    use std::io::{BufRead, BufReader, Seek, SeekFrom};

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
        let rec: UsageRecord = match serde_json::from_str(trimmed) {
            Ok(r) => r,
            Err(_) => continue,
        };
        for (model, stats) in rec.models {
            let input = stats.input_tokens.unwrap_or(0);
            let output = stats.output_tokens.unwrap_or(0) + stats.thoughts_tokens.unwrap_or(0);
            if input == 0 && output == 0 {
                continue;
            }
            let sid = rec.session_id.clone();
            events.push(SpendEvent {
                event_id: event_id(&[
                    "qwen_code",
                    "usage_record",
                    &sid,
                    &model,
                    &rec.timestamp.to_string(),
                ]),
                source: "qwen_code".into(),
                client: "qwen-code".into(),
                model,
                input_tokens: input,
                output_tokens: output,
                cache_read_tokens: stats.cached_tokens.unwrap_or(0),
                cache_write_tokens: 0,
                cost_usd: None,
                currency: "USD".into(),
                accuracy: Accuracy::Exact,
                occurred_at: parse_epoch_ms(rec.timestamp),
                session_id: Some(sid),
                project_slug: rec.project.clone(),
            });
        }
    }

    ledger.set_scan_cursor(&path_key, mtime_ms, offset)?;
    Ok(events)
}

#[derive(Debug, Deserialize)]
struct QwenLine {
    uuid: Option<String>,
    session_id: Option<String>,
    timestamp: Option<String>,
    cwd: Option<String>,
    #[serde(rename = "type")]
    record_type: Option<String>,
    subtype: Option<String>,
    #[serde(rename = "systemPayload")]
    system_payload: Option<SystemPayload>,
}

#[derive(Debug, Deserialize)]
struct SystemPayload {
    #[serde(rename = "uiEvent")]
    ui_event: Option<UiEvent>,
}

#[derive(Debug, Deserialize)]
struct UiEvent {
    #[serde(rename = "event.name")]
    event_name: Option<String>,
    response_id: Option<String>,
    model: Option<String>,
    input_token_count: Option<u64>,
    output_token_count: Option<u64>,
    cached_content_token_count: Option<u64>,
    thoughts_token_count: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct UsageRecord {
    session_id: String,
    timestamp: i64,
    project: Option<String>,
    models: std::collections::BTreeMap<String, ModelStats>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelStats {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cached_tokens: Option<u64>,
    thoughts_tokens: Option<u64>,
}
