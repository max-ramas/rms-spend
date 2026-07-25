//! Shared helpers for collectors (event ids, incremental JSONL cursors).

use crate::event::SpendEvent;
use crate::ledger::Ledger;
use chrono::{DateTime, TimeZone, Utc};
use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::Path;

pub fn event_id(parts: &[&str]) -> String {
    let mut hasher = blake3::Hasher::new();
    for part in parts {
        hasher.update(part.as_bytes());
    }
    hasher.finalize().to_hex().to_string()
}

pub fn parse_rfc3339(s: Option<&str>) -> DateTime<Utc> {
    parse_rfc3339_or(s, Utc::now())
}

pub fn parse_rfc3339_or(s: Option<&str>, fallback: DateTime<Utc>) -> DateTime<Utc> {
    s.and_then(|t| DateTime::parse_from_rfc3339(t).ok())
        .map(|t| t.with_timezone(&Utc))
        .unwrap_or(fallback)
}

pub fn parse_epoch_ms(ms: i64) -> DateTime<Utc> {
    Utc.timestamp_millis_opt(ms)
        .single()
        .unwrap_or_else(Utc::now)
}

/// Incrementally scan a JSONL file using ledger cursors (mtime + byte offset).
pub fn scan_jsonl_incremental<F>(
    path: &Path,
    ledger: &Ledger,
    parse_line: F,
) -> anyhow::Result<Vec<SpendEvent>>
where
    F: FnMut(&str) -> anyhow::Result<Option<SpendEvent>>,
{
    scan_jsonl_incremental_keyed(path, &path.to_string_lossy(), ledger, parse_line)
}

/// Like [`scan_jsonl_incremental`], but with an explicit cursor key (e.g. path + schema tag).
pub fn scan_jsonl_incremental_keyed<F>(
    path: &Path,
    cursor_key: &str,
    ledger: &Ledger,
    mut parse_line: F,
) -> anyhow::Result<Vec<SpendEvent>>
where
    F: FnMut(&str) -> anyhow::Result<Option<SpendEvent>>,
{
    let meta = std::fs::metadata(path)?;
    let mtime_ms = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);

    let file_len = meta.len() as i64;
    // JSONL session logs are append-only. Their mtime changes on every append, so
    // requiring an identical mtime makes every incremental scan restart at byte 0.
    // Keep the saved offset while the file is at least that long; a shorter file
    // was truncated/replaced and must be scanned from the beginning.
    let start_offset = match ledger.get_scan_cursor(cursor_key)? {
        Some((_prev_mtime, prev_off)) if prev_off >= 0 && prev_off <= file_len => prev_off,
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
        if let Some(ev) = parse_line(trimmed)? {
            events.push(ev);
        }
    }

    ledger.set_scan_cursor(cursor_key, mtime_ms, offset)?;
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Accuracy, SpendEvent};
    use chrono::Utc;
    use std::io::Write;

    fn event(line: &str) -> SpendEvent {
        SpendEvent {
            event_id: line.to_string(),
            source: "test".into(),
            client: "test".into(),
            model: "test".into(),
            input_tokens: 1,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            cost_usd: None,
            currency: "USD".into(),
            accuracy: Accuracy::Exact,
            occurred_at: Utc::now(),
            session_id: None,
            project_slug: None,
        }
    }

    #[test]
    fn appended_jsonl_reads_only_new_lines_even_when_mtime_changes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rollout.jsonl");
        std::fs::write(&path, "first\n").unwrap();
        let ledger = Ledger::open_in_memory().unwrap();

        let first = scan_jsonl_incremental(&path, &ledger, |line| Ok(Some(event(line)))).unwrap();
        assert_eq!(
            first
                .iter()
                .map(|e| e.event_id.as_str())
                .collect::<Vec<_>>(),
            ["first"]
        );

        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(file, "second").unwrap();
        file.flush().unwrap();

        let appended =
            scan_jsonl_incremental(&path, &ledger, |line| Ok(Some(event(line)))).unwrap();
        assert_eq!(
            appended
                .iter()
                .map(|e| e.event_id.as_str())
                .collect::<Vec<_>>(),
            ["second"]
        );
    }

    #[test]
    fn truncated_jsonl_restarts_from_beginning() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rollout.jsonl");
        std::fs::write(&path, "a much longer first record\n").unwrap();
        let ledger = Ledger::open_in_memory().unwrap();
        let _ = scan_jsonl_incremental(&path, &ledger, |line| Ok(Some(event(line)))).unwrap();

        std::fs::write(&path, "new\n").unwrap();
        let rescanned =
            scan_jsonl_incremental(&path, &ledger, |line| Ok(Some(event(line)))).unwrap();
        assert_eq!(
            rescanned
                .iter()
                .map(|e| e.event_id.as_str())
                .collect::<Vec<_>>(),
            ["new"]
        );
    }
}
