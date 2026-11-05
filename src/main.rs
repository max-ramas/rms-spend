use anyhow::Context;
use chrono::{DateTime, Utc};
use clap::{Parser, Subcommand, ValueEnum};
use rms_spend::budget::{BudgetConfig, BudgetPeriod};
use rms_spend::period::{PeriodFilter, PeriodPreset};
use rms_spend::service::{
    export_events_filtered, init_spend, init_tariffs, refresh_tariff_bundle, scan,
    set_connector_enabled, set_connector_network, status, summary_filtered,
    ExportFormat as SvcExportFormat, ScanOptions, SummaryQuery,
};
use rms_spend::summary::print_table;
use rms_spend::sync::{
    fetch_remote_tariffs, register_account, sync_config_path, sync_events, SyncConfig,
};
use rms_spend::tui::run_tui;
use rms_spend::SpendPaths;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

#[derive(Parser, Debug)]
#[command(
    name = "rms-spend",
    about = "IDE-native AI token spend tracker (local ledger, no interception)",
    version
)]
struct Cli {
    /// Override base dir (default: ~/.rms-memory)
    #[arg(long, global = true, env = "RMS_SPEND_HOME")]
    home: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// First-run setup: dirs, builtin tariffs, default connectors
    Init,

    /// Scan local IDE artifacts into the SQLite ledger
    Scan {
        #[arg(long, env = "CLAUDE_PROJECTS_DIR")]
        claude_projects: Option<PathBuf>,

        #[arg(long)]
        memory_runs: Option<PathBuf>,

        #[arg(long)]
        cursor_tracking: Option<PathBuf>,

        #[arg(long)]
        cursor_state: Option<PathBuf>,

        /// RMS Threads Assistant app.db override
        #[arg(long)]
        threads_db: Option<PathBuf>,

        /// Collector id or `all` (see README collectors table)
        #[arg(long, default_value = "all")]
        only: String,

        /// Ignore connector enable/disable flags
        #[arg(long)]
        force: bool,

        /// Opt-in outbound network (e.g. Windsurf GetUserStatus)
        #[arg(long)]
        network: bool,
    },

    /// Foreground poll loop: scan + sleep (Ctrl-C to exit)
    Watch {
        /// Seconds between scans
        #[arg(long, default_value_t = 300)]
        interval: u64,

        #[arg(long, default_value = "all")]
        only: String,

        #[arg(long)]
        force: bool,

        #[arg(long)]
        network: bool,
    },

    Summary {
        #[arg(long)]
        source: Option<String>,

        /// all | today | week | month | range
        #[arg(long, default_value = "all")]
        period: String,

        /// Range start (RFC3339), with --period range
        #[arg(long)]
        from: Option<String>,

        /// Range end (RFC3339), with --period range
        #[arg(long)]
        to: Option<String>,

        #[arg(long)]
        json: bool,

        #[arg(long)]
        limit: Option<usize>,
    },

    /// Interactive terminal dashboard
    Tui,

    Export {
        #[arg(long, value_enum, default_value = "json")]
        format: ExportFormat,

        #[arg(long, short)]
        output: PathBuf,

        #[arg(long)]
        source: Option<String>,

        #[arg(long, default_value = "all")]
        period: String,

        #[arg(long)]
        from: Option<String>,

        #[arg(long)]
        to: Option<String>,
    },

    Status,

    /// Set / show / clear optional USD budget
    Budget {
        #[command(subcommand)]
        action: BudgetCmd,
    },

    InitTariffs,

    /// Refresh tariff cache (builtin rewrite, or remote via --url / RMS_SPEND_TARIFF_URL)
    RefreshTariffs {
        #[arg(long, env = "RMS_SPEND_TARIFF_URL")]
        url: Option<String>,
    },

    /// Enable or disable a connector (and optional network allow)
    Connector {
        id: String,
        #[arg(long)]
        enable: bool,
        #[arg(long)]
        disable: bool,
        /// Allow outbound network for this collector (Windsurf)
        #[arg(long)]
        allow_network: bool,
        #[arg(long)]
        deny_network: bool,
    },

    /// Opt-in cloud sync to rms-spend-service (requires spend-sync.json)
    #[command(hide = true)]
    Sync {
        /// Override visibility for uploaded events: private|team|public
        #[arg(long, default_value = "private")]
        visibility: String,

        #[arg(long)]
        limit: Option<usize>,

        /// Also refresh tariffs from server before sync
        #[arg(long)]
        refresh_tariffs: bool,
    },

    /// Register a new account on rms-spend-service and write api_key to spend-sync.json
    #[command(hide = true)]
    Register {
        /// Base URL of spend-service (default: https://spend.rms-ds.com/api/v1)
        #[arg(long, env = "RMS_SPEND_API_BASE")]
        api_base: Option<String>,
    },

    /// Write a template spend-sync.json
    #[command(hide = true)]
    InitSync,
}

#[derive(Subcommand, Debug)]
enum BudgetCmd {
    /// Set budget ceiling
    Set {
        #[arg(long)]
        usd: f64,
        /// day | week | month | all
        #[arg(long, default_value = "month")]
        period: String,
    },
    /// Show current budget file
    Show,
    /// Remove spend-budget.json
    Clear,
}

#[derive(Clone, Debug, ValueEnum)]
enum ExportFormat {
    Json,
    Csv,
}

fn parse_period(
    period: &str,
    from: Option<&str>,
    to: Option<&str>,
) -> anyhow::Result<PeriodFilter> {
    let preset = PeriodPreset::parse(period)?;
    let from_dt = from
        .map(DateTime::parse_from_rfc3339)
        .transpose()?
        .map(|d| d.with_timezone(&Utc));
    let to_dt = to
        .map(DateTime::parse_from_rfc3339)
        .transpose()?
        .map(|d| d.with_timezone(&Utc));
    Ok(PeriodFilter {
        preset,
        from: from_dt,
        to: to_dt,
    })
}

fn summary_query(
    source: Option<String>,
    period: &str,
    from: Option<String>,
    to: Option<String>,
    limit: Option<usize>,
) -> anyhow::Result<SummaryQuery> {
    Ok(SummaryQuery {
        source,
        limit,
        period: parse_period(period, from.as_deref(), to.as_deref())?,
    })
}

fn print_budget_line(paths: &SpendPaths, cost_usd: Option<f64>) {
    match BudgetConfig::load_or_none(&paths.budget_json) {
        Ok(Some(b)) => {
            if let Some(line) = b.format_vs_cost(cost_usd) {
                println!("{line}");
            }
        }
        Ok(None) => {}
        Err(e) => eprintln!("warn: budget: {e}"),
    }
}

fn print_scan_report(report: &rms_spend::service::ScanReport) {
    println!("Ledger: {}", report.ledger_path);
    println!(
        "Tariffs: {} ({}){}",
        report.tariff_version,
        &report.tariff_hash[..12.min(report.tariff_hash.len())],
        if report.tariff_stale { " STALE" } else { "" }
    );
    for c in &report.collectors {
        if c.skipped {
            println!("[{}] disabled — skipped", c.collector_id);
            continue;
        }
        if let Some(err) = &c.error {
            eprintln!("[{}] ERROR (skipped): {err}", c.collector_id);
            continue;
        }
        println!(
            "[{}] {:?} — {} event(s){}",
            c.collector_id,
            c.status,
            c.event_count,
            c.message
                .as_ref()
                .map(|m| format!(" ({m})"))
                .unwrap_or_default()
        );
    }
    println!("Total events in ledger: {}", report.total_events);
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let paths = match cli.home {
        Some(base) => SpendPaths::from_base(base),
        None => SpendPaths::default_home(),
    };
    paths.ensure_base()?;

    match cli.command {
        Commands::Init => {
            let report = init_spend(&paths)?;
            println!("RMS Spend home: {}", report.home);
            println!(
                "Tariffs: {}{}",
                report.tariffs_json,
                if report.tariffs_wrote {
                    " (wrote builtin)"
                } else {
                    " (kept existing)"
                }
            );
            println!(
                "Connectors: {}{}",
                report.connectors_json,
                if report.connectors_wrote {
                    " (wrote defaults)"
                } else {
                    " (kept existing)"
                }
            );
            println!("Ledger: {}", report.ledger_db);
            println!();
            println!("Next steps:");
            println!("  rms-spend scan");
            println!("  rms-spend tui");
            println!("  rms-spend connector <id> --enable   # e.g. windsurf, cursor_quota");
            println!("  rms-spend connector windsurf --allow-network   # opt-in live credits");
            println!("  rms-spend budget set --usd 50 --period month");
        }
        Commands::Scan {
            claude_projects,
            memory_runs,
            cursor_tracking,
            cursor_state,
            threads_db,
            only,
            force,
            network,
        } => {
            let report = scan(
                &paths,
                ScanOptions {
                    only,
                    claude_projects,
                    memory_runs,
                    cursor_tracking,
                    cursor_state,
                    threads_db,
                    force_all_enabled: force,
                    allow_network: network,
                    ..Default::default()
                },
            )?;
            print_scan_report(&report);
        }
        Commands::Watch {
            interval,
            only,
            force,
            network,
        } => {
            let secs = interval.max(5);
            eprintln!("rms-spend watch: every {secs}s (Ctrl-C to stop)");
            loop {
                let started = Utc::now();
                match scan(
                    &paths,
                    ScanOptions {
                        only: only.clone(),
                        force_all_enabled: force,
                        allow_network: network,
                        ..Default::default()
                    },
                ) {
                    Ok(report) => {
                        let new: usize = report
                            .collectors
                            .iter()
                            .filter(|c| !c.skipped && c.error.is_none())
                            .map(|c| c.event_count)
                            .sum();
                        println!(
                            "{} scan ok — {} collector event(s) this pass, ledger={}",
                            started.to_rfc3339(),
                            new,
                            report.total_events
                        );
                    }
                    Err(e) => eprintln!("{} scan error: {e}", started.to_rfc3339()),
                }
                thread::sleep(Duration::from_secs(secs));
            }
        }
        Commands::Summary {
            source,
            period,
            from,
            to,
            json,
            limit,
        } => {
            let query = summary_query(source, &period, from, to, limit)?;
            let report = summary_filtered(&paths, &query)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                print_table(&report);
                print_budget_line(&paths, report.cost_usd);
            }
        }
        Commands::Tui => {
            run_tui(&paths)?;
        }
        Commands::Export {
            format,
            output,
            source,
            period,
            from,
            to,
        } => {
            let fmt = match format {
                ExportFormat::Json => SvcExportFormat::Json,
                ExportFormat::Csv => SvcExportFormat::Csv,
            };
            let query = summary_query(source, &period, from, to, None)?;
            let n = export_events_filtered(&paths, fmt, &output, &query)?;
            println!("Exported {n} events → {}", output.display());
        }
        Commands::Status => {
            let report = status(&paths)?;
            println!("Ledger: {}", report.ledger_path);
            println!("Events: {}", report.event_count);
            println!(
                "Tariff: {} ({}) source={} stale={}",
                report.tariff.version,
                &report.tariff.hash[..12.min(report.tariff.hash.len())],
                report.tariff.source,
                report.tariff.stale
            );
            println!("Connectors:");
            for (id, on) in &report.connectors.enabled {
                let net = if report.connectors.network_allowed(id) {
                    " network=on"
                } else {
                    ""
                };
                println!("  {id}: {}{net}", if *on { "on" } else { "off" });
            }
            if let Ok(Some(b)) = BudgetConfig::load_or_none(&paths.budget_json) {
                let period = match b.period {
                    BudgetPeriod::Day => "today",
                    BudgetPeriod::Week => "week",
                    BudgetPeriod::Month => "month",
                    BudgetPeriod::All => "all",
                };
                let cost = summary_filtered(
                    &paths,
                    &SummaryQuery {
                        period: parse_period(period, None, None)?,
                        ..Default::default()
                    },
                )
                .ok()
                .and_then(|r| r.cost_usd);
                if let Some(line) = b.format_vs_cost(cost) {
                    println!("{line}");
                }
            }
            println!();
            if report.health.is_empty() {
                println!("No collector health yet. Run: rms-spend scan");
            } else {
                for h in report.health {
                    println!(
                        "[{}] {:?}  last_ok={}  {}",
                        h.collector_id,
                        h.status,
                        h.last_ok_at
                            .map(|t| t.to_rfc3339())
                            .unwrap_or_else(|| "—".into()),
                        h.message.unwrap_or_default()
                    );
                }
            }
        }
        Commands::Budget { action } => match action {
            BudgetCmd::Set { usd, period } => {
                if usd <= 0.0 {
                    anyhow::bail!("--usd must be positive");
                }
                let cfg = BudgetConfig {
                    usd,
                    period: BudgetPeriod::parse(&period)?,
                };
                cfg.save(&paths.budget_json)?;
                println!(
                    "Wrote {} — ${:.2} / {}",
                    paths.budget_json.display(),
                    cfg.usd,
                    cfg.period.as_str()
                );
            }
            BudgetCmd::Show => match BudgetConfig::load_or_none(&paths.budget_json)? {
                Some(cfg) => {
                    println!(
                        "{} — ${:.2} / {}",
                        paths.budget_json.display(),
                        cfg.usd,
                        cfg.period.as_str()
                    );
                }
                None => println!("No budget set ({})", paths.budget_json.display()),
            },
            BudgetCmd::Clear => {
                BudgetConfig::clear(&paths.budget_json)?;
                println!("Cleared {}", paths.budget_json.display());
            }
        },
        Commands::InitTariffs => {
            let bundle = init_tariffs(&paths, true)?;
            println!("Wrote {}", paths.tariffs_json.display());
            println!("version={} hash={}", bundle.version, bundle.hash);
        }
        Commands::RefreshTariffs { url } => {
            let (bundle, filled) = refresh_tariff_bundle(&paths, url.as_deref())?;
            println!("Wrote {}", paths.tariffs_json.display());
            println!(
                "version={} hash={} source={} stale={} repriced={}",
                bundle.version,
                bundle.hash,
                bundle.source,
                bundle.is_stale(),
                filled
            );
        }
        Commands::Connector {
            id,
            enable,
            disable,
            allow_network,
            deny_network,
        } => {
            let enable_flags = (enable as u8) + (disable as u8);
            let net_flags = (allow_network as u8) + (deny_network as u8);
            if enable_flags > 1 || net_flags > 1 {
                anyhow::bail!("pass at most one of --enable/--disable and one of --allow-network/--deny-network");
            }
            if enable_flags == 0 && net_flags == 0 {
                anyhow::bail!(
                    "pass --enable|--disable and/or --allow-network|--deny-network"
                );
            }
            let mut cfg = if enable_flags == 1 {
                set_connector_enabled(&paths, &id, enable)?
            } else {
                rms_spend::load_connectors(&paths)
            };
            if net_flags == 1 {
                cfg = set_connector_network(&paths, &id, allow_network)?;
            }
            println!("Updated {}", paths.connectors_json.display());
            for (cid, on) in &cfg.enabled {
                let net = if cfg.network_allowed(cid) {
                    " network=on"
                } else {
                    ""
                };
                println!("  {cid}: {}{net}", if *on { "on" } else { "off" });
            }
        }
        Commands::InitSync => {
            let path = sync_config_path(&paths);
            if path.exists() {
                anyhow::bail!("already exists: {}", path.display());
            }
            let cfg = SyncConfig::default();
            cfg.save(&path)?;
            println!("Wrote {}", path.display());
            println!("Run: rms-spend register  (creates account + writes api_key)");
            println!("Or: edit spend-sync.json manually and set enabled=true");
        }
        Commands::Register { api_base } => {
            let cfg_path = sync_config_path(&paths);
            let mut cfg = SyncConfig::load_or_default(&cfg_path);
            if let Some(base) = api_base {
                cfg.api_base = base;
            }
            let resp = register_account(&mut cfg)?;
            cfg.api_key = Some(resp.api_key.clone());
            cfg.enabled = true;
            cfg.save(&cfg_path)?;
            println!("Registered! userId: {}", resp.user_id);
            println!("API key written to {}", cfg_path.display());
            println!("Cloud sync is now enabled. Run: rms-spend sync");
        }
        Commands::Sync {
            visibility,
            limit,
            refresh_tariffs,
        } => {
            let cfg_path = sync_config_path(&paths);
            let cfg = SyncConfig::load(&cfg_path)
                .with_context(|| format!("load {}", cfg_path.display()))?;
            if refresh_tariffs {
                let bundle = fetch_remote_tariffs(&paths, &cfg)?;
                println!(
                    "Tariffs refreshed: {} ({})",
                    bundle.version,
                    &bundle.hash[..12.min(bundle.hash.len())]
                );
            }
            let result = sync_events(&paths, &cfg, &visibility, limit)?;
            println!(
                "Sync: accepted={} skipped={} rejected={} flagged={}",
                result.accepted, result.skipped, result.rejected, result.flagged
            );
            for w in result.warnings {
                println!("  warn: {w}");
            }
        }
    }
    Ok(())
}
