use crate::event::SpendEvent;
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// Default offline TTL for a cached remote/file tariff bundle.
pub const DEFAULT_TARIFF_TTL_DAYS: i64 = 7;

/// Versioned tariff bundle. Unknown models → cost_usd = null (never invent a price).
/// Explicit free models use 0/0.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TariffBundle {
    pub version: String,
    pub hash: String,
    pub models: HashMap<String, ModelRate>,
    /// `builtin` | `file` | `remote`
    #[serde(default = "default_source")]
    pub source: String,
    /// RFC3339 UTC when this bundle was fetched/written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fetched_at: Option<String>,
    /// Offline TTL in days (default 7).
    #[serde(default = "default_ttl_days")]
    pub ttl_days: u32,
}

fn default_source() -> String {
    "builtin".into()
}

fn default_ttl_days() -> u32 {
    DEFAULT_TARIFF_TTL_DAYS as u32
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRate {
    /// USD per 1M input tokens.
    pub input_per_m: f64,
    /// USD per 1M output tokens.
    pub output_per_m: f64,
    /// Optional USD per 1M cache-read tokens.
    #[serde(default)]
    pub cache_read_per_m: Option<f64>,
    /// Optional USD per 1M cache-write tokens.
    #[serde(default)]
    pub cache_write_per_m: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TariffStatus {
    pub version: String,
    pub hash: String,
    pub source: String,
    pub fetched_at: Option<String>,
    pub ttl_days: u32,
    pub stale: bool,
    pub path: String,
}

impl TariffBundle {
    pub fn builtin() -> Self {
        let mut models = HashMap::new();

        // ── Anthropic (public list) ─────────────────────────────────────
        insert(
            &mut models,
            "claude-opus-4",
            15.0,
            75.0,
            Some(1.50),
            Some(18.75),
        );
        insert(
            &mut models,
            "claude-sonnet-4",
            3.0,
            15.0,
            Some(0.30),
            Some(3.75),
        );
        insert(
            &mut models,
            "claude-haiku-4",
            0.80,
            4.0,
            Some(0.08),
            Some(1.0),
        );
        insert(
            &mut models,
            "claude-opus-4-1",
            15.0,
            75.0,
            Some(1.50),
            Some(18.75),
        );
        insert(
            &mut models,
            "claude-sonnet-4-5",
            3.0,
            15.0,
            Some(0.30),
            Some(3.75),
        );
        insert(&mut models, "claude-3-5-sonnet", 3.0, 15.0, None, None);
        insert(&mut models, "claude-3-5-haiku", 0.80, 4.0, None, None);
        insert(&mut models, "claude-3-opus", 15.0, 75.0, None, None);
        insert(&mut models, "claude-3-sonnet", 3.0, 15.0, None, None);
        insert(&mut models, "claude-3-haiku", 0.25, 1.25, None, None);

        // ── OpenAI ──────────────────────────────────────────────────────
        insert(&mut models, "gpt-4o", 2.50, 10.0, None, None);
        insert(&mut models, "gpt-4o-mini", 0.15, 0.60, None, None);
        insert(&mut models, "gpt-4.1", 2.0, 8.0, None, None);
        insert(&mut models, "gpt-4.1-mini", 0.40, 1.60, None, None);
        insert(&mut models, "gpt-4.1-nano", 0.10, 0.40, None, None);
        insert(&mut models, "gpt-4-turbo", 10.0, 30.0, None, None);
        insert(&mut models, "gpt-5", 1.25, 10.0, None, None);
        insert(&mut models, "gpt-5-mini", 0.25, 2.0, None, None);
        insert(&mut models, "o1", 15.0, 60.0, None, None);
        insert(&mut models, "o1-mini", 1.10, 4.40, None, None);
        insert(&mut models, "o3", 2.0, 8.0, None, None);
        insert(&mut models, "o3-mini", 1.10, 4.40, None, None);
        insert(&mut models, "o4-mini", 1.10, 4.40, None, None);

        // ── Google Gemini (Developer API standard tier, mid-2026) ───────
        // Gemini 2.5
        insert(&mut models, "gemini-2.5-pro", 1.25, 10.0, Some(0.125), None);
        insert(
            &mut models,
            "gemini-2.5-flash",
            0.30,
            2.50,
            Some(0.03),
            None,
        );
        insert(
            &mut models,
            "gemini-2.5-flash-lite",
            0.10,
            0.40,
            Some(0.01),
            None,
        );
        insert(
            &mut models,
            "gemini-2.0-flash",
            0.10,
            0.40,
            Some(0.025),
            None,
        );
        insert(&mut models, "gemini-1.5-pro", 1.25, 5.0, None, None);
        insert(&mut models, "gemini-1.5-flash", 0.075, 0.30, None, None);
        // Gemini 3.x
        insert(
            &mut models,
            "gemini-3.6-flash",
            1.50,
            7.50,
            Some(0.15),
            None,
        );
        insert(
            &mut models,
            "gemini-3.5-flash",
            1.50,
            9.00,
            Some(0.15),
            None,
        );
        insert(
            &mut models,
            "gemini-3.5-flash-lite",
            0.25,
            1.50,
            Some(0.025),
            None,
        );
        insert(&mut models, "gemini-3.1-pro", 2.00, 12.00, Some(0.20), None);
        insert(
            &mut models,
            "gemini-3.1-flash-lite",
            0.25,
            1.50,
            Some(0.025),
            None,
        );
        insert(&mut models, "gemini-3-flash", 0.50, 3.00, Some(0.05), None);
        insert(&mut models, "gemini-3-pro", 2.00, 12.00, Some(0.20), None);
        // Antigravity / product aliases (canonical keys also matched via alias table)
        insert(
            &mut models,
            "gemini-pro-default",
            2.00,
            12.00,
            Some(0.20),
            None,
        );
        insert(
            &mut models,
            "gemini-3-flash-a",
            1.50,
            7.50,
            Some(0.15),
            None,
        );
        insert(
            &mut models,
            "gemini-3.1-pro-low",
            2.00,
            12.00,
            Some(0.20),
            None,
        );
        insert(&mut models, "gemini-default", 0.50, 3.00, Some(0.05), None);
        insert(&mut models, "gemini", 0.50, 3.00, Some(0.05), None);

        // ── DeepSeek (official API cache-miss rates) ────────────────────
        insert(
            &mut models,
            "deepseek-v4-pro",
            0.435,
            0.87,
            Some(0.003625),
            None,
        );
        insert(
            &mut models,
            "deepseek-v4-flash",
            0.14,
            0.28,
            Some(0.0028),
            None,
        );
        insert(&mut models, "deepseek-chat", 0.14, 0.28, Some(0.0028), None);
        insert(
            &mut models,
            "deepseek-reasoner",
            0.14,
            0.28,
            Some(0.0028),
            None,
        );
        insert(&mut models, "deepseek-v3", 0.27, 1.10, None, None);
        insert(&mut models, "deepseek-r1", 0.55, 2.19, None, None);

        // ── OpenCode Zen list prices (when OpenCode doesn't embed cost) ─
        insert(&mut models, "big-pickle", 0.0, 0.0, Some(0.0), None);
        insert(&mut models, "minimax-m2.5", 0.30, 1.20, Some(0.06), None);
        insert(&mut models, "minimax-m2.7", 0.30, 1.20, Some(0.06), None);
        insert(&mut models, "minimax-m3", 0.30, 1.20, Some(0.06), None);
        insert(&mut models, "glm-5.2", 0.60, 2.20, Some(0.20), None);
        insert(&mut models, "glm-5.1", 0.60, 2.20, Some(0.20), None);
        insert(&mut models, "glm-5", 0.60, 2.20, Some(0.20), None);
        insert(&mut models, "glm-4.6", 0.60, 2.20, None, None);
        insert(
            &mut models,
            "qwen3.6-plus",
            0.50,
            3.00,
            Some(0.05),
            Some(0.625),
        );
        insert(
            &mut models,
            "qwen3.6-max",
            2.50,
            7.50,
            Some(0.50),
            Some(3.125),
        );
        insert(
            &mut models,
            "qwen3.6-max-preview",
            2.50,
            7.50,
            Some(0.50),
            Some(3.125),
        );
        insert(
            &mut models,
            "qwen3.7-plus",
            0.40,
            1.60,
            Some(0.04),
            Some(0.50),
        );
        insert(
            &mut models,
            "qwen3.7-max",
            2.50,
            7.50,
            Some(0.50),
            Some(3.125),
        );
        insert(
            &mut models,
            "qwen3.5-plus",
            0.20,
            1.20,
            Some(0.02),
            Some(0.25),
        );
        insert(&mut models, "qwen3.5-122b-a10b", 0.20, 1.20, None, None);
        insert(&mut models, "qwen3-coder", 0.20, 0.80, None, None);
        insert(&mut models, "coder-model", 0.20, 0.80, None, None);
        insert(&mut models, "kimi-k2.7-code", 0.60, 2.50, None, None);
        insert(&mut models, "kimi-k3", 1.00, 4.00, None, None);
        insert(&mut models, "kimi-k2", 0.60, 2.50, None, None);

        // Free / promotional Zen & provider tiers (explicit $0, not "unknown")
        for free_id in [
            "minimax-m2.5-free",
            "qwen3.6-plus-free",
            "mimo-v2.5-free",
            "nemotron-3-super-free",
            "hy3-preview-free",
            "deepseek-v4-flash-free",
            "north-mini-code-free",
        ] {
            insert(&mut models, free_id, 0.0, 0.0, Some(0.0), None);
        }

        // ── Codex opaque provider label ─────────────────────────────────
        // Codex often stores provider id `openai` — use GPT-4.1 as estimate.
        insert(&mut models, "openai", 2.0, 8.0, None, None);

        // ── Misc common OSS / gateway models ────────────────────────────
        insert(&mut models, "llama-4-maverick", 0.20, 0.60, None, None);
        insert(&mut models, "llama-3.3-70b", 0.40, 0.40, None, None);
        insert(&mut models, "llama-3.1-405b", 2.0, 2.0, None, None);
        insert(&mut models, "mistral-large", 2.0, 6.0, None, None);
        insert(&mut models, "mistral-small", 0.10, 0.30, None, None);
        insert(&mut models, "codestral", 0.30, 0.90, None, None);
        insert(&mut models, "grok-3", 3.0, 15.0, None, None);
        insert(&mut models, "grok-3-mini", 0.30, 0.50, None, None);
        insert(&mut models, "grok-2", 2.0, 10.0, None, None);

        let hash = hash_models(&models);
        Self {
            version: "0.2.0-builtin".into(),
            hash,
            models,
            source: "builtin".into(),
            fetched_at: Some(Utc::now().to_rfc3339()),
            ttl_days: default_ttl_days(),
        }
    }

    pub fn load_or_builtin(path: &Path) -> Self {
        match Self::load(path) {
            Ok(bundle) => bundle,
            Err(_) => Self::builtin(),
        }
    }

    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let raw = std::fs::read_to_string(path)?;
        let mut bundle: TariffBundle = serde_json::from_str(&raw)?;
        if bundle.hash.is_empty() {
            bundle.hash = hash_models(&bundle.models);
        }
        if bundle.source.is_empty() {
            bundle.source = "file".into();
        }
        Ok(bundle)
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let raw = serde_json::to_string_pretty(self)?;
        std::fs::write(path, raw)?;
        Ok(())
    }

    pub fn is_stale(&self) -> bool {
        let Some(fetched) = self.fetched_at.as_deref() else {
            return self.source != "builtin";
        };
        let Ok(dt) = chrono::DateTime::parse_from_rfc3339(fetched) else {
            return true;
        };
        let age = Utc::now().signed_duration_since(dt.with_timezone(&Utc));
        age > Duration::days(i64::from(self.ttl_days.max(1)))
    }

    pub fn status_for(&self, path: &Path) -> TariffStatus {
        TariffStatus {
            version: self.version.clone(),
            hash: self.hash.clone(),
            source: self.source.clone(),
            fetched_at: self.fetched_at.clone(),
            ttl_days: self.ttl_days,
            stale: self.is_stale(),
            path: path.display().to_string(),
        }
    }

    pub fn rate_for(&self, model: &str) -> Option<&ModelRate> {
        let lower = model.to_lowercase();
        let trimmed = lower.trim();

        // 1. Exact
        if let Some(r) = self.models.get(trimmed).or_else(|| self.models.get(model)) {
            return Some(r);
        }

        // 2. Explicit aliases (Antigravity / product ids → catalog keys)
        if let Some(canon) = model_alias(trimmed) {
            if let Some(r) = self.models.get(canon) {
                return Some(r);
            }
        }

        // 3. Free-tier suffix without an explicit row
        if trimmed.ends_with("-free") || trimmed.ends_with("_free") {
            if let Some(r) = self.models.get("big-pickle") {
                // big-pickle is the canonical $0/$0 row
                return Some(r);
            }
        }

        // 4. Longest catalog key contained in the model id (prefer longer keys)
        let mut best: Option<(&str, &ModelRate, usize)> = None;
        for (k, v) in &self.models {
            let kl = k.to_lowercase();
            if trimmed.contains(&kl) {
                let score = kl.len();
                if best.map(|(_, _, s)| score > s).unwrap_or(true) {
                    best = Some((k.as_str(), v, score));
                }
            }
        }
        best.map(|(_, v, _)| v)
    }

    pub fn calculate_cost(&self, event: &SpendEvent) -> Option<f64> {
        let rate = self.rate_for(&event.model)?;
        let input = (event.input_tokens as f64 / 1_000_000.0) * rate.input_per_m;
        let output = (event.output_tokens as f64 / 1_000_000.0) * rate.output_per_m;
        let cache_read = rate
            .cache_read_per_m
            .map(|p| (event.cache_read_tokens as f64 / 1_000_000.0) * p)
            .unwrap_or(0.0);
        let cache_write = rate
            .cache_write_per_m
            .map(|p| (event.cache_write_tokens as f64 / 1_000_000.0) * p)
            .unwrap_or(0.0);
        Some(input + output + cache_read + cache_write)
    }

    pub fn apply_costs(&self, events: &mut [SpendEvent]) {
        for e in events {
            if matches!(
                e.accuracy,
                crate::event::Accuracy::Proxy | crate::event::Accuracy::Quota
            ) {
                e.cost_usd = None;
                continue;
            }
            // Preserve collector-provided cost (e.g. Threads / OpenCode embedded).
            if e.cost_usd.is_some() {
                continue;
            }
            e.cost_usd = self.calculate_cost(e);
        }
    }

    /// Fill missing costs for already-stored ledger events (does not overwrite known costs).
    pub fn fill_missing_costs(&self, events: &mut [SpendEvent]) -> usize {
        let mut filled = 0usize;
        for e in events {
            if matches!(
                e.accuracy,
                crate::event::Accuracy::Proxy | crate::event::Accuracy::Quota
            ) {
                continue;
            }
            if e.cost_usd.is_some() {
                continue;
            }
            if let Some(c) = self.calculate_cost(e) {
                e.cost_usd = Some(c);
                filled += 1;
            }
        }
        filled
    }
}

/// Map product / shorthand model ids onto catalog keys.
fn model_alias(model: &str) -> Option<&'static str> {
    Some(match model {
        // Antigravity
        "gemini-pro-default" | "gemini-pro-agent" | "gemini-3.1-pro-high" => "gemini-3.1-pro",
        "gemini-3-flash-a" | "gemini-3-flash-agent" | "gemini-3-flash-high" => "gemini-3.6-flash",
        "gemini-3.1-pro-low" | "gemini-3.1-pro-preview" => "gemini-3.1-pro",
        "gemini-default" | "gemini" => "gemini-3-flash",
        // Qwen Code
        "coder-model" | "qwen-coder" => "qwen3-coder",
        // Codex
        "openai" | "codex" => "gpt-4.1",
        _ => return None,
    })
}

fn hash_models(models: &HashMap<String, ModelRate>) -> String {
    let mut keys: Vec<_> = models.keys().cloned().collect();
    keys.sort();
    let mut hasher = blake3::Hasher::new();
    for k in &keys {
        let rate = &models[k];
        hasher.update(k.as_bytes());
        hasher.update(b"\0");
        hasher.update(&rate.input_per_m.to_le_bytes());
        hasher.update(&rate.output_per_m.to_le_bytes());
    }
    hasher.finalize().to_hex().to_string()
}

fn insert(
    map: &mut HashMap<String, ModelRate>,
    id: &str,
    input: f64,
    output: f64,
    cache_read: Option<f64>,
    cache_write: Option<f64>,
) {
    map.insert(
        id.to_string(),
        ModelRate {
            input_per_m: input,
            output_per_m: output,
            cache_read_per_m: cache_read,
            cache_write_per_m: cache_write,
        },
    );
}

/// Refresh tariffs from a remote URL into `cache_path`.
///
/// Env `RMS_SPEND_TARIFF_URL` is used when `url` is None.
/// Without a URL, rewrites the builtin bundle to the cache (local refresh).
pub fn refresh_tariffs(cache_path: &Path, url: Option<&str>) -> anyhow::Result<TariffBundle> {
    let resolved = url
        .map(str::to_string)
        .or_else(|| std::env::var("RMS_SPEND_TARIFF_URL").ok())
        .filter(|s| !s.trim().is_empty());

    let mut bundle = if let Some(url) = resolved {
        fetch_remote_bundle(&url)?
    } else {
        let mut b = TariffBundle::builtin();
        b.source = "builtin".into();
        b
    };
    bundle.fetched_at = Some(Utc::now().to_rfc3339());
    if bundle.source.is_empty() {
        bundle.source = if url.is_some() || std::env::var("RMS_SPEND_TARIFF_URL").is_ok() {
            "remote".into()
        } else {
            "builtin".into()
        };
    }
    if bundle.hash.is_empty() {
        bundle.hash = hash_models(&bundle.models);
    }
    if bundle.ttl_days == 0 {
        bundle.ttl_days = default_ttl_days();
    }
    bundle.save(cache_path)?;
    Ok(bundle)
}

fn fetch_remote_bundle(url: &str) -> anyhow::Result<TariffBundle> {
    let response = ureq::get(url)
        .call()
        .map_err(|e| anyhow::anyhow!("tariff fetch failed: {e}"))?;
    if !(200..300).contains(&response.status()) {
        anyhow::bail!("tariff fetch HTTP {}", response.status());
    }
    let mut bundle: TariffBundle = response
        .into_json()
        .map_err(|e| anyhow::anyhow!("tariff JSON parse failed: {e}"))?;
    bundle.source = "remote".into();
    Ok(bundle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Accuracy;
    use chrono::Utc;

    #[test]
    fn unknown_model_is_none_not_zero() {
        let bundle = TariffBundle::builtin();
        let mut ev = SpendEvent {
            event_id: "t".into(),
            source: "test".into(),
            client: "test".into(),
            model: "totally-unknown-model-xyz".into(),
            input_tokens: 1000,
            output_tokens: 1000,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            cost_usd: None,
            currency: "USD".into(),
            accuracy: Accuracy::Exact,
            occurred_at: Utc::now(),
            session_id: None,
            project_slug: None,
        };
        assert!(bundle.calculate_cost(&ev).is_none());
        bundle.apply_costs(std::slice::from_mut(&mut ev));
        assert!(ev.cost_usd.is_none());
    }

    #[test]
    fn known_model_has_cost() {
        let bundle = TariffBundle::builtin();
        let ev = SpendEvent {
            event_id: "t".into(),
            source: "test".into(),
            client: "test".into(),
            model: "claude-sonnet-4-20250514".into(),
            input_tokens: 1_000_000,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            cost_usd: None,
            currency: "USD".into(),
            accuracy: Accuracy::Exact,
            occurred_at: Utc::now(),
            session_id: None,
            project_slug: None,
        };
        let cost = bundle.calculate_cost(&ev).unwrap();
        assert!((cost - 3.0).abs() < 0.01);
    }

    #[test]
    fn gemini_antigravity_alias_has_cost() {
        let bundle = TariffBundle::builtin();
        let ev = SpendEvent {
            event_id: "t".into(),
            source: "antigravity".into(),
            client: "antigravity".into(),
            model: "gemini-pro-default".into(),
            input_tokens: 1_000_000,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            cost_usd: None,
            currency: "USD".into(),
            accuracy: Accuracy::Exact,
            occurred_at: Utc::now(),
            session_id: None,
            project_slug: None,
        };
        let cost = bundle.calculate_cost(&ev).unwrap();
        assert!((cost - 2.0).abs() < 0.01);
    }

    #[test]
    fn free_model_is_zero_not_unknown() {
        let bundle = TariffBundle::builtin();
        let cost = bundle
            .calculate_cost(&SpendEvent {
                event_id: "t".into(),
                source: "opencode".into(),
                client: "opencode".into(),
                model: "big-pickle".into(),
                input_tokens: 1_000_000,
                output_tokens: 500_000,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                cost_usd: None,
                currency: "USD".into(),
                accuracy: Accuracy::Exact,
                occurred_at: Utc::now(),
                session_id: None,
                project_slug: None,
            })
            .unwrap();
        assert_eq!(cost, 0.0);
    }

    #[test]
    fn longest_match_prefers_mini_over_base() {
        let bundle = TariffBundle::builtin();
        let cost = bundle
            .calculate_cost(&SpendEvent {
                event_id: "t".into(),
                source: "test".into(),
                client: "test".into(),
                model: "gpt-4o-mini-2024-07-18".into(),
                input_tokens: 1_000_000,
                output_tokens: 0,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                cost_usd: None,
                currency: "USD".into(),
                accuracy: Accuracy::Exact,
                occurred_at: Utc::now(),
                session_id: None,
                project_slug: None,
            })
            .unwrap();
        assert!((cost - 0.15).abs() < 0.01);
    }

    #[test]
    fn stale_when_fetched_old() {
        let mut bundle = TariffBundle::builtin();
        bundle.ttl_days = 7;
        bundle.fetched_at = Some((Utc::now() - Duration::days(10)).to_rfc3339());
        assert!(bundle.is_stale());
        bundle.fetched_at = Some(Utc::now().to_rfc3339());
        assert!(!bundle.is_stale());
    }
}
