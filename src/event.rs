use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// How accurately tokens / cost were measured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Accuracy {
    /// Provider-reported usage (Claude JSONL, OpenAI usage block).
    Exact,
    /// Heuristic from text length or similar.
    Estimated,
    /// Non-token proxy (Cursor line stats, context %).
    Proxy,
    /// Subscription / credit quota units (not API tokens).
    Quota,
}

impl Accuracy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::Estimated => "estimated",
            Self::Proxy => "proxy",
            Self::Quota => "quota",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "exact" => Some(Self::Exact),
            "estimated" => Some(Self::Estimated),
            "proxy" => Some(Self::Proxy),
            "quota" => Some(Self::Quota),
            _ => None,
        }
    }

    /// Counts toward API token totals / cost rollups.
    pub fn is_billable(self) -> bool {
        matches!(self, Self::Exact | Self::Estimated)
    }
}

/// Normalized spend event. Never contains prompts, code, or transcript text.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpendEvent {
    /// Stable dedupe key — hash(source + session_id + message_id) for Claude.
    pub event_id: String,
    /// Collector id: `claude_code`, `rms_memory`, `cursor_proxy`, …
    pub source: String,
    /// Client / product label for display.
    pub client: String,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    /// None when tariff unknown — never 0 for "unknown".
    pub cost_usd: Option<f64>,
    pub currency: String,
    pub accuracy: Accuracy,
    /// Always UTC.
    pub occurred_at: DateTime<Utc>,
    pub session_id: Option<String>,
    pub project_slug: Option<String>,
}

impl SpendEvent {
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens + self.output_tokens + self.cache_read_tokens + self.cache_write_tokens
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollectorStatus {
    #[default]
    Ok,
    Stale,
    Partial,
    Disabled,
}

impl CollectorStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Stale => "stale",
            Self::Partial => "partial",
            Self::Disabled => "disabled",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "ok" => Some(Self::Ok),
            "stale" => Some(Self::Stale),
            "partial" => Some(Self::Partial),
            "disabled" => Some(Self::Disabled),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectorHealth {
    pub collector_id: String,
    pub status: CollectorStatus,
    pub last_ok_at: Option<DateTime<Utc>>,
    pub schema_fingerprint: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct CollectorResult {
    pub events: Vec<SpendEvent>,
    pub status: CollectorStatus,
    pub message: Option<String>,
    pub schema_fingerprint: Option<String>,
}
