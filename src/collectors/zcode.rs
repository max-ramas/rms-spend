//! ZCode collector — reads `~/.zcode/cli/rollout/model-io-*.jsonl`.

use super::util::{event_id, parse_rfc3339, scan_jsonl_incremental};
use super::SpendCollector;
use crate::event::{Accuracy, CollectorResult, CollectorStatus, SpendEvent};
use crate::ledger::Ledger;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

pub struct ZCodeCollector {
    pub rollout_dir: PathBuf,
}

impl ZCodeCollector {
    pub fn new(rollout_dir: PathBuf) -> Self {
        Self { rollout_dir }
    }
}

impl SpendCollector for ZCodeCollector {
    fn id(&self) -> &str {
        "zcode"
    }

    fn scan(&self, ledger: &Ledger) -> anyhow::Result<CollectorResult> {
        if !self.rollout_dir.exists() {
            return Ok(CollectorResult {
                events: vec![],
                status: CollectorStatus::Ok,
                message: Some(format!(
                    "ZCode rollout dir not found: {}",
                    self.rollout_dir.display()
                )),
                schema_fingerprint: Some("zcode_rollout_v1".into()),
            });
        }

        let mut events = Vec::new();
        let mut files = 0usize;
        let mut errors = 0usize;

        for entry in WalkDir::new(&self.rollout_dir)
            .max_depth(1)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !name.starts_with("model-io-") || !name.ends_with(".jsonl") {
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
                "ZCode: {files} rollout files, {} events, {errors} errors",
                events.len()
            )),
            events,
            status,
            schema_fingerprint: Some("zcode_rollout_v1".into()),
        })
    }
}

fn scan_rollout(path: &Path, ledger: &Ledger) -> anyhow::Result<Vec<SpendEvent>> {
    let session_id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();

    scan_jsonl_incremental(path, ledger, |line| {
        let rec: ZCodeLine = match serde_json::from_str(line) {
            Ok(r) => r,
            Err(_) => return Ok(None),
        };
        let usage = match rec.response.and_then(|r| r.usage) {
            Some(u) => u,
            None => return Ok(None),
        };
        let input = usage.input_tokens.unwrap_or(0);
        let output = usage.output_tokens.unwrap_or(0);
        if input == 0 && output == 0 {
            return Ok(None);
        }

        let request_id = rec
            .request_id
            .unwrap_or_else(|| blake3::hash(line.as_bytes()).to_hex().to_string());
        let model = rec
            .model
            .and_then(|m| m.model_id)
            .unwrap_or_else(|| "zcode".into());

        Ok(Some(SpendEvent {
            event_id: event_id(&["zcode", &session_id, &request_id]),
            source: "zcode".into(),
            client: "zcode".into(),
            model,
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: usage.cache_read_tokens.unwrap_or(0),
            cache_write_tokens: usage.cache_write_tokens.unwrap_or(0),
            cost_usd: None,
            currency: "USD".into(),
            accuracy: Accuracy::Exact,
            occurred_at: parse_rfc3339(rec.completed_at.as_deref()),
            session_id: Some(session_id.clone()),
            project_slug: None,
        }))
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ZCodeLine {
    completed_at: Option<String>,
    request_id: Option<String>,
    model: Option<ModelMeta>,
    response: Option<ResponseBlock>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelMeta {
    model_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResponseBlock {
    usage: Option<UsageBlock>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsageBlock {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_read_tokens: Option<u64>,
    cache_write_tokens: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::Ledger;
    use tempfile::TempDir;

    #[test]
    fn missing_dir_ok() {
        let ledger = Ledger::open_in_memory().unwrap();
        let r = ZCodeCollector::new(PathBuf::from("/no/zcode"))
            .scan(&ledger)
            .unwrap();
        assert_eq!(r.status, CollectorStatus::Ok);
    }

    #[test]
    fn parses_model_io() {
        let dir = TempDir::new().unwrap();
        let f = dir.path().join("model-io-1.jsonl");
        std::fs::write(
            &f,
            r#"{"completedAt":"2026-03-01T00:00:00Z","requestId":"r1","model":{"modelId":"gpt-4.1"},"response":{"usage":{"inputTokens":11,"outputTokens":7,"cacheReadTokens":1}}}
"#,
        )
        .unwrap();
        let ledger = Ledger::open_in_memory().unwrap();
        let r = ZCodeCollector::new(dir.path().to_path_buf())
            .scan(&ledger)
            .unwrap();
        assert_eq!(r.events.len(), 1);
        assert_eq!(r.events[0].input_tokens, 11);
        assert_eq!(r.events[0].output_tokens, 7);
        assert_eq!(r.events[0].model, "gpt-4.1");
    }
}
