//! VS Code Chat (Copilot / agent) collector — `workspaceStorage/*/chatSessions/*.jsonl`.

use super::util::{event_id, parse_rfc3339_or, scan_jsonl_incremental_keyed};
use super::SpendCollector;
use crate::event::{Accuracy, CollectorResult, CollectorStatus, SpendEvent};
use crate::ledger::Ledger;
use chrono::{DateTime, TimeZone, Utc};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

pub struct VscodeCopilotCollector {
    pub workspace_storage: PathBuf,
}

impl VscodeCopilotCollector {
    pub fn new(workspace_storage: PathBuf) -> Self {
        Self { workspace_storage }
    }
}

impl SpendCollector for VscodeCopilotCollector {
    fn id(&self) -> &str {
        "vscode_copilot"
    }

    fn scan(&self, ledger: &Ledger) -> anyhow::Result<CollectorResult> {
        if !self.workspace_storage.exists() {
            return Ok(CollectorResult {
                events: vec![],
                status: CollectorStatus::Ok,
                message: Some(format!(
                    "VS Code workspaceStorage not found: {}",
                    self.workspace_storage.display()
                )),
                schema_fingerprint: Some("vscode_chat_v3".into()),
            });
        }

        let mut events = Vec::new();
        let mut files = 0usize;
        let mut errors = 0usize;

        for entry in WalkDir::new(&self.workspace_storage)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            if path
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                != Some("chatSessions")
            {
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            files += 1;
            match scan_chat_session(path, ledger) {
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
                "VS Code Chat: {files} sessions, {} events, {errors} errors",
                events.len()
            )),
            events,
            status,
            schema_fingerprint: Some("vscode_chat_v3".into()),
        })
    }
}

fn file_mtime(path: &Path) -> DateTime<Utc> {
    std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|d| Utc.timestamp_opt(d.as_secs() as i64, 0).single())
        .unwrap_or_else(Utc::now)
}

fn scan_chat_session(path: &Path, ledger: &Ledger) -> anyhow::Result<Vec<SpendEvent>> {
    let session_id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();
    let fallback_ts = file_mtime(path);

    let cursor_key = format!("{}#vscode_chat_v3", path.display());
    scan_jsonl_incremental_keyed(path, &cursor_key, ledger, |line| {
        if !line.contains("\"promptTokens\"") {
            return Ok(None);
        }
        let patch: CopilotPatch = match serde_json::from_str(line) {
            Ok(p) => p,
            Err(_) => return Ok(None),
        };
        if patch.kind != Some(1) {
            return Ok(None);
        }
        let keys = match patch.k {
            Some(k) => k,
            None => return Ok(None),
        };
        if keys.len() != 3 {
            return Ok(None);
        }
        if keys[0].as_str() != Some("requests") || keys[2].as_str() != Some("result") {
            return Ok(None);
        }
        let req_idx = keys[1].to_string();
        let result: CopilotResult = match patch.v {
            Some(v) => v,
            None => return Ok(None),
        };
        let meta = match result.metadata {
            Some(m) => m,
            None => return Ok(None),
        };
        let input = meta.prompt_tokens.unwrap_or(0);
        let output = meta.output_tokens.unwrap_or(0);
        if input == 0 && output == 0 {
            return Ok(None);
        }

        let model = meta
            .resolved_model
            .or_else(|| {
                meta.tool_call_rounds
                    .as_ref()
                    .and_then(|rounds| rounds.iter().rev().find_map(|r| r.model_id.clone()))
            })
            .or(meta.model_id)
            .or(meta.model_family)
            .unwrap_or_else(|| "vscode-chat".into());

        let occurred_at = meta
            .tool_call_rounds
            .as_ref()
            .and_then(|rounds| rounds.iter().rev().find_map(|r| r.timestamp.as_ref()))
            .map(|v| timestamp_from_value(Some(v), fallback_ts))
            .unwrap_or(fallback_ts);

        Ok(Some(SpendEvent {
            event_id: event_id(&["vscode_copilot", &session_id, &req_idx]),
            source: "vscode_copilot".into(),
            client: "vscode-chat".into(),
            model,
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            cost_usd: None,
            currency: "USD".into(),
            accuracy: Accuracy::Exact,
            occurred_at,
            session_id: Some(session_id.clone()),
            project_slug: None,
        }))
    })
}

#[derive(Debug, Deserialize)]
struct CopilotPatch {
    kind: Option<i64>,
    k: Option<Vec<serde_json::Value>>,
    v: Option<CopilotResult>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CopilotResult {
    metadata: Option<CopilotMetadata>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CopilotMetadata {
    prompt_tokens: Option<u64>,
    output_tokens: Option<u64>,
    model_id: Option<String>,
    model_family: Option<String>,
    resolved_model: Option<String>,
    tool_call_rounds: Option<Vec<ToolCallRound>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ToolCallRound {
    model_id: Option<String>,
    #[serde(default)]
    timestamp: Option<serde_json::Value>,
}

fn timestamp_from_value(v: Option<&serde_json::Value>, fallback: DateTime<Utc>) -> DateTime<Utc> {
    let Some(v) = v else {
        return fallback;
    };
    match v {
        serde_json::Value::String(s) => parse_rfc3339_or(Some(s), fallback),
        serde_json::Value::Number(n) => {
            let ms = n.as_i64().or_else(|| n.as_f64().map(|f| f as i64));
            match ms {
                Some(ms) if ms > 1_000_000_000_000 => {
                    Utc.timestamp_millis_opt(ms).single().unwrap_or(fallback)
                }
                Some(secs) if secs > 1_000_000_000 => {
                    Utc.timestamp_opt(secs, 0).single().unwrap_or(fallback)
                }
                _ => fallback,
            }
        }
        _ => fallback,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::Ledger;
    use tempfile::TempDir;

    #[test]
    fn missing_storage_ok() {
        let ledger = Ledger::open_in_memory().unwrap();
        let r = VscodeCopilotCollector::new(PathBuf::from("/no/ws"))
            .scan(&ledger)
            .unwrap();
        assert_eq!(r.status, CollectorStatus::Ok);
    }

    #[test]
    fn parses_chat_session_patch() {
        let dir = TempDir::new().unwrap();
        let sess = dir
            .path()
            .join("ws1")
            .join("chatSessions");
        std::fs::create_dir_all(&sess).unwrap();
        std::fs::write(
            sess.join("chat1.jsonl"),
            r#"{"kind":1,"k":["requests",0,"result"],"v":{"metadata":{"promptTokens":15,"outputTokens":4,"resolvedModel":"gpt-4.1","toolCallRounds":[{"timestamp":1700000000000}]}}}
"#,
        )
        .unwrap();
        let ledger = Ledger::open_in_memory().unwrap();
        let r = VscodeCopilotCollector::new(dir.path().to_path_buf())
            .scan(&ledger)
            .unwrap();
        assert_eq!(r.events.len(), 1);
        assert_eq!(r.events[0].input_tokens, 15);
        assert_eq!(r.events[0].output_tokens, 4);
        assert_eq!(r.events[0].model, "gpt-4.1");
    }
}
