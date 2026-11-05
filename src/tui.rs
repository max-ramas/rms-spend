//! Interactive terminal UI for spend summaries.

use crate::paths::SpendPaths;
use crate::period::{PeriodFilter, PeriodPreset};
use crate::service::{summary_filtered, SummaryQuery};
use crate::summary::{format_tokens_short, SummaryReport};
use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Row, Table};
use ratatui::Frame;
use std::io::stdout;
use std::time::Duration;

struct App {
    period: PeriodPreset,
    report: SummaryReport,
    error: Option<String>,
    budget_line: Option<String>,
}

pub fn run_tui(paths: &SpendPaths) -> Result<()> {
    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    let mut terminal = ratatui::init();
    let result = run_loop(paths, &mut terminal);
    disable_raw_mode()?;
    stdout().execute(LeaveAlternateScreen)?;
    ratatui::restore();
    result
}

fn run_loop(paths: &SpendPaths, terminal: &mut ratatui::DefaultTerminal) -> Result<()> {
    let mut period = PeriodPreset::Today;
    let mut app = load_app(paths, period)?;

    loop {
        terminal.draw(|frame| draw(frame, &app))?;

        if event::poll(Duration::from_millis(200))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    KeyCode::Char('r') => app = load_app(paths, app.period)?,
                    KeyCode::Char('1') | KeyCode::Char('t') => {
                        period = PeriodPreset::Today;
                        app = load_app(paths, period)?;
                    }
                    KeyCode::Char('2') | KeyCode::Char('w') => {
                        period = PeriodPreset::Week;
                        app = load_app(paths, period)?;
                    }
                    KeyCode::Char('3') | KeyCode::Char('m') => {
                        period = PeriodPreset::Month;
                        app = load_app(paths, period)?;
                    }
                    KeyCode::Char('4') | KeyCode::Char('a') => {
                        period = PeriodPreset::All;
                        app = load_app(paths, period)?;
                    }
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => break,
                    _ => {}
                }
            }
        }
    }
    Ok(())
}

fn load_app(paths: &SpendPaths, period: PeriodPreset) -> Result<App> {
    let filter = PeriodFilter {
        preset: period,
        ..Default::default()
    };
    let budget_cfg = crate::budget::BudgetConfig::load_or_none(&paths.budget_json)
        .ok()
        .flatten();
    match summary_filtered(
        paths,
        &SummaryQuery {
            period: filter,
            ..Default::default()
        },
    ) {
        Ok(report) => {
            let budget_line = budget_cfg.and_then(|b| b.format_vs_cost(report.cost_usd));
            Ok(App {
                period,
                report,
                error: None,
                budget_line,
            })
        }
        Err(e) => Ok(App {
            period,
            report: summarize_empty(period),
            error: Some(e.to_string()),
            budget_line: None,
        }),
    }
}

fn summarize_empty(period: PeriodPreset) -> SummaryReport {
    let mut r = crate::summary::summarize(&[]);
    r.period = Some(period.as_str().into());
    r
}

fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(7),
            Constraint::Min(4),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(area);

    draw_tabs(chunks[0], app.period, frame);
    draw_totals(chunks[1], app, frame);
    draw_token_sources(chunks[2], app, frame);
    draw_side_panels(chunks[3], app, frame);
    let footer = Paragraph::new(" q quit · r refresh · 1 Today · 2 Week · 3 Month · 4 All ")
        .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(footer, chunks[4]);
}

fn draw_tabs(area: Rect, active: PeriodPreset, frame: &mut Frame) {
    let labels = [
        (PeriodPreset::Today, "1 Today"),
        (PeriodPreset::Week, "2 Week"),
        (PeriodPreset::Month, "3 Month"),
        (PeriodPreset::All, "4 All"),
    ];
    let spans: Vec<Span> = labels
        .iter()
        .map(|(p, label)| {
            if *p == active {
                Span::styled(
                    format!(" {label} "),
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                Span::raw(format!(" {label} "))
            }
        })
        .collect();
    let w = Paragraph::new(Line::from(spans))
        .block(Block::default().borders(Borders::ALL).title(" RMS Spend "));
    frame.render_widget(w, area);
}

fn draw_totals(area: Rect, app: &App, frame: &mut Frame) {
    let r = &app.report;
    let cost = r
        .cost_usd
        .map(|c| format!("${c:.2}"))
        .unwrap_or_else(|| "—".into());
    let mut lines = vec![
        Line::from(format!(
            "Events {}  ·  Tokens {}  ·  in {} / out {}",
            r.billable_event_count,
            format_tokens_short(r.total_tokens),
            format_tokens_short(r.input_tokens),
            format_tokens_short(r.output_tokens),
        )),
        Line::from(format!("Cost {cost}")),
    ];
    if r.proxy_event_count > 0 {
        lines.push(Line::from(format!(
            "Activity {} sug / {} acc lines",
            format_tokens_short(r.proxy_suggested_lines),
            format_tokens_short(r.proxy_accepted_lines),
        )));
    }
    if r.quota_event_count > 0 {
        lines.push(Line::from(format!(
            "Quotas {} units ({} snaps)",
            format_tokens_short(r.quota_units),
            r.quota_event_count
        )));
    }
    if let Some(b) = &app.budget_line {
        lines.push(Line::from(Span::styled(
            b.clone(),
            Style::default().fg(Color::Cyan),
        )));
    }
    if let Some(err) = &app.error {
        lines.push(Line::from(Span::styled(
            err.clone(),
            Style::default().fg(Color::Red),
        )));
    }
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Totals / Общий итог "),
        ),
        area,
    );
}

fn draw_token_sources(area: Rect, app: &App, frame: &mut Frame) {
    let mut rows_data: Vec<_> = app
        .report
        .by_source
        .iter()
        .filter(|(_, b)| b.events > 0 && !b.is_proxy && !b.is_quota)
        .map(|(src, b)| {
            let units =
                b.input_tokens + b.output_tokens + b.cache_read_tokens + b.cache_write_tokens;
            (src.clone(), b.events, units, b.cost_usd)
        })
        .collect();
    rows_data.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.cmp(&b.0)));

    let rows: Vec<Row> = rows_data
        .into_iter()
        .map(|(src, events, units, cost)| {
            Row::new(vec![
                crate::labels::source_label(&src),
                events.to_string(),
                format_tokens_short(units),
                cost.map(|c| format!("${c:.2}"))
                    .unwrap_or_else(|| "—".into()),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(20),
            Constraint::Length(8),
            Constraint::Length(12),
            Constraint::Length(10),
        ],
    )
    .header(
        Row::new(vec!["Source", "Events", "Tokens", "Cost"])
            .style(Style::default().add_modifier(Modifier::BOLD)),
    )
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Active consumers "),
    );
    frame.render_widget(table, area);
}

fn draw_side_panels(area: Rect, app: &App, frame: &mut Frame) {
    let mut lines = Vec::new();
    let proxies: Vec<_> = app
        .report
        .by_source
        .iter()
        .filter(|(_, b)| b.events > 0 && b.is_proxy)
        .collect();
    if !proxies.is_empty() {
        lines.push(Line::from(Span::styled(
            "Activity (lines)",
            Style::default().add_modifier(Modifier::BOLD),
        )));
        for (src, b) in proxies {
            lines.push(Line::from(format!(
                "  {}  {} lines · {} ev",
                crate::labels::source_label(src),
                format_tokens_short(b.input_tokens + b.output_tokens),
                b.events
            )));
        }
    }
    let quotas: Vec<_> = app
        .report
        .by_source
        .iter()
        .filter(|(_, b)| b.events > 0 && b.is_quota)
        .collect();
    if !quotas.is_empty() {
        if !lines.is_empty() {
            lines.push(Line::from(""));
        }
        lines.push(Line::from(Span::styled(
            "Quotas / credits",
            Style::default().add_modifier(Modifier::BOLD),
        )));
        for (src, b) in quotas {
            lines.push(Line::from(format!(
                "  {}  {} units · {} ev",
                crate::labels::source_label(src),
                format_tokens_short(
                    b.input_tokens + b.output_tokens + b.cache_read_tokens + b.cache_write_tokens
                ),
                b.events
            )));
        }
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "(no line proxies or quota snaps)",
            Style::default().fg(Color::DarkGray),
        )));
    }
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Activity / Quotas "),
        ),
        area,
    );
}
