//! Gemini CLI / Antigravity collector — scans `~/.gemini/**/*.jsonl` for `usageMetadata`.

use super::util::{event_id, parse_rfc3339, scan_jsonl_incremental};
use super::SpendCollector;
use crate::event::{Accuracy, CollectorResult, CollectorStatus, SpendEvent};
use crate::ledger::Ledger;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

pub struct GeminiCliCollector {
    pub gemini_home: PathBuf,
}

impl GeminiCliCollector {
    pub fn new(gemini_home: PathBuf) -> Self {
        Self { gemini_home }
    }
}

impl SpendCollector for GeminiCliCollector {
    fn id(&self) -> &str {
        "gemini_cli"
    }

    fn scan(&self, ledger: &Ledger) -> anyhow::Result<CollectorResult> {
        if !self.gemini_home.exists() {
            return Ok(CollectorResult {
                events: vec![],
                status: CollectorStatus::Ok,
                message: Some(format!(
                    "Gemini home not found: {}",
                    self.gemini_home.display()
                )),
                schema_fingerprint: Some("gemini_jsonl_v1".into()),
            });
        }

        let mut events = Vec::new();
        let mut files = 0usize;
        let mut errors = 0usize;

        for entry in WalkDir::new(&self.gemini_home)
            .into_iter()
            .filter_entry(|e| {
                let p = e.path().to_string_lossy();
                !p.contains("node_modules")
                    && !p.contains("/plugins/cache/")
                    && !p.contains("antigravity-browser-profile")
                    && !p.contains("/antigravity/conversations/")
                    && !p.contains("/antigravity/brain/")
                    && !p.contains("/antigravity-ide/")
            })
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            files += 1;
            match scan_gemini_jsonl(path, ledger) {
                Ok(mut batch) => events.append(&mut batch),
                Err(e) => {
                    errors += 1;
                    eprintln!("warn: skip {}: {e}", path.display());
                }
            }
        }

        let message = if events.is_empty() && files == 0 {
            "Gemini: no jsonl usage logs found (Antigravity IDE may not export token counts locally)"
                .into()
        } else {
            format!(
                "Gemini: {files} jsonl files, {} events, {errors} errors",
                events.len()
            )
        };

        let status = if errors > 0 && events.is_empty() {
            CollectorStatus::Partial
        } else {
            CollectorStatus::Ok
        };

        Ok(CollectorResult {
            message: Some(message),
            events,
            status,
            schema_fingerprint: Some("gemini_jsonl_v1".into()),
        })
    }
}

fn scan_gemini_jsonl(path: &Path, ledger: &Ledger) -> anyhow::Result<Vec<SpendEvent>> {
    let file_id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("file")
        .to_string();

    scan_jsonl_incremental(path, ledger, |line| {
        if !line.contains("usageMetadata") && !line.contains("usage_metadata") {
            return Ok(None);
        }
        let v: serde_json::Value = serde_json::from_str(line)?;
        let usage = find_usage_metadata(&v);
        let usage = match usage {
            Some(u) => u,
            None => return Ok(None),
        };
        let input = usage.prompt_token_count.unwrap_or(0);
        let output =
            usage.candidates_token_count.unwrap_or(0) + usage.thoughts_token_count.unwrap_or(0);
        if input == 0 && output == 0 {
            return Ok(None);
        }

        let model = find_model(&v).unwrap_or_else(|| "gemini".into());
        let ts = find_timestamp(&v);
        let line_id = blake3::hash(line.as_bytes()).to_hex().to_string();

        Ok(Some(SpendEvent {
            event_id: event_id(&["gemini_cli", &file_id, &line_id]),
            source: "gemini_cli".into(),
            client: "gemini".into(),
            model,
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: usage.cached_content_token_count.unwrap_or(0),
            cache_write_tokens: 0,
            cost_usd: None,
            currency: "USD".into(),
            accuracy: Accuracy::Exact,
            occurred_at: parse_rfc3339(ts.as_deref()),
            session_id: Some(file_id.clone()),
            project_slug: None,
        }))
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsageMetadata {
    prompt_token_count: Option<u64>,
    candidates_token_count: Option<u64>,
    cached_content_token_count: Option<u64>,
    thoughts_token_count: Option<u64>,
}

fn find_usage_metadata(v: &serde_json::Value) -> Option<UsageMetadata> {
    match v {
        serde_json::Value::Object(map) => {
            if let Some(u) = map.get("usageMetadata").or(map.get("usage_metadata")) {
                return serde_json::from_value(u.clone()).ok();
            }
            for val in map.values() {
                if let Some(found) = find_usage_metadata(val) {
                    return Some(found);
                }
            }
        }
        serde_json::Value::Array(arr) => {
            for val in arr {
                if let Some(found) = find_usage_metadata(val) {
                    return Some(found);
                }
            }
        }
        _ => {}
    }
    None
}

fn find_model(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::Object(map) => {
            if let Some(serde_json::Value::String(m)) = map.get("model").or(map.get("modelId")) {
                return Some(m.clone());
            }
            for val in map.values() {
                if let Some(m) = find_model(val) {
                    return Some(m);
                }
            }
        }
        serde_json::Value::Array(arr) => {
            for val in arr {
                if let Some(m) = find_model(val) {
                    return Some(m);
                }
            }
        }
        _ => {}
    }
    None
}

fn find_timestamp(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::Object(map) => {
            for key in ["created_at", "timestamp", "createdAt"] {
                if let Some(serde_json::Value::String(s)) = map.get(key) {
                    return Some(s.clone());
                }
            }
            for val in map.values() {
                if let Some(s) = find_timestamp(val) {
                    return Some(s);
                }
            }
        }
        serde_json::Value::Array(arr) => {
            for val in arr {
                if let Some(s) = find_timestamp(val) {
                    return Some(s);
                }
            }
        }
        _ => {}
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::Ledger;
    use tempfile::TempDir;

    #[test]
    fn missing_home_ok() {
        let ledger = Ledger::open_in_memory().unwrap();
        let r = GeminiCliCollector::new(PathBuf::from("/no/gemini"))
            .scan(&ledger)
            .unwrap();
        assert_eq!(r.status, CollectorStatus::Ok);
    }

    #[test]
    fn parses_usage_metadata() {
        let dir = TempDir::new().unwrap();
        let f = dir.path().join("session.jsonl");
        std::fs::write(
            &f,
            r#"{"model":"gemini-2.5-pro","created_at":"2026-02-01T12:00:00Z","usageMetadata":{"promptTokenCount":100,"candidatesTokenCount":20,"cachedContentTokenCount":5}}
"#,
        )
        .unwrap();
        // collector walks gemini home recursively — put file under home
        let ledger = Ledger::open_in_memory().unwrap();
        let r = GeminiCliCollector::new(dir.path().to_path_buf())
            .scan(&ledger)
            .unwrap();
        assert_eq!(r.events.len(), 1);
        assert_eq!(r.events[0].input_tokens, 100);
        assert_eq!(r.events[0].output_tokens, 20);
        assert_eq!(r.events[0].cache_read_tokens, 5);
    }
}
