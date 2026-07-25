use crate::event::SpendEvent;
use anyhow::Result;
use std::io::Write;
use std::path::Path;

pub fn export_json(events: &[SpendEvent], path: &Path) -> Result<()> {
    let raw = serde_json::to_string_pretty(events)?;
    std::fs::write(path, raw)?;
    Ok(())
}

pub fn export_csv(events: &[SpendEvent], path: &Path) -> Result<()> {
    let mut f = std::fs::File::create(path)?;
    writeln!(
        f,
        "event_id,source,client,model,input_tokens,output_tokens,cache_read_tokens,cache_write_tokens,cost_usd,currency,accuracy,occurred_at,session_id,project_slug"
    )?;
    for e in events {
        writeln!(
            f,
            "{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
            csv_escape(&e.event_id),
            csv_escape(&e.source),
            csv_escape(&e.client),
            csv_escape(&e.model),
            e.input_tokens,
            e.output_tokens,
            e.cache_read_tokens,
            e.cache_write_tokens,
            e.cost_usd.map(|c| format!("{c:.6}")).unwrap_or_default(),
            csv_escape(&e.currency),
            e.accuracy.as_str(),
            e.occurred_at.to_rfc3339(),
            csv_escape(e.session_id.as_deref().unwrap_or("")),
            csv_escape(e.project_slug.as_deref().unwrap_or("")),
        )?;
    }
    Ok(())
}

fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}
