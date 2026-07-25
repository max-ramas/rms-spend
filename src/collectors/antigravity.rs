//! Google Antigravity collector — `~/.gemini/antigravity/conversations/*.db`.
//!
//! Token counts live in protobuf blobs inside `gen_metadata.data`. We extract the
//! nested usage message that matches Antigravity's observed wire shape
//! (field 6 == 24, field 2 = prompt, field 3 = output, field 5 = cache/thoughts).

use super::util::event_id;
use super::SpendCollector;
use crate::event::{Accuracy, CollectorResult, CollectorStatus, SpendEvent};
use crate::ledger::Ledger;
use chrono::{DateTime, TimeZone, Utc};
use rusqlite::Connection;
use std::path::{Path, PathBuf};

pub struct AntigravityCollector {
    pub conversations_dir: PathBuf,
}

impl AntigravityCollector {
    pub fn new(conversations_dir: PathBuf) -> Self {
        Self { conversations_dir }
    }
}

impl SpendCollector for AntigravityCollector {
    fn id(&self) -> &str {
        "antigravity"
    }

    fn scan(&self, _ledger: &Ledger) -> anyhow::Result<CollectorResult> {
        if !self.conversations_dir.exists() {
            return Ok(CollectorResult {
                events: vec![],
                status: CollectorStatus::Ok,
                message: Some(format!(
                    "Antigravity conversations not found: {}",
                    self.conversations_dir.display()
                )),
                schema_fingerprint: Some("antigravity_gen_metadata_v2".into()),
            });
        }

        let mut events = Vec::new();
        let mut files = 0usize;
        let mut errors = 0usize;

        let entries = std::fs::read_dir(&self.conversations_dir)?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("db") {
                continue;
            }
            files += 1;
            match scan_conversation_db(&path) {
                Ok(mut batch) => events.append(&mut batch),
                Err(e) => {
                    errors += 1;
                    eprintln!("warn: skip antigravity {}: {e}", path.display());
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
                "Antigravity: {files} conversations, {} events, {errors} errors",
                events.len()
            )),
            events,
            status,
            schema_fingerprint: Some("antigravity_gen_metadata_v2".into()),
        })
    }
}

fn scan_conversation_db(path: &Path) -> anyhow::Result<Vec<SpendEvent>> {
    let conv_id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();
    let fallback_at = file_mtime(path).unwrap_or_else(Utc::now);

    let conn = Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;

    let mut stmt = conn.prepare("SELECT idx, data FROM gen_metadata ORDER BY idx")?;
    let rows = stmt.query_map([], |row| {
        let idx: i64 = row.get(0)?;
        let data: Vec<u8> = row.get(1)?;
        Ok((idx, data))
    })?;

    let mut events = Vec::new();
    for row in rows {
        let (idx, data) = row?;
        let Some((input, output, cache)) = extract_usage(&data) else {
            continue;
        };
        if input == 0 && output == 0 && cache == 0 {
            continue;
        }
        let model = extract_model(&data).unwrap_or_else(|| "gemini".into());
        let occurred_at = extract_occurred_at(&data).unwrap_or(fallback_at);
        events.push(SpendEvent {
            event_id: event_id(&["antigravity", &conv_id, &idx.to_string()]),
            source: "antigravity".into(),
            client: "antigravity".into(),
            model,
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: cache,
            cache_write_tokens: 0,
            cost_usd: None,
            currency: "USD".into(),
            accuracy: Accuracy::Exact,
            occurred_at,
            session_id: Some(conv_id.clone()),
            project_slug: None,
        });
    }
    Ok(events)
}

/// Per-step time from gen_metadata blob: protobuf Timestamp seconds, then ISO-8601.
fn extract_occurred_at(blob: &[u8]) -> Option<DateTime<Utc>> {
    let mut best: Option<i64> = None;
    walk_proto(blob, 0, &mut |fields| {
        let d: std::collections::BTreeMap<u64, u64> = fields
            .iter()
            .filter_map(|(f, w, v)| {
                if *w == 0 {
                    if let ProtoVal::Varint(n) = v {
                        return Some((*f, *n));
                    }
                }
                None
            })
            .collect();
        // google.protobuf.Timestamp: field 1 = seconds, optional field 2 = nanos.
        if let Some(&secs) = d.get(&1) {
            if is_plausible_unix_secs(secs) {
                best = Some(match best {
                    Some(cur) => cur.min(secs as i64),
                    None => secs as i64,
                });
            }
        }
        false
    });
    if let Some(secs) = best {
        return Utc.timestamp_opt(secs, 0).single();
    }
    extract_iso_timestamp(blob)
}

fn is_plausible_unix_secs(secs: u64) -> bool {
    // 2024-01-01 .. 2035-01-01 roughly
    (1_704_067_200..=2_051_222_400).contains(&secs)
}

fn extract_iso_timestamp(blob: &[u8]) -> Option<DateTime<Utc>> {
    let s = String::from_utf8_lossy(blob);
    // Match RFC3339-ish substrings embedded in protobuf ASCII.
    let bytes = s.as_bytes();
    let mut i = 0;
    while i + 20 < bytes.len() {
        if bytes[i] == b'2'
            && bytes[i + 1] == b'0'
            && bytes.get(i + 4) == Some(&b'-')
            && bytes.get(i + 7) == Some(&b'-')
            && bytes.get(i + 10) == Some(&b'T')
        {
            let end = (i + 40).min(bytes.len());
            let slice = &s[i..end];
            // Truncate at first non-timestamp char after the date portion.
            let mut cut = slice.len();
            for (j, c) in slice.char_indices().skip(10) {
                if !(c.is_ascii_digit()
                    || c == '-'
                    || c == ':'
                    || c == 'T'
                    || c == 'Z'
                    || c == '+'
                    || c == '.')
                {
                    cut = j;
                    break;
                }
            }
            let candidate = &slice[..cut];
            if let Ok(dt) = DateTime::parse_from_rfc3339(candidate) {
                return Some(dt.with_timezone(&Utc));
            }
            // Try appending Z if timezone stripped.
            if let Ok(dt) = DateTime::parse_from_rfc3339(&format!("{candidate}Z")) {
                return Some(dt.with_timezone(&Utc));
            }
        }
        i += 1;
    }
    None
}

fn file_mtime(path: &Path) -> Option<DateTime<Utc>> {
    let meta = std::fs::metadata(path).ok()?;
    let modified = meta.modified().ok()?;
    let secs = modified
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs() as i64;
    Utc.timestamp_opt(secs, 0).single()
}

fn extract_model(blob: &[u8]) -> Option<String> {
    let s = String::from_utf8_lossy(blob);
    // Prefer full model ids embedded as ascii.
    for needle in ["gemini-", "claude-"] {
        if let Some(start) = s.find(needle) {
            let rest = &s[start..];
            let end = rest
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '_'))
                .unwrap_or(rest.len());
            let id = &rest[..end];
            if id.len() > needle.len() {
                return Some(id.to_string());
            }
        }
    }
    None
}

/// Extract (prompt, output, cache/thoughts) from nested protobuf.
fn extract_usage(blob: &[u8]) -> Option<(u64, u64, u64)> {
    let mut found = None;
    walk_proto(blob, 0, &mut |fields| {
        let d: std::collections::BTreeMap<u64, u64> = fields
            .iter()
            .filter_map(|(f, w, v)| {
                if *w == 0 {
                    if let ProtoVal::Varint(n) = v {
                        return Some((*f, *n));
                    }
                }
                None
            })
            .collect();
        // Observed Antigravity usage signature.
        if d.get(&6) == Some(&24) && d.contains_key(&2) && d.contains_key(&3) {
            let prompt = *d.get(&2).unwrap_or(&0);
            let output = *d.get(&3).unwrap_or(&0);
            let cache = *d.get(&5).unwrap_or(&0);
            if prompt < 20_000_000 && output < 5_000_000 && cache < 50_000_000 {
                found = Some((prompt, output, cache));
                return true; // stop
            }
        }
        false
    });
    found
}

enum ProtoVal {
    Varint(u64),
    Bytes(Vec<u8>),
}

fn walk_proto(data: &[u8], depth: usize, on_msg: &mut dyn FnMut(&[(u64, u8, ProtoVal)]) -> bool) {
    if depth > 5 || data.is_empty() {
        return;
    }
    let Some(fields) = parse_fields(data) else {
        return;
    };
    if on_msg(&fields) {
        return;
    }
    for (_f, w, v) in &fields {
        if *w == 2 {
            if let ProtoVal::Bytes(payload) = v {
                if payload.len() >= 6 && payload.len() < 8000 {
                    walk_proto(payload, depth + 1, on_msg);
                }
            }
        }
    }
}

fn parse_fields(data: &[u8]) -> Option<Vec<(u64, u8, ProtoVal)>> {
    let mut i = 0usize;
    let mut out = Vec::new();
    while i < data.len() {
        let (key, ni) = read_varint(data, i)?;
        if ni == i {
            break;
        }
        i = ni;
        let field = key >> 3;
        let wt = (key & 7) as u8;
        if field == 0 {
            break;
        }
        match wt {
            0 => {
                let (val, ni) = read_varint(data, i)?;
                i = ni;
                out.push((field, 0, ProtoVal::Varint(val)));
            }
            2 => {
                let (ln, ni) = read_varint(data, i)?;
                i = ni;
                let len = ln as usize;
                if i + len > data.len() {
                    break;
                }
                let payload = data[i..i + len].to_vec();
                i += len;
                out.push((field, 2, ProtoVal::Bytes(payload)));
            }
            5 => {
                if i + 4 > data.len() {
                    break;
                }
                i += 4;
            }
            1 => {
                if i + 8 > data.len() {
                    break;
                }
                i += 8;
            }
            _ => break,
        }
    }
    Some(out)
}

fn read_varint(data: &[u8], mut i: usize) -> Option<(u64, usize)> {
    let mut x: u64 = 0;
    let mut s: u32 = 0;
    while i < data.len() {
        let c = data[i];
        i += 1;
        x |= u64::from(c & 0x7f) << s;
        if c & 0x80 == 0 {
            return Some((x, i));
        }
        s += 7;
        if s > 70 {
            return None;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_signature_extracts_prompt_output() {
        // Minimal crafted message: field6=24, field2=1000, field3=50
        // key for field 2 wt0 = (2<<3)|0 = 16 = 0x10, value 1000 = 0xe8 0x07
        // field 3 = 0x18, value 50 = 0x32
        // field 6 = 0x30, value 24 = 0x18
        let msg = vec![
            0x10, 0xe8, 0x07, // field2 = 1000
            0x18, 0x32, // field3 = 50
            0x30, 0x18, // field6 = 24
        ];
        // wrap as length-delimited field 1 so walker enters nested
        let mut outer = vec![0x0a, msg.len() as u8];
        outer.extend_from_slice(&msg);
        assert_eq!(extract_usage(&outer), Some((1000, 50, 0)));
    }

    #[test]
    fn extract_occurred_at_from_protobuf_timestamp() {
        // Nested Timestamp: field 1 = 1780594301 (2026-06-04T17:31:41Z)
        // encode varint 1780594301
        fn enc_varint(mut v: u64) -> Vec<u8> {
            let mut out = Vec::new();
            while v >= 0x80 {
                out.push((v as u8) | 0x80);
                v >>= 7;
            }
            out.push(v as u8);
            out
        }
        let mut ts = vec![0x08]; // field1 wt0
        ts.extend(enc_varint(1_780_594_301));
        let mut outer = vec![0x0a, ts.len() as u8];
        outer.extend_from_slice(&ts);
        let dt = extract_occurred_at(&outer).unwrap();
        assert_eq!(dt.timestamp(), 1_780_594_301);
    }

    #[test]
    fn extract_occurred_at_iso_fallback() {
        let blob = b"xxxx2026-06-04T17:31:45Zyyyy";
        let dt = extract_occurred_at(blob).unwrap();
        assert_eq!(dt.timestamp(), 1_780_594_305);
    }

    #[test]
    fn extract_occurred_at_prefers_earliest_timestamp() {
        fn enc_varint(mut v: u64) -> Vec<u8> {
            let mut out = Vec::new();
            while v >= 0x80 {
                out.push((v as u8) | 0x80);
                v >>= 7;
            }
            out.push(v as u8);
            out
        }
        let mut msg = Vec::new();
        // field 2 = later ts message
        let mut later = vec![0x08];
        later.extend(enc_varint(1_780_594_309));
        msg.push(0x12);
        msg.push(later.len() as u8);
        msg.extend_from_slice(&later);
        // field 3 = earlier ts message
        let mut earlier = vec![0x08];
        earlier.extend(enc_varint(1_780_594_301));
        msg.push(0x1a);
        msg.push(earlier.len() as u8);
        msg.extend_from_slice(&earlier);
        let dt = extract_occurred_at(&msg).unwrap();
        assert_eq!(dt.timestamp(), 1_780_594_301);
    }
}
