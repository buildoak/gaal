use chrono::{DateTime, Duration, Local};

use crate::model::SessionRecord;
use crate::output::HumanReadable;

/// Print a simple column-aligned table.
pub fn print_table(headers: &[&str], rows: &[Vec<String>]) {
    if headers.is_empty() {
        return;
    }

    let cols = headers.len();
    let mut widths: Vec<usize> = headers.iter().map(|h| h.len()).collect();

    for row in rows {
        for (i, cell) in row.iter().enumerate().take(cols) {
            widths[i] = widths[i].max(cell.chars().count());
        }
    }

    let render_row = |cells: Vec<String>| -> String {
        let mut parts = Vec::new();
        for (i, cell) in cells.into_iter().enumerate().take(cols) {
            let width = widths.get(i).copied().unwrap_or(0);
            parts.push(format!("{cell:<width$}"));
        }
        parts.join("  ")
    };

    println!(
        "{}",
        render_row(headers.iter().map(|h| h.to_string()).collect())
    );
    println!(
        "{}",
        widths
            .iter()
            .map(|w| "-".repeat(*w))
            .collect::<Vec<_>>()
            .join("  ")
    );
    for row in rows {
        println!("{}", render_row(row.clone()));
    }
}

/// Format seconds as a compact human-friendly duration.
pub fn format_duration(secs: i64) -> String {
    if secs < 0 {
        return "-".to_string();
    }
    if secs >= 86_400 {
        return format!("{}d {}h", secs / 86_400, (secs % 86_400) / 3_600);
    }
    if secs >= 3_600 {
        return format!("{}h {}m", secs / 3_600, (secs % 3_600) / 60);
    }
    if secs >= 60 {
        return format!("{}m {}s", secs / 60, secs % 60);
    }
    format!("{secs}s")
}

/// Format token counts as compact suffix values.
pub fn format_tokens(n: i64) -> String {
    if n.abs() >= 1_000_000 {
        return format!("{:.1}M", n as f64 / 1_000_000.0);
    }
    if n.abs() >= 1_000 {
        return format!("{}K", n / 1_000);
    }
    n.to_string()
}

/// Format an RFC3339 timestamp for terminal display.
pub fn format_timestamp(ts: &str) -> String {
    let Ok(parsed) = DateTime::parse_from_rfc3339(ts) else {
        return ts.to_string();
    };
    let local = parsed.with_timezone(&Local);
    let now = Local::now();

    if local.date_naive() == now.date_naive() {
        return local.format("today %H:%M").to_string();
    }
    if local.date_naive() == (now - Duration::days(1)).date_naive() {
        return local.format("yday %H:%M").to_string();
    }
    local.format("%b %d %H:%M").to_string()
}

impl HumanReadable for Vec<SessionRecord> {
    fn print_human(&self) {
        if self.is_empty() {
            println!("No sessions.");
            return;
        }

        let headers = [
            "ID", "Engine", "Status", "Started", "Duration", "Tokens", "Model", "CWD",
        ];
        let rows: Vec<Vec<String>> = self
            .iter()
            .map(|session| {
                let id = session.id.chars().take(8).collect::<String>();
                let tokens = format!(
                    "{} / {}",
                    format_tokens(session.tokens.input as i64),
                    format_tokens(session.tokens.output as i64)
                );
                vec![
                    id,
                    session.engine.clone(),
                    session.status.clone(),
                    format_timestamp(&session.started_at),
                    format_duration(session.duration_secs as i64),
                    tokens,
                    session.model.clone(),
                    session.cwd.clone(),
                ]
            })
            .collect();
        print_table(&headers, &rows);
    }
}
