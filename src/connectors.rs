//! Per-connector enable/disable flags (explicit opt-in UX).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

pub const KNOWN_COLLECTORS: &[&str] = &[
    "claude_code",
    "rms_memory",
    "cursor_proxy",
    "cursor_quota",
    "rms_threads",
    "opencode",
    "codex",
    "qwen_code",
    "zcode",
    "gemini_cli",
    "antigravity",
    "vscode_copilot",
    "zed",
    "windsurf",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorConfig {
    /// collector_id → enabled
    pub enabled: BTreeMap<String, bool>,
    /// collector_id → allow outbound network (e.g. Windsurf GetUserStatus). Default off.
    #[serde(default)]
    pub allow_network: BTreeMap<String, bool>,
}

impl Default for ConnectorConfig {
    fn default() -> Self {
        let mut enabled = BTreeMap::new();
        for id in KNOWN_COLLECTORS {
            // Quota-style collectors are opt-in (not token ledgers).
            let on = *id != "cursor_quota" && *id != "windsurf";
            enabled.insert((*id).to_string(), on);
        }
        Self {
            enabled,
            allow_network: BTreeMap::new(),
        }
    }
}

impl ConnectorConfig {
    pub fn load_or_default(path: &Path) -> Self {
        Self::load(path).unwrap_or_default()
    }

    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let raw = std::fs::read_to_string(path)?;
        let mut cfg: ConnectorConfig = serde_json::from_str(&raw)?;
        for id in KNOWN_COLLECTORS {
            let default_on = *id != "cursor_quota" && *id != "windsurf";
            cfg.enabled.entry((*id).to_string()).or_insert(default_on);
        }
        Ok(cfg)
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let raw = serde_json::to_string_pretty(self)?;
        std::fs::write(path, raw)?;
        Ok(())
    }

    pub fn is_enabled(&self, id: &str) -> bool {
        self.enabled.get(id).copied().unwrap_or(true)
    }

    pub fn set_enabled(&mut self, id: &str, on: bool) {
        self.enabled.insert(id.to_string(), on);
    }

    pub fn network_allowed(&self, id: &str) -> bool {
        self.allow_network.get(id).copied().unwrap_or(false)
    }

    pub fn set_network_allowed(&mut self, id: &str, on: bool) {
        self.allow_network.insert(id.to_string(), on);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn roundtrip() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("connectors.json");
        let mut cfg = ConnectorConfig::default();
        cfg.set_enabled("cursor_proxy", false);
        cfg.save(&path).unwrap();
        let loaded = ConnectorConfig::load(&path).unwrap();
        assert!(!loaded.is_enabled("cursor_proxy"));
        assert!(loaded.is_enabled("claude_code"));
    }
}
