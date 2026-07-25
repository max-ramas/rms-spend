pub mod antigravity;
pub mod claude_code;
pub mod codex;
pub mod cursor_proxy;
pub mod cursor_quota;
pub mod gemini_cli;
pub mod opencode;
pub mod qwen_code;
pub mod rms_memory;
pub mod rms_threads;
pub mod util;
pub mod vscode_copilot;
pub mod windsurf;
pub mod zcode;
pub mod zed;

use crate::event::CollectorResult;
use crate::ledger::Ledger;
use crate::tariff::TariffBundle;
use std::path::Path;

pub trait SpendCollector {
    fn id(&self) -> &str;
    fn scan(&self, ledger: &Ledger) -> anyhow::Result<CollectorResult>;
}

pub fn run_collector(
    collector: &dyn SpendCollector,
    ledger: &Ledger,
    tariffs: &TariffBundle,
) -> anyhow::Result<CollectorResult> {
    let mut result = collector.scan(ledger)?;
    tariffs.apply_costs(&mut result.events);
    ledger.upsert_events(&result.events)?;

    let health = crate::event::CollectorHealth {
        collector_id: collector.id().to_string(),
        status: result.status,
        last_ok_at: if matches!(
            result.status,
            crate::event::CollectorStatus::Ok | crate::event::CollectorStatus::Partial
        ) {
            Some(chrono::Utc::now())
        } else {
            None
        },
        schema_fingerprint: result.schema_fingerprint.clone(),
        message: result.message.clone(),
    };
    // Preserve last_ok_at on stale if we had one before.
    if matches!(result.status, crate::event::CollectorStatus::Stale) {
        if let Ok(existing) = ledger.list_collector_health() {
            if let Some(prev) = existing.iter().find(|h| h.collector_id == collector.id()) {
                let mut h = health;
                h.last_ok_at = prev.last_ok_at;
                ledger.set_collector_health(&h)?;
                return Ok(result);
            }
        }
    }
    ledger.set_collector_health(&health)?;
    Ok(result)
}

pub fn resolve_claude_root(override_path: Option<&Path>) -> std::path::PathBuf {
    override_path
        .map(|p| p.to_path_buf())
        .unwrap_or_else(crate::paths::claude_projects_dir)
}

pub fn resolve_memory_root(override_path: Option<&Path>) -> std::path::PathBuf {
    override_path
        .map(|p| p.to_path_buf())
        .unwrap_or_else(crate::paths::rms_memory_ai_runs_dir)
}
