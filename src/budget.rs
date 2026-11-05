//! Optional spend budget (`~/.rms-memory/spend-budget.json`).

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BudgetPeriod {
    Day,
    Week,
    Month,
    All,
}

impl BudgetPeriod {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Day => "day",
            Self::Week => "week",
            Self::Month => "month",
            Self::All => "all",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "day" | "today" => Ok(Self::Day),
            "week" => Ok(Self::Week),
            "month" => Ok(Self::Month),
            "all" => Ok(Self::All),
            other => bail!("unknown budget period: {other} (day|week|month|all)"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BudgetConfig {
    pub usd: f64,
    pub period: BudgetPeriod,
}

impl BudgetConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path)?;
        Ok(serde_json::from_str(&raw)?)
    }

    pub fn load_or_none(path: &Path) -> Result<Option<Self>> {
        if !path.exists() {
            return Ok(None);
        }
        Ok(Some(Self::load(path)?))
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    pub fn clear(path: &Path) -> Result<()> {
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        Ok(())
    }

    /// Human line for status / TUI: `cost $X / budget $Y (N%)` or over-budget warning.
    pub fn format_vs_cost(&self, cost_usd: Option<f64>) -> Option<String> {
        let used = cost_usd.unwrap_or(0.0);
        if self.usd <= 0.0 {
            return None;
        }
        let pct = (used / self.usd) * 100.0;
        if used > self.usd {
            Some(format!(
                "Budget OVER ${used:.2} / ${:.2} ({:.0}%) · {}",
                self.usd,
                pct,
                self.period.as_str()
            ))
        } else {
            Some(format!(
                "Budget ${used:.2} / ${:.2} ({:.0}%) · {}",
                self.usd,
                pct,
                self.period.as_str()
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn roundtrip_and_format() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("budget.json");
        let cfg = BudgetConfig {
            usd: 50.0,
            period: BudgetPeriod::Month,
        };
        cfg.save(&path).unwrap();
        let loaded = BudgetConfig::load(&path).unwrap();
        assert_eq!(loaded.usd, 50.0);
        let line = loaded.format_vs_cost(Some(12.5)).unwrap();
        assert!(line.contains("12.50"));
        assert!(line.contains("50.00"));
        let over = loaded.format_vs_cost(Some(60.0)).unwrap();
        assert!(over.contains("OVER"));
    }
}
