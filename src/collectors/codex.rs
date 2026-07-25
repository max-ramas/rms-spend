//! OpenAI Codex collector — reads `~/.codex/sessions/**/rollout-*.jsonl`.

use super::util::{event_id, parse_rfc3339, scan_jsonl_incremental};
use super::SpendCollector;
use crate::event::{Accuracy, CollectorResult, CollectorStatus, SpendEvent};
use crate::ledger::Ledger;
use serde::Deserialize;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

pub struct CodexCollector {
    pub sessions_dir: PathBuf,
}

impl CodexCollector {
    pub fn new(sessions_dir: PathBuf) -> Self {
        Self { sessions_dir }
    }
}

impl SpendCollector for CodexCollector {
    fn id(&self) -> &str {
        "codex"
    }

    fn scan(&self, ledger: &Ledger) -> anyhow::Result<CollectorResult> {
        if !self.sessions_dir.exists() {
            return Ok(CollectorResult {
                events: vec![],
                status: CollectorStatus::Ok,
                message: Some(format!(
                    "Codex sessions dir not found: {}",
                    self.sessions_dir.display()
                )),
                schema_fingerprint: Some("codex_jsonl_v1".into()),
            });
        }

        let mut events = Vec::new();
        let mut files = 0usize;
        let mut errors = 0usize;

        for entry in WalkDir::new(&self.sessions_dir)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !name.starts_with("rollout-") || !name.ends_with(".jsonl") {
                continue;
            }
            files += 1;
            match scan_rollout(path, ledger) {
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
                "Codex: {files} rollout files, {} events, {errors} errors",
                events.len()
            )),
            events,
            status,
            schema_fingerprint: Some("codex_jsonl_v1".into()),
        })
    }
}

fn scan_rollout(path: &Path, ledger: &Ledger) -> anyhow::Result<Vec<SpendEvent>> {
    let session_id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();
    let mut model = "codex".to_string();
    let mut cwd: Option<String> = None;

    // Incremental scans start after the previously consumed byte offset, while
    // session metadata lives on the first line. Seed stable session attributes
    // before reading only the appended token events.
    if let Ok(file) = std::fs::File::open(path) {
        if let Some(Ok(line)) = BufReader::new(file).lines().next() {
            if let Ok(rec) = serde_json::from_str::<CodexLine>(&line) {
                if rec.record_type.as_deref() == Some("session_meta") {
                    if let Some(payload) = rec.payload {
                        if let Some(provider) = payload.model_provider {
                            model = provider;
                        }
                        cwd = payload.cwd;
                    }
                }
            }
        }
    }

    scan_jsonl_incremental(path, ledger, |line| {
        let rec: CodexLine = match serde_json::from_str(line) {
            Ok(r) => r,
            Err(_) => return Ok(None),
        };

        if rec.record_type.as_deref() == Some("session_meta") {
            if let Some(payload) = rec.payload {
                if let Some(p) = payload.model_provider {
                    model = p;
                }
                if let Some(c) = payload.cwd {
                    cwd = Some(c);
                }
                if let Some(s) = payload.session_id {
                    // rollout stem may differ from logical session id
                    let _ = s;
                }
            }
            return Ok(None);
        }

        if rec.record_type.as_deref() != Some("event_msg") {
            return Ok(None);
        }
        let payload = match rec.payload {
            Some(p) => p,
            None => return Ok(None),
        };
        if payload.inner_type.as_deref() != Some("token_count") {
            return Ok(None);
        }
        let info = match payload.info {
            Some(i) => i,
            None => return Ok(None),
        };
        let usage = match info.last_token_usage {
            Some(u) => u,
            None => return Ok(None),
        };
        if usage.input_tokens.unwrap_or(0) == 0 && usage.output_tokens.unwrap_or(0) == 0 {
            return Ok(None);
        }

        let ts = rec.timestamp.as_deref().unwrap_or("");
        let input = usage.input_tokens.unwrap_or(0);
        let output = usage.output_tokens.unwrap_or(0) + usage.reasoning_output_tokens.unwrap_or(0);

        Ok(Some(SpendEvent {
            event_id: event_id(&[
                "codex",
                &session_id,
                ts,
                &input.to_string(),
                &output.to_string(),
            ]),
            source: "codex".into(),
            client: "codex".into(),
            model: model.clone(),
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: usage.cached_input_tokens.unwrap_or(0),
            cache_write_tokens: usage.cache_write_input_tokens.unwrap_or(0),
            cost_usd: None,
            currency: "USD".into(),
            accuracy: Accuracy::Exact,
            occurred_at: parse_rfc3339(rec.timestamp.as_deref()),
            session_id: Some(session_id.clone()),
            project_slug: cwd.clone(),
        }))
    })
}

#[derive(Debug, Deserialize)]
struct CodexLine {
    timestamp: Option<String>,
    #[serde(rename = "type")]
    record_type: Option<String>,
    payload: Option<CodexPayload>,
}

#[derive(Debug, Deserialize)]
struct CodexPayload {
    #[serde(rename = "type")]
    inner_type: Option<String>,
    session_id: Option<String>,
    cwd: Option<String>,
    model_provider: Option<String>,
    info: Option<TokenInfo>,
}

#[derive(Debug, Deserialize)]
struct TokenInfo {
    last_token_usage: Option<TokenUsage>,
}

#[derive(Debug, Deserialize)]
struct TokenUsage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cached_input_tokens: Option<u64>,
    cache_write_input_tokens: Option<u64>,
    reasoning_output_tokens: Option<u64>,
}
