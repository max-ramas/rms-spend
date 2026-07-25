//! RMS Spend — local IDE-native token spend ledger (library core).
//!
//! Collectors read official local artifacts (Claude Code JSONL, RMS Memory ai-runs).
//! SQLite is the single source of truth. Raw JSONL is audit-only input.
//!
//! CLI / TUI shells live in the standalone `rms-spend` application, not this crate.
//! See `SYNC.md` and `scripts/sync-rms-spend-lib.sh`.

pub mod collectors;
pub mod connectors;
pub mod event;
pub mod export;
pub mod labels;
pub mod ledger;
pub mod paths;
pub mod period;
pub mod service;
pub mod summary;
pub mod sync;
pub mod tariff;
pub mod tui;

pub use connectors::ConnectorConfig;
pub use event::{Accuracy, CollectorHealth, CollectorStatus, SpendEvent};
pub use labels::{known_collectors, source_label, source_label_locale};
pub use ledger::Ledger;
pub use paths::SpendPaths;
pub use period::{PeriodFilter, PeriodPreset};
pub use service::{
    export_events, export_events_filtered, init_tariffs, load_connectors, refresh_tariff_bundle,
    reprice_ledger, save_connectors, scan, set_connector_enabled, status, summary,
    summary_filtered, tray_snapshot, ExportFormat, ScanOptions, ScanReport, StatusReport,
    SummaryQuery, TraySourceLine, TraySpendSnapshot,
};
pub use summary::{Bucket, SummaryReport};
pub use sync::{
    fetch_cloud_summary, fetch_remote_tariffs, register_account, sync_config_path, sync_events,
    CloudSummary, RegisterResponse, SyncConfig, SyncResult, SyncWithSummary,
};
pub use tariff::{TariffBundle, TariffStatus};
