//! Time-window filters for summary / export / TUI / tray.

use anyhow::{bail, Result};
use chrono::{DateTime, Datelike, Duration, Local, TimeZone, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PeriodPreset {
    #[default]
    All,
    Today,
    Week,
    Month,
    Range,
}

impl PeriodPreset {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Today => "today",
            Self::Week => "week",
            Self::Month => "month",
            Self::Range => "range",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "all" | "total" | "всего" => Ok(Self::All),
            "today" | "day" | "сегодня" => Ok(Self::Today),
            "week" | "7d" | "неделя" => Ok(Self::Week),
            "month" | "30d" | "месяц" => Ok(Self::Month),
            "range" | "period" | "custom" | "период" => Ok(Self::Range),
            other => bail!("unknown period: {other} (use all|today|week|month|range)"),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All time",
            Self::Today => "Today",
            Self::Week => "Last 7 days",
            Self::Month => "This month",
            Self::Range => "Custom range",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PeriodFilter {
    pub preset: PeriodPreset,
    /// Inclusive lower bound (UTC). Required for `Range`.
    pub from: Option<DateTime<Utc>>,
    /// Inclusive upper bound (UTC). Required for `Range`.
    pub to: Option<DateTime<Utc>>,
}

impl PeriodFilter {
    pub fn all() -> Self {
        Self {
            preset: PeriodPreset::All,
            ..Default::default()
        }
    }

    pub fn today() -> Self {
        Self {
            preset: PeriodPreset::Today,
            ..Default::default()
        }
    }

    pub fn week() -> Self {
        Self {
            preset: PeriodPreset::Week,
            ..Default::default()
        }
    }

    pub fn month() -> Self {
        Self {
            preset: PeriodPreset::Month,
            ..Default::default()
        }
    }

    /// Resolve to inclusive UTC bounds. `None`/`None` means no filter (`All`).
    pub fn bounds(&self) -> Result<(Option<DateTime<Utc>>, Option<DateTime<Utc>>)> {
        match self.preset {
            PeriodPreset::All => Ok((None, None)),
            PeriodPreset::Today => {
                let local = Local::now();
                let start = local.date_naive().and_hms_opt(0, 0, 0).unwrap();
                let end = local.date_naive().and_hms_opt(23, 59, 59).unwrap();
                Ok((Some(local_to_utc(start)), Some(local_to_utc(end))))
            }
            PeriodPreset::Week => {
                let local = Local::now();
                let end = local.date_naive().and_hms_opt(23, 59, 59).unwrap();
                let start_day = local.date_naive() - Duration::days(6);
                let start = start_day.and_hms_opt(0, 0, 0).unwrap();
                Ok((Some(local_to_utc(start)), Some(local_to_utc(end))))
            }
            PeriodPreset::Month => {
                let local = Local::now();
                let start = local
                    .date_naive()
                    .with_day(1)
                    .unwrap()
                    .and_hms_opt(0, 0, 0)
                    .unwrap();
                let end = local.date_naive().and_hms_opt(23, 59, 59).unwrap();
                Ok((Some(local_to_utc(start)), Some(local_to_utc(end))))
            }
            PeriodPreset::Range => {
                let from = self
                    .from
                    .ok_or_else(|| anyhow::anyhow!("--from required for period=range"))?;
                let to = self
                    .to
                    .ok_or_else(|| anyhow::anyhow!("--to required for period=range"))?;
                if from > to {
                    bail!("--from must be before --to");
                }
                Ok((Some(from), Some(to)))
            }
        }
    }
}

fn local_to_utc(naive: chrono::NaiveDateTime) -> DateTime<Utc> {
    Local
        .from_local_datetime(&naive)
        .single()
        .map(|t| t.with_timezone(&Utc))
        .unwrap_or_else(|| Utc.from_utc_datetime(&naive))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn today_has_bounds() {
        let (from, to) = PeriodFilter::today().bounds().unwrap();
        assert!(from.is_some() && to.is_some());
        assert!(from.unwrap() <= to.unwrap());
    }

    #[test]
    fn all_has_no_bounds() {
        let (from, to) = PeriodFilter::all().bounds().unwrap();
        assert!(from.is_none() && to.is_none());
    }
}
