use crate::event::{Accuracy, CollectorHealth, CollectorStatus, SpendEvent};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;

#[derive(Debug, Clone, Default)]
pub struct EventQuery<'a> {
    pub source: Option<&'a str>,
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
    pub limit: Option<usize>,
}

pub struct Ledger {
    conn: Connection,
}

impl Ledger {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn =
            Connection::open(path).with_context(|| format!("open ledger {}", path.display()))?;
        let ledger = Self { conn };
        ledger.migrate()?;
        Ok(ledger)
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let ledger = Self { conn };
        ledger.migrate()?;
        Ok(ledger)
    }

    fn migrate(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            PRAGMA journal_mode = WAL;
            PRAGMA foreign_keys = ON;

            CREATE TABLE IF NOT EXISTS schema_meta (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS events (
                event_id TEXT PRIMARY KEY,
                source TEXT NOT NULL,
                client TEXT NOT NULL,
                model TEXT NOT NULL,
                input_tokens INTEGER NOT NULL DEFAULT 0,
                output_tokens INTEGER NOT NULL DEFAULT 0,
                cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                cache_write_tokens INTEGER NOT NULL DEFAULT 0,
                cost_usd REAL,
                currency TEXT NOT NULL DEFAULT 'USD',
                accuracy TEXT NOT NULL,
                occurred_at TEXT NOT NULL,
                session_id TEXT,
                project_slug TEXT,
                ingested_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_events_occurred ON events(occurred_at);
            CREATE INDEX IF NOT EXISTS idx_events_source ON events(source);
            CREATE INDEX IF NOT EXISTS idx_events_model ON events(model);

            CREATE TABLE IF NOT EXISTS scan_cursors (
                path TEXT PRIMARY KEY,
                mtime_ms INTEGER NOT NULL,
                byte_offset INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS collector_health (
                collector_id TEXT PRIMARY KEY,
                status TEXT NOT NULL,
                last_ok_at TEXT,
                schema_fingerprint TEXT,
                message TEXT,
                updated_at TEXT NOT NULL
            );
            "#,
        )?;
        self.conn.execute(
            "INSERT OR REPLACE INTO schema_meta(key, value) VALUES('version', '1')",
            [],
        )?;
        Ok(())
    }

    /// Upsert events by event_id. Returns number of events written.
    pub fn upsert_events(&self, events: &[SpendEvent]) -> Result<usize> {
        if events.is_empty() {
            return Ok(0);
        }
        let now = Utc::now().to_rfc3339();
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare(
                r#"
                INSERT INTO events (
                    event_id, source, client, model,
                    input_tokens, output_tokens, cache_read_tokens, cache_write_tokens,
                    cost_usd, currency, accuracy, occurred_at,
                    session_id, project_slug, ingested_at
                ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)
                ON CONFLICT(event_id) DO UPDATE SET
                    source=excluded.source,
                    client=excluded.client,
                    model=excluded.model,
                    input_tokens=excluded.input_tokens,
                    output_tokens=excluded.output_tokens,
                    cache_read_tokens=excluded.cache_read_tokens,
                    cache_write_tokens=excluded.cache_write_tokens,
                    cost_usd=excluded.cost_usd,
                    currency=excluded.currency,
                    accuracy=excluded.accuracy,
                    occurred_at=excluded.occurred_at,
                    session_id=excluded.session_id,
                    project_slug=excluded.project_slug,
                    ingested_at=excluded.ingested_at
                "#,
            )?;
            for e in events {
                stmt.execute(params![
                    e.event_id,
                    e.source,
                    e.client,
                    e.model,
                    e.input_tokens as i64,
                    e.output_tokens as i64,
                    e.cache_read_tokens as i64,
                    e.cache_write_tokens as i64,
                    e.cost_usd,
                    e.currency,
                    e.accuracy.as_str(),
                    e.occurred_at.to_rfc3339(),
                    e.session_id,
                    e.project_slug,
                    now,
                ])?;
            }
        }
        tx.commit()?;
        Ok(events.len())
    }

    pub fn set_scan_cursor(&self, path: &str, mtime_ms: i64, byte_offset: i64) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT INTO scan_cursors(path, mtime_ms, byte_offset)
            VALUES(?1, ?2, ?3)
            ON CONFLICT(path) DO UPDATE SET
                mtime_ms=excluded.mtime_ms,
                byte_offset=excluded.byte_offset
            "#,
            params![path, mtime_ms, byte_offset],
        )?;
        Ok(())
    }

    pub fn get_scan_cursor(&self, path: &str) -> Result<Option<(i64, i64)>> {
        let row = self
            .conn
            .query_row(
                "SELECT mtime_ms, byte_offset FROM scan_cursors WHERE path = ?1",
                params![path],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
            )
            .optional()?;
        Ok(row)
    }

    pub fn set_collector_health(&self, health: &CollectorHealth) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT INTO collector_health(
                collector_id, status, last_ok_at, schema_fingerprint, message, updated_at
            ) VALUES(?1,?2,?3,?4,?5,?6)
            ON CONFLICT(collector_id) DO UPDATE SET
                status=excluded.status,
                last_ok_at=excluded.last_ok_at,
                schema_fingerprint=excluded.schema_fingerprint,
                message=excluded.message,
                updated_at=excluded.updated_at
            "#,
            params![
                health.collector_id,
                health.status.as_str(),
                health.last_ok_at.map(|t| t.to_rfc3339()),
                health.schema_fingerprint,
                health.message,
                Utc::now().to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn list_collector_health(&self) -> Result<Vec<CollectorHealth>> {
        let mut stmt = self.conn.prepare(
            "SELECT collector_id, status, last_ok_at, schema_fingerprint, message FROM collector_health ORDER BY collector_id",
        )?;
        let rows = stmt.query_map([], |r| {
            let status_s: String = r.get(1)?;
            let last_ok: Option<String> = r.get(2)?;
            Ok(CollectorHealth {
                collector_id: r.get(0)?,
                status: CollectorStatus::parse(&status_s).unwrap_or(CollectorStatus::Ok),
                last_ok_at: last_ok
                    .as_deref()
                    .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                    .map(|d| d.with_timezone(&Utc)),
                schema_fingerprint: r.get(3)?,
                message: r.get(4)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn list_events(&self, query: EventQuery<'_>) -> Result<Vec<SpendEvent>> {
        let mut sql = String::from(
            "SELECT event_id, source, client, model, input_tokens, output_tokens,
                    cache_read_tokens, cache_write_tokens, cost_usd, currency,
                    accuracy, occurred_at, session_id, project_slug
             FROM events WHERE 1=1",
        );
        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        if let Some(from) = query.from {
            sql.push_str(" AND occurred_at >= ?");
            params.push(Box::new(from.to_rfc3339()));
        }
        if let Some(to) = query.to {
            sql.push_str(" AND occurred_at <= ?");
            params.push(Box::new(to.to_rfc3339()));
        }
        if let Some(src) = query.source {
            sql.push_str(" AND source = ?");
            params.push(Box::new(src.to_string()));
        }
        sql.push_str(" ORDER BY occurred_at DESC");
        if let Some(lim) = query.limit {
            sql.push_str(&format!(" LIMIT {lim}"));
        }

        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(param_refs.as_slice(), map_event_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Back-compat wrapper.
    pub fn list_events_legacy(
        &self,
        since: Option<DateTime<Utc>>,
        source: Option<&str>,
        limit: Option<usize>,
    ) -> Result<Vec<SpendEvent>> {
        self.list_events(EventQuery {
            source,
            from: since,
            to: None,
            limit,
        })
    }

    pub fn event_count(&self) -> Result<usize> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))?;
        Ok(n as usize)
    }
}

fn map_event_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<SpendEvent> {
    let accuracy_s: String = r.get(10)?;
    let occurred_s: String = r.get(11)?;
    let occurred_at = DateTime::parse_from_rfc3339(&occurred_s)
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());
    Ok(SpendEvent {
        event_id: r.get(0)?,
        source: r.get(1)?,
        client: r.get(2)?,
        model: r.get(3)?,
        input_tokens: r.get::<_, i64>(4)? as u64,
        output_tokens: r.get::<_, i64>(5)? as u64,
        cache_read_tokens: r.get::<_, i64>(6)? as u64,
        cache_write_tokens: r.get::<_, i64>(7)? as u64,
        cost_usd: r.get(8)?,
        currency: r.get(9)?,
        accuracy: Accuracy::parse(&accuracy_s).unwrap_or(Accuracy::Exact),
        occurred_at,
        session_id: r.get(12)?,
        project_slug: r.get(13)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Accuracy;

    #[test]
    fn upsert_is_idempotent() {
        let ledger = Ledger::open_in_memory().unwrap();
        let ev = SpendEvent {
            event_id: "abc".into(),
            source: "claude_code".into(),
            client: "claude-code".into(),
            model: "claude-sonnet-4".into(),
            input_tokens: 10,
            output_tokens: 5,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            cost_usd: Some(0.01),
            currency: "USD".into(),
            accuracy: Accuracy::Exact,
            occurred_at: Utc::now(),
            session_id: Some("s1".into()),
            project_slug: None,
        };
        ledger.upsert_events(&[ev.clone()]).unwrap();
        ledger.upsert_events(&[ev]).unwrap();
        assert_eq!(ledger.event_count().unwrap(), 1);
    }
}
