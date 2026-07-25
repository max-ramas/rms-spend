//! Library-facing scan / summary / status / export orchestration.
//! Used by the CLI binary and by consumers (e.g. RMS Memory GUI).

use crate::collectors::antigravity::AntigravityCollector;
use crate::collectors::claude_code::ClaudeCodeCollector;
use crate::collectors::codex::CodexCollector;
use crate::collectors::cursor_proxy::CursorProxyCollector;
use crate::collectors::cursor_quota::CursorQuotaCollector;
use crate::collectors::gemini_cli::GeminiCliCollector;
use crate::collectors::opencode::OpenCodeCollector;
use crate::collectors::qwen_code::QwenCodeCollector;
use crate::collectors::rms_memory::RmsMemoryCollector;
use crate::collectors::rms_threads::RmsThreadsCollector;
use crate::collectors::vscode_copilot::VscodeCopilotCollector;
use crate::collectors::windsurf::WindsurfCollector;
use crate::collectors::zcode::ZCodeCollector;
use crate::collectors::zed::ZedCollector;
use crate::collectors::{run_collector, SpendCollector};
use crate::connectors::ConnectorConfig;
use crate::event::{CollectorHealth, CollectorStatus};
use crate::export::{export_csv, export_json};
use crate::ledger::{EventQuery, Ledger};
use crate::paths::SpendPaths;
use crate::period::{PeriodFilter, PeriodPreset};
use crate::summary::{summarize, SummaryReport};
use crate::tariff::{refresh_tariffs, TariffBundle, TariffStatus};
use anyhow::{bail, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default)]
pub struct ScanOptions {
    /// `all` | collector id (see `KNOWN_COLLECTORS`)
    pub only: String,
    pub claude_projects: Option<PathBuf>,
    pub memory_runs: Option<PathBuf>,
    pub cursor_tracking: Option<PathBuf>,
    pub cursor_state: Option<PathBuf>,
    pub threads_db: Option<PathBuf>,
    pub opencode_db: Option<PathBuf>,
    pub codex_sessions: Option<PathBuf>,
    pub qwen_home: Option<PathBuf>,
    pub zcode_rollout: Option<PathBuf>,
    pub gemini_home: Option<PathBuf>,
    pub antigravity_conversations: Option<PathBuf>,
    pub vscode_workspace_storage: Option<PathBuf>,
    pub zed_threads_db: Option<PathBuf>,
    pub windsurf_state: Option<PathBuf>,
    /// When true, ignore connector enable flags (still respects `--only`).
    pub force_all_enabled: bool,
    /// Opt-in outbound network for collectors that support it (Windsurf GetUserStatus).
    pub allow_network: bool,
}

impl ScanOptions {
    pub fn all() -> Self {
        Self {
            only: "all".into(),
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectorScanReport {
    pub collector_id: String,
    pub status: CollectorStatus,
    pub event_count: usize,
    pub message: Option<String>,
    pub error: Option<String>,
    pub skipped: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanReport {
    pub ledger_path: String,
    pub tariff_version: String,
    pub tariff_hash: String,
    pub tariff_stale: bool,
    pub collectors: Vec<CollectorScanReport>,
    pub total_events: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusReport {
    pub ledger_path: String,
    pub tariffs_path: String,
    pub tariff: TariffStatus,
    pub event_count: usize,
    pub health: Vec<CollectorHealth>,
    pub connectors: ConnectorConfig,
}

fn build_collectors(
    opts: &ScanOptions,
    connectors: &ConnectorConfig,
) -> Result<Vec<Box<dyn SpendCollector>>> {
    let only = if opts.only.is_empty() {
        "all"
    } else {
        opts.only.as_str()
    };

    let mut collectors: Vec<Box<dyn SpendCollector>> = Vec::new();
    let want = |id: &str| only == "all" || only == id;

    if want("claude_code") {
        collectors.push(Box::new(ClaudeCodeCollector::new(
            opts.claude_projects
                .clone()
                .unwrap_or_else(crate::paths::claude_projects_dir),
        )));
    }
    if want("rms_memory") {
        collectors.push(Box::new(RmsMemoryCollector::new(
            opts.memory_runs
                .clone()
                .unwrap_or_else(crate::paths::rms_memory_ai_runs_dir),
        )));
    }
    if want("cursor_proxy") {
        let cursor_state = opts
            .cursor_state
            .clone()
            .unwrap_or_else(crate::paths::cursor_state_vscdb);
        collectors.push(Box::new(CursorProxyCollector::new(
            opts.cursor_tracking
                .clone()
                .unwrap_or_else(crate::paths::cursor_tracking_db),
            cursor_state.clone(),
        )));
    }
    if want("cursor_quota") {
        collectors.push(Box::new(CursorQuotaCollector::new(
            opts.cursor_state
                .clone()
                .unwrap_or_else(crate::paths::cursor_state_vscdb),
        )));
    }
    if want("rms_threads") {
        collectors.push(Box::new(RmsThreadsCollector::new(
            opts.threads_db
                .clone()
                .unwrap_or_else(crate::paths::rms_threads_app_db),
        )));
    }
    if want("opencode") {
        collectors.push(Box::new(OpenCodeCollector::new(
            opts.opencode_db
                .clone()
                .unwrap_or_else(crate::paths::opencode_db),
        )));
    }
    if want("codex") {
        collectors.push(Box::new(CodexCollector::new(
            opts.codex_sessions
                .clone()
                .unwrap_or_else(crate::paths::codex_sessions_dir),
        )));
    }
    if want("qwen_code") {
        collectors.push(Box::new(QwenCodeCollector::new(
            opts.qwen_home
                .clone()
                .unwrap_or_else(crate::paths::qwen_home_dir),
        )));
    }
    if want("zcode") {
        collectors.push(Box::new(ZCodeCollector::new(
            opts.zcode_rollout
                .clone()
                .unwrap_or_else(crate::paths::zcode_rollout_dir),
        )));
    }
    if want("gemini_cli") {
        collectors.push(Box::new(GeminiCliCollector::new(
            opts.gemini_home
                .clone()
                .unwrap_or_else(crate::paths::gemini_home_dir),
        )));
    }
    if want("antigravity") {
        collectors.push(Box::new(AntigravityCollector::new(
            opts.antigravity_conversations
                .clone()
                .unwrap_or_else(crate::paths::antigravity_conversations_dir),
        )));
    }
    if want("vscode_copilot") {
        collectors.push(Box::new(VscodeCopilotCollector::new(
            opts.vscode_workspace_storage
                .clone()
                .unwrap_or_else(crate::paths::vscode_workspace_storage_dir),
        )));
    }
    if want("zed") {
        collectors.push(Box::new(ZedCollector::new(
            opts.zed_threads_db
                .clone()
                .unwrap_or_else(crate::paths::zed_threads_db),
        )));
    }
    if want("windsurf") {
        let dbs = if let Some(p) = &opts.windsurf_state {
            vec![p.clone()]
        } else {
            vec![
                crate::paths::windsurf_state_vscdb(),
                crate::paths::windsurf_next_state_vscdb(),
            ]
        };
        let windsurf_net = opts.allow_network || connectors.network_allowed("windsurf");
        collectors.push(Box::new(WindsurfCollector::new(dbs).with_network(windsurf_net)));
    }

    if collectors.is_empty() {
        bail!(
            "unknown collector: {only} (use all or one of: {})",
            crate::connectors::KNOWN_COLLECTORS.join("|")
        );
    }
    Ok(collectors)
}

/// Run collectors and upsert into the ledger.
pub fn scan(paths: &SpendPaths, opts: ScanOptions) -> Result<ScanReport> {
    paths.ensure_base()?;
    let ledger = Ledger::open(&paths.ledger_db)?;
    let tariffs = TariffBundle::load_or_builtin(&paths.tariffs_json);
    let connectors = ConnectorConfig::load_or_default(&paths.connectors_json);
    let collectors = build_collectors(&opts, &connectors)?;

    let mut reports = Vec::with_capacity(collectors.len());
    for c in &collectors {
        if !opts.force_all_enabled && !connectors.is_enabled(c.id()) {
            reports.push(CollectorScanReport {
                collector_id: c.id().to_string(),
                status: CollectorStatus::Disabled,
                event_count: 0,
                message: Some("disabled in connector config".into()),
                error: None,
                skipped: true,
            });
            continue;
        }
        match run_collector(c.as_ref(), &ledger, &tariffs) {
            Ok(result) => reports.push(CollectorScanReport {
                collector_id: c.id().to_string(),
                status: result.status,
                event_count: result.events.len(),
                message: result.message,
                error: None,
                skipped: false,
            }),
            Err(e) => reports.push(CollectorScanReport {
                collector_id: c.id().to_string(),
                status: CollectorStatus::Stale,
                event_count: 0,
                message: None,
                error: Some(e.to_string()),
                skipped: false,
            }),
        }
    }

    Ok(ScanReport {
        ledger_path: paths.ledger_db.display().to_string(),
        tariff_version: tariffs.version.clone(),
        tariff_hash: tariffs.hash.clone(),
        tariff_stale: tariffs.is_stale(),
        collectors: reports,
        total_events: ledger.event_count()?,
    })
}

pub fn status(paths: &SpendPaths) -> Result<StatusReport> {
    paths.ensure_base()?;
    let ledger = Ledger::open(&paths.ledger_db)?;
    let tariffs = TariffBundle::load_or_builtin(&paths.tariffs_json);
    let connectors = ConnectorConfig::load_or_default(&paths.connectors_json);
    Ok(StatusReport {
        ledger_path: paths.ledger_db.display().to_string(),
        tariffs_path: paths.tariffs_json.display().to_string(),
        tariff: tariffs.status_for(&paths.tariffs_json),
        event_count: ledger.event_count()?,
        health: ledger.list_collector_health()?,
        connectors,
    })
}

#[derive(Debug, Clone, Default)]
pub struct SummaryQuery {
    pub source: Option<String>,
    pub limit: Option<usize>,
    pub period: PeriodFilter,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TraySourceLine {
    pub id: String,
    pub tokens: u64,
    pub events: usize,
    pub cost_usd: Option<f64>,
    pub is_proxy: bool,
    pub is_quota: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TraySpendSnapshot {
    pub today_tokens: u64,
    pub today_cost_usd: Option<f64>,
    pub week_tokens: u64,
    pub week_cost_usd: Option<f64>,
    pub month_tokens: u64,
    pub month_cost_usd: Option<f64>,
    /// Sources with activity today only (no zero rows).
    pub today_sources: Vec<TraySourceLine>,
}

pub fn summary_filtered(paths: &SpendPaths, query: &SummaryQuery) -> Result<SummaryReport> {
    paths.ensure_base()?;
    let ledger = Ledger::open(&paths.ledger_db)?;
    let (from, to) = query.period.bounds()?;
    let events = ledger.list_events(EventQuery {
        source: query.source.as_deref(),
        from,
        to,
        limit: query.limit,
    })?;
    let mut report = summarize(&events);
    if query.period.preset != PeriodPreset::All {
        report.period = Some(query.period.preset.as_str().into());
        report.filter_from = from.map(|t| t.to_rfc3339());
        report.filter_to = to.map(|t| t.to_rfc3339());
    }
    Ok(report)
}

pub fn summary(
    paths: &SpendPaths,
    source: Option<&str>,
    limit: Option<usize>,
) -> Result<SummaryReport> {
    summary_filtered(
        paths,
        &SummaryQuery {
            source: source.map(|s| s.to_string()),
            limit,
            period: PeriodFilter::all(),
        },
    )
}

pub fn tray_snapshot(paths: &SpendPaths) -> Result<TraySpendSnapshot> {
    let today = summary_filtered(
        paths,
        &SummaryQuery {
            period: PeriodFilter::today(),
            ..Default::default()
        },
    )?;
    let week = summary_filtered(
        paths,
        &SummaryQuery {
            period: PeriodFilter::week(),
            ..Default::default()
        },
    )?;
    let month = summary_filtered(
        paths,
        &SummaryQuery {
            period: PeriodFilter::month(),
            ..Default::default()
        },
    )?;
    let mut today_sources: Vec<TraySourceLine> = today
        .by_source
        .iter()
        .filter(|(_, b)| b.events > 0)
        .map(|(id, b)| {
            let tokens = if b.is_proxy || b.is_quota {
                b.input_tokens + b.output_tokens
            } else {
                b.input_tokens + b.output_tokens + b.cache_read_tokens + b.cache_write_tokens
            };
            TraySourceLine {
                id: id.clone(),
                tokens,
                events: b.events,
                cost_usd: b.cost_usd,
                is_proxy: b.is_proxy,
                is_quota: b.is_quota,
            }
        })
        .collect();
    // Prefer token consumers first in tray ordering.
    today_sources.sort_by(|a, b| {
        let rank = |s: &TraySourceLine| if s.is_proxy || s.is_quota { 1 } else { 0 };
        rank(a)
            .cmp(&rank(b))
            .then_with(|| b.tokens.cmp(&a.tokens))
            .then_with(|| a.id.cmp(&b.id))
    });

    Ok(TraySpendSnapshot {
        today_tokens: today.total_tokens,
        today_cost_usd: today.cost_usd,
        week_tokens: week.total_tokens,
        week_cost_usd: week.cost_usd,
        month_tokens: month.total_tokens,
        month_cost_usd: month.cost_usd,
        today_sources,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Json,
    Csv,
}

pub fn export_events_filtered(
    paths: &SpendPaths,
    format: ExportFormat,
    output: &Path,
    query: &SummaryQuery,
) -> Result<usize> {
    paths.ensure_base()?;
    let ledger = Ledger::open(&paths.ledger_db)?;
    let (from, to) = query.period.bounds()?;
    let events = ledger.list_events(EventQuery {
        source: query.source.as_deref(),
        from,
        to,
        limit: query.limit,
    })?;
    match format {
        ExportFormat::Json => export_json(&events, output)?,
        ExportFormat::Csv => export_csv(&events, output)?,
    }
    Ok(events.len())
}

pub fn export_events(
    paths: &SpendPaths,
    format: ExportFormat,
    output: &Path,
    source: Option<&str>,
) -> Result<usize> {
    export_events_filtered(
        paths,
        format,
        output,
        &SummaryQuery {
            source: source.map(|s| s.to_string()),
            period: PeriodFilter::all(),
            ..Default::default()
        },
    )
}

/// Ensure builtin tariffs are written (idempotent overwrite when forced).
pub fn init_tariffs(paths: &SpendPaths, force: bool) -> Result<TariffBundle> {
    paths.ensure_base()?;
    if paths.tariffs_json.exists() && !force {
        return Ok(TariffBundle::load_or_builtin(&paths.tariffs_json));
    }
    let mut bundle = TariffBundle::builtin();
    bundle.fetched_at = Some(chrono::Utc::now().to_rfc3339());
    bundle.save(&paths.tariffs_json)?;
    Ok(bundle)
}

pub fn refresh_tariff_bundle(
    paths: &SpendPaths,
    url: Option<&str>,
) -> Result<(TariffBundle, usize)> {
    paths.ensure_base()?;
    let bundle = refresh_tariffs(&paths.tariffs_json, url)?;
    let filled = reprice_ledger(paths, &bundle)?;
    Ok((bundle, filled))
}

/// Apply current tariffs to ledger events that still have `cost_usd = null`.
/// Returns how many events received a newly computed price.
pub fn reprice_ledger(paths: &SpendPaths, tariffs: &TariffBundle) -> Result<usize> {
    let ledger = Ledger::open(&paths.ledger_db)?;
    let mut events = ledger.list_events(EventQuery {
        limit: None,
        ..Default::default()
    })?;
    let filled = tariffs.fill_missing_costs(&mut events);
    if filled > 0 {
        ledger.upsert_events(&events)?;
    }
    Ok(filled)
}

pub fn load_connectors(paths: &SpendPaths) -> ConnectorConfig {
    let _ = paths.ensure_base();
    ConnectorConfig::load_or_default(&paths.connectors_json)
}

pub fn save_connectors(paths: &SpendPaths, cfg: &ConnectorConfig) -> Result<()> {
    paths.ensure_base()?;
    cfg.save(&paths.connectors_json)?;
    Ok(())
}

pub fn set_connector_enabled(
    paths: &SpendPaths,
    id: &str,
    enabled: bool,
) -> Result<ConnectorConfig> {
    let mut cfg = load_connectors(paths);
    if !crate::connectors::KNOWN_COLLECTORS.contains(&id) {
        bail!("unknown connector: {id}");
    }
    cfg.set_enabled(id, enabled);
    save_connectors(paths, &cfg)?;
    Ok(cfg)
}

pub fn set_connector_network(
    paths: &SpendPaths,
    id: &str,
    allow: bool,
) -> Result<ConnectorConfig> {
    let mut cfg = load_connectors(paths);
    if !crate::connectors::KNOWN_COLLECTORS.contains(&id) {
        bail!("unknown connector: {id}");
    }
    cfg.set_network_allowed(id, allow);
    save_connectors(paths, &cfg)?;
    Ok(cfg)
}

/// First-run onboarding: dirs, builtin tariffs, default connectors.
pub fn init_spend(paths: &SpendPaths) -> Result<InitReport> {
    paths.ensure_base()?;
    let tariffs_wrote = if !paths.tariffs_json.exists() {
        init_tariffs(paths, false)?;
        true
    } else {
        false
    };
    let connectors_wrote = if !paths.connectors_json.exists() {
        ConnectorConfig::default().save(&paths.connectors_json)?;
        true
    } else {
        false
    };
    Ok(InitReport {
        home: paths.base_dir.display().to_string(),
        ledger_db: paths.ledger_db.display().to_string(),
        tariffs_json: paths.tariffs_json.display().to_string(),
        connectors_json: paths.connectors_json.display().to_string(),
        budget_json: paths.budget_json.display().to_string(),
        tariffs_wrote,
        connectors_wrote,
    })
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InitReport {
    pub home: String,
    pub ledger_db: String,
    pub tariffs_json: String,
    pub connectors_json: String,
    pub budget_json: String,
    pub tariffs_wrote: bool,
    pub connectors_wrote: bool,
}
