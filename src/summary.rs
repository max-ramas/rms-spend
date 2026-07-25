use crate::event::{Accuracy, SpendEvent};
use chrono::{DateTime, Datelike, Utc};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SummaryReport {
    /// Applied period preset (`today`, `week`, …) when filtered.
    pub period: Option<String>,
    pub filter_from: Option<String>,
    pub filter_to: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub event_count: usize,
    /// Provider / estimated token rows only (`exact` + `estimated`).
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub total_tokens: u64,
    /// Cursor-style line proxies (`accuracy=proxy`) — not API tokens.
    pub proxy_suggested_lines: u64,
    pub proxy_accepted_lines: u64,
    pub proxy_event_count: usize,
    pub billable_event_count: usize,
    /// Sum of known `cost_usd` (never invents price for proxy/unknown models).
    pub cost_usd: Option<f64>,
    pub cost_known_events: usize,
    pub cost_unknown_events: usize,
    pub by_source: BTreeMap<String, Bucket>,
    pub by_model: BTreeMap<String, Bucket>,
    pub by_day: BTreeMap<String, Bucket>,
    pub by_accuracy: BTreeMap<String, usize>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Bucket {
    pub events: usize,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub cost_usd: Option<f64>,
    /// True when this bucket mixes proxy line metrics (print carefully).
    pub is_proxy: bool,
}

impl Bucket {
    fn add(&mut self, e: &SpendEvent) {
        self.events += 1;
        if e.accuracy == Accuracy::Proxy {
            self.is_proxy = true;
        }
        self.input_tokens += e.input_tokens;
        self.output_tokens += e.output_tokens;
        self.cache_read_tokens += e.cache_read_tokens;
        self.cache_write_tokens += e.cache_write_tokens;
        match (self.cost_usd, e.cost_usd) {
            (None, Some(c)) if self.events == 1 => self.cost_usd = Some(c),
            (Some(a), Some(c)) => self.cost_usd = Some(a + c),
            (Some(_), None) => {}
            (None, None) => self.cost_usd = None,
            (None, Some(c)) => {
                self.cost_usd = Some(c);
            }
        }
    }

    fn units_label_total(&self) -> u64 {
        self.input_tokens + self.output_tokens + self.cache_read_tokens + self.cache_write_tokens
    }
}

fn is_billable(a: Accuracy) -> bool {
    matches!(a, Accuracy::Exact | Accuracy::Estimated)
}

pub fn summarize(events: &[SpendEvent]) -> SummaryReport {
    let mut by_source: BTreeMap<String, Bucket> = BTreeMap::new();
    let mut by_model: BTreeMap<String, Bucket> = BTreeMap::new();
    let mut by_day: BTreeMap<String, Bucket> = BTreeMap::new();
    let mut by_accuracy: BTreeMap<String, usize> = BTreeMap::new();

    let mut input = 0u64;
    let mut output = 0u64;
    let mut cache_read = 0u64;
    let mut cache_write = 0u64;
    let mut proxy_suggested = 0u64;
    let mut proxy_accepted = 0u64;
    let mut proxy_events = 0usize;
    let mut billable_events = 0usize;
    let mut cost_sum = 0.0;
    let mut cost_known = 0usize;
    let mut cost_unknown = 0usize;
    let mut min_ts: Option<DateTime<Utc>> = None;
    let mut max_ts: Option<DateTime<Utc>> = None;

    for e in events {
        min_ts = Some(min_ts.map_or(e.occurred_at, |m| m.min(e.occurred_at)));
        max_ts = Some(max_ts.map_or(e.occurred_at, |m| m.max(e.occurred_at)));

        if is_billable(e.accuracy) {
            billable_events += 1;
            input += e.input_tokens;
            output += e.output_tokens;
            cache_read += e.cache_read_tokens;
            cache_write += e.cache_write_tokens;
        } else if e.accuracy == Accuracy::Proxy {
            proxy_events += 1;
            proxy_suggested += e.input_tokens;
            proxy_accepted += e.output_tokens;
        }

        match e.cost_usd {
            Some(c) => {
                cost_sum += c;
                cost_known += 1;
            }
            None => cost_unknown += 1,
        }

        by_source.entry(e.source.clone()).or_default().add(e);
        by_model.entry(e.model.clone()).or_default().add(e);
        let day = format!(
            "{:04}-{:02}-{:02}",
            e.occurred_at.year(),
            e.occurred_at.month(),
            e.occurred_at.day()
        );
        by_day.entry(day).or_default().add(e);
        *by_accuracy.entry(e.accuracy.as_str().into()).or_default() += 1;
    }

    SummaryReport {
        period: None,
        filter_from: None,
        filter_to: None,
        from: min_ts.map(|t| t.to_rfc3339()),
        to: max_ts.map(|t| t.to_rfc3339()),
        event_count: events.len(),
        input_tokens: input,
        output_tokens: output,
        cache_read_tokens: cache_read,
        cache_write_tokens: cache_write,
        total_tokens: input + output + cache_read + cache_write,
        proxy_suggested_lines: proxy_suggested,
        proxy_accepted_lines: proxy_accepted,
        proxy_event_count: proxy_events,
        billable_event_count: billable_events,
        cost_usd: if cost_known > 0 { Some(cost_sum) } else { None },
        cost_known_events: cost_known,
        cost_unknown_events: cost_unknown,
        by_source,
        by_model,
        by_day,
        by_accuracy,
    }
}

pub fn print_table(report: &SummaryReport) {
    println!("RMS Spend Summary");
    println!("─────────────────");
    if let Some(p) = &report.period {
        print!("Period:    {p}");
        if let (Some(from), Some(to)) = (&report.filter_from, &report.filter_to) {
            println!(" ({from} → {to})");
        } else {
            println!();
        }
    }
    if let (Some(from), Some(to)) = (&report.from, &report.to) {
        println!("Data:      {from} → {to}");
    }
    println!(
        "Events:    {} ({} billable, {} proxy)",
        report.event_count, report.billable_event_count, report.proxy_event_count
    );
    println!(
        "Tokens:    {} in / {} out / {} cache-read / {} cache-write",
        report.input_tokens,
        report.output_tokens,
        report.cache_read_tokens,
        report.cache_write_tokens
    );
    println!(
        "Total:     {} (API / estimated tokens only)",
        report.total_tokens
    );
    if report.proxy_event_count > 0 {
        println!(
            "Proxy:     {} suggested / {} accepted lines (Cursor — not tokens)",
            report.proxy_suggested_lines, report.proxy_accepted_lines
        );
    }
    match report.cost_usd {
        Some(c) => {
            print!("Cost USD:  ${c:.4}");
            if report.cost_unknown_events > 0 {
                print!(
                    " ({} events with unknown/null price)",
                    report.cost_unknown_events
                );
            }
            println!();
        }
        None => println!("Cost USD:  unknown"),
    }
    if !report.by_accuracy.is_empty() {
        print!("Accuracy: ");
        let parts: Vec<_> = report
            .by_accuracy
            .iter()
            .map(|(k, n)| format!("{k}={n}"))
            .collect();
        println!("{}", parts.join(", "));
    }
    println!();
    println!("Active consumers:");
    let mut sources: Vec<_> = report
        .by_source
        .iter()
        .filter(|(_, b)| b.events > 0)
        .collect();
    sources.sort_by(|a, b| {
        b.1.units_label_total()
            .cmp(&a.1.units_label_total())
            .then_with(|| a.0.cmp(b.0))
    });
    if sources.is_empty() {
        println!("  (none)");
    } else {
        for (k, b) in sources {
            let unit = if b.is_proxy { "units" } else { "tokens" };
            println!(
                "  {:<22} events={:<5} {unit}={:<10} cost={}",
                crate::labels::source_label(k),
                b.events,
                b.units_label_total(),
                format_cost(b.cost_usd)
            );
        }
    }
    println!();
    println!("By model:");
    let mut models: Vec<_> = report
        .by_model
        .iter()
        .filter(|(_, b)| b.events > 0)
        .collect();
    models.sort_by(|a, b| {
        b.1.units_label_total()
            .cmp(&a.1.units_label_total())
            .then_with(|| a.0.cmp(b.0))
    });
    if models.is_empty() {
        println!("  (none)");
    } else {
        for (k, b) in models {
            let unit = if b.is_proxy { "units" } else { "tokens" };
            println!(
                "  {k:<40} events={:<5} {unit}={:<10} cost={}",
                b.events,
                b.units_label_total(),
                format_cost(b.cost_usd)
            );
        }
    }
    if !report.by_day.is_empty() {
        println!();
        println!("By day (UTC):");
        for (k, b) in report.by_day.iter().filter(|(_, b)| b.events > 0) {
            let unit = if b.is_proxy { "units" } else { "tokens" };
            println!(
                "  {k}  events={:<5} {unit}={:<10} cost={}",
                b.events,
                b.units_label_total(),
                format_cost(b.cost_usd)
            );
        }
    }
}

fn format_cost(c: Option<f64>) -> String {
    match c {
        Some(v) => format!("${v:.4}"),
        None => "unknown".into(),
    }
}

/// Compact token count for tray / TUI (1.2M, 45k, …).
pub fn format_tokens_short(n: u64) -> String {
    if n >= 1_000_000_000 {
        format!("{:.1}B", n as f64 / 1_000_000_000.0)
    } else if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn ev(acc: Accuracy, inn: u64, out: u64, cost: Option<f64>) -> SpendEvent {
        SpendEvent {
            event_id: format!("{acc:?}-{inn}"),
            source: "t".into(),
            client: "t".into(),
            model: "m".into(),
            input_tokens: inn,
            output_tokens: out,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            cost_usd: cost,
            currency: "USD".into(),
            accuracy: acc,
            occurred_at: Utc::now(),
            session_id: None,
            project_slug: None,
        }
    }

    #[test]
    fn proxy_does_not_inflate_token_totals() {
        let events = vec![
            ev(Accuracy::Exact, 100, 50, Some(0.01)),
            ev(Accuracy::Proxy, 1000, 2000, None),
        ];
        let r = summarize(&events);
        assert_eq!(r.total_tokens, 150);
        assert_eq!(r.proxy_suggested_lines, 1000);
        assert_eq!(r.proxy_accepted_lines, 2000);
        assert_eq!(r.billable_event_count, 1);
        assert_eq!(r.proxy_event_count, 1);
        assert!((r.cost_usd.unwrap() - 0.01).abs() < 1e-9);
    }
}
