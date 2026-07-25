//! Opt-in cloud sync against rms-spend-service `/api/v1/*`.

use crate::event::SpendEvent;
use crate::ledger::Ledger;
use crate::paths::SpendPaths;
use crate::tariff::TariffBundle;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncConfig {
    /// Base URL e.g. https://spend.rms-ds.com/api/v1
    pub api_base: String,
    /// API key (`X-API-Key`) — obtain via `rms-spend register` or POST /api/v1/register.
    pub api_key: Option<String>,
    pub enabled: bool,
    /// Stable local device id for register HMAC attestation.
    #[serde(default)]
    pub device_id: Option<String>,
    /// Optional shared register HMAC secret (also via `RMS_SPEND_REGISTER_SECRET`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub register_secret: Option<String>,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            api_base: "https://spend.rms-ds.com/api/v1".into(),
            api_key: None,
            enabled: false,
            device_id: None,
            register_secret: None,
        }
    }
}

impl SyncConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path)?;
        Ok(serde_json::from_str(&raw)?)
    }

    pub fn load_or_default(path: &Path) -> Self {
        Self::load(path).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    /// Ensure a stable device_id is present (persisted by caller).
    pub fn ensure_device_id(&mut self) -> &str {
        if self
            .device_id
            .as_ref()
            .map(|s| s.is_empty())
            .unwrap_or(true)
        {
            self.device_id = Some(uuid::Uuid::new_v4().to_string());
        }
        self.device_id.as_deref().unwrap()
    }

    pub fn resolve_register_secret(&self) -> Option<String> {
        if let Ok(s) = std::env::var("RMS_SPEND_REGISTER_SECRET") {
            if !s.is_empty() {
                return Some(s);
            }
        }
        self.register_secret.clone().filter(|s| !s.is_empty())
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SyncPayload<'a> {
    events: &'a [SyncEventDto],
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SyncEventDto {
    event_id: String,
    source: String,
    client: String,
    model: String,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_write_tokens: u64,
    cost_usd: Option<f64>,
    currency: String,
    accuracy: String,
    visibility: String,
    occurred_at: String,
    session_id: Option<String>,
    project_slug: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncResult {
    pub accepted: usize,
    pub skipped: usize,
    pub rejected: usize,
    #[serde(default)]
    pub warnings: Vec<String>,
    #[serde(default)]
    pub flagged: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncWithSummary {
    #[serde(flatten)]
    pub sync: SyncResult,
    pub cloud_summary: Option<CloudSummary>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudSummary {
    pub event_count: usize,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterResponse {
    pub user_id: String,
    pub api_key: String,
}

fn to_dto(e: &SpendEvent, visibility: &str) -> SyncEventDto {
    SyncEventDto {
        event_id: e.event_id.clone(),
        source: e.source.clone(),
        client: e.client.clone(),
        model: e.model.clone(),
        input_tokens: e.input_tokens,
        output_tokens: e.output_tokens,
        cache_read_tokens: e.cache_read_tokens,
        cache_write_tokens: e.cache_write_tokens,
        cost_usd: e.cost_usd,
        currency: e.currency.clone(),
        accuracy: e.accuracy.as_str().to_string(),
        visibility: visibility.to_string(),
        occurred_at: e.occurred_at.to_rfc3339(),
        session_id: e.session_id.clone(),
        project_slug: e.project_slug.clone(),
    }
}

fn auth_header(cfg: &SyncConfig) -> Result<(&'static str, String)> {
    if let Some(key) = cfg.api_key.as_ref().filter(|s| !s.is_empty()) {
        return Ok(("X-API-Key", key.clone()));
    }
    bail!("sync requires api_key in spend-sync.json — run: rms-spend register")
}

/// Fetch remote tariff bundle and cache locally.
pub fn fetch_remote_tariffs(paths: &SpendPaths, cfg: &SyncConfig) -> Result<TariffBundle> {
    let url = format!("{}/tariffs", cfg.api_base.trim_end_matches('/'));
    let resp = ureq::get(&url)
        .call()
        .with_context(|| format!("GET {url}"))?;
    let mut bundle: TariffBundle = resp.into_json()?;
    bundle.source = "remote".into();
    bundle.fetched_at = Some(chrono::Utc::now().to_rfc3339());
    if bundle.ttl_days == 0 {
        bundle.ttl_days = 7;
    }
    bundle.save(&paths.tariffs_json)?;
    Ok(bundle)
}

/// Fetch aggregated cloud summary for the authenticated user.
pub fn fetch_cloud_summary(cfg: &SyncConfig) -> Result<CloudSummary> {
    let (hk, hv) = auth_header(cfg)?;
    let url = format!("{}/me/summary", cfg.api_base.trim_end_matches('/'));
    let resp = ureq::get(&url)
        .set(hk, &hv)
        .call()
        .with_context(|| format!("GET {url}"))?;
    if !(200..300).contains(&resp.status()) {
        bail!("cloud summary HTTP {}", resp.status());
    }
    Ok(resp.into_json()?)
}

/// Register a new account on rms-spend-service and return the api_key.
///
/// When `RMS_SPEND_REGISTER_SECRET` or `registerSecret` is set, sends HMAC
/// attestation (`deviceId|ts|nonce`). Otherwise posts `{}` for soft/dev servers.
pub fn register_account(cfg: &mut SyncConfig) -> Result<RegisterResponse> {
    let url = format!("{}/register", cfg.api_base.trim_end_matches('/'));
    let device_id = cfg.ensure_device_id().to_string();
    let body = if let Some(secret) = cfg.resolve_register_secret() {
        let ts = chrono::Utc::now().timestamp();
        let nonce = uuid::Uuid::new_v4().to_string();
        let sig = hmac_sign(&secret, &device_id, ts, &nonce);
        serde_json::json!({
            "deviceId": device_id,
            "ts": ts,
            "nonce": nonce,
            "sig": sig,
        })
    } else {
        serde_json::json!({})
    };

    let resp = ureq::post(&url)
        .set("Content-Type", "application/json")
        .send_json(body)
        .with_context(|| format!("POST {url}"))?;
    if !(200..300).contains(&resp.status()) {
        let body = resp.into_string().unwrap_or_default();
        bail!("register failed: {body}");
    }
    Ok(resp.into_json()?)
}

fn hmac_sign(secret: &str, device_id: &str, ts: i64, nonce: &str) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    type HmacSha256 = Hmac<Sha256>;
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC key");
    mac.update(device_id.as_bytes());
    mac.update(b"|");
    mac.update(ts.to_string().as_bytes());
    mac.update(b"|");
    mac.update(nonce.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// Upload local ledger events (metadata only) to the cloud.
pub fn sync_events(
    paths: &SpendPaths,
    cfg: &SyncConfig,
    visibility: &str,
    limit: Option<usize>,
) -> Result<SyncResult> {
    if !cfg.enabled {
        bail!("cloud sync disabled in spend-sync.json (set enabled=true)");
    }
    let (hk, hv) = auth_header(cfg)?;
    let ledger = Ledger::open(&paths.ledger_db)?;
    let events = ledger.list_events(crate::ledger::EventQuery {
        limit,
        ..Default::default()
    })?;
    let dtos: Vec<_> = events.iter().map(|e| to_dto(e, visibility)).collect();

    let url = format!("{}/sync", cfg.api_base.trim_end_matches('/'));
    let payload = SyncPayload { events: &dtos };
    let resp = ureq::post(&url)
        .set("Content-Type", "application/json")
        .set(hk, &hv)
        .send_json(serde_json::to_value(&payload)?)
        .with_context(|| format!("POST {url}"))?;
    if resp.status() == 400 {
        let body = resp.into_string().unwrap_or_default();
        bail!("sync rejected: {body}");
    }
    if !(200..300).contains(&resp.status()) {
        bail!("sync HTTP {}", resp.status());
    }
    Ok(resp.into_json()?)
}

pub fn sync_config_path(paths: &SpendPaths) -> std::path::PathBuf {
    paths.base_dir.join("spend-sync.json")
}
