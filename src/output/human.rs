use chrono::{DateTime, Duration, Local};

use crate::model::SessionRecord;
use crate::output::HumanReadable;

/// Get the terminal width.
///
/// Priority: `terminal_size` crate (ioctl) > `COLUMNS` env var > 120 default.
fn terminal_width() -> usize {
    terminal_size::terminal_size()
        .map(|(w, _)| w.0 as usize)
        .or_else(|| {
            std::env::var("COLUMNS")
                .ok()
                .and_then(|v| v.parse::<usize>().ok())
                .filter(|&w| w > 0)
        })
        .unwrap_or(120)
}

/// Truncate a CWD path to fit within `max_width` by showing the last meaningful
/// path components with a `...` prefix.
///
/// Strategy:
/// 1. If the path fits, return as-is.
/// 2. Try `.../<last-2-components>`. If that fits, use it.
/// 3. Try `.../<last-component>`. If that fits, use it.
/// 4. Otherwise, truncate the last component with `...` suffix.
pub fn format_cwd(path: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if path.chars().count() <= max_width {
        return path.to_string();
    }

    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    if parts.is_empty() {
        return path.to_string();
    }

    // Try last 3 components
    if parts.len() >= 3 {
        let candidate = format!(".../{}", parts[parts.len() - 3..].join("/"));
        if candidate.chars().count() <= max_width {
            return candidate;
        }
    }

    // Try last 2 components
    if parts.len() >= 2 {
        let candidate = format!(".../{}", parts[parts.len() - 2..].join("/"));
        if candidate.chars().count() <= max_width {
            return candidate;
        }
    }

    // Try last 1 component
    let candidate = format!(".../{}", parts[parts.len() - 1]);
    if candidate.chars().count() <= max_width {
        return candidate;
    }

    // Last resort: truncate the last component itself
    let last = parts[parts.len() - 1];
    let prefix = ".../";
    let available = max_width.saturating_sub(prefix.len() + 3); // 3 for trailing "..."
    if available == 0 {
        return ".....".chars().take(max_width).collect();
    }
    let truncated: String = last.chars().take(available).collect();
    format!("{prefix}{truncated}...")
}

/// Truncate a string to `max_width`, appending `...` if it exceeds.
pub fn truncate_field(value: &str, max_width: usize) -> String {
    if max_width <= 3 {
        return value.chars().take(max_width).collect();
    }
    if value.chars().count() <= max_width {
        return value.to_string();
    }
    let keep = max_width.saturating_sub(3);
    let head: String = value.chars().take(keep).collect();
    format!("{head}...")
}

/// Column sizing hint: fixed-width columns get their natural width,
/// variable-width columns share remaining terminal space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnKind {
    /// Column has a roughly fixed/bounded width (ID, engine, status, duration, etc.)
    Fixed,
    /// Column holds variable-length content (CWD, headline, subject, detail, etc.)
    Variable,
}

/// Print a terminal-width-aware column-aligned table.
///
/// `col_kinds` maps each column index to Fixed or Variable. If not provided,
/// all columns are treated as Fixed (original behavior with natural widths).
pub fn print_table(headers: &[&str], rows: &[Vec<String>]) {
    print_table_with_kinds(headers, rows, &[]);
}

/// Print a table with explicit column kind hints for terminal-aware layout.
pub fn print_table_with_kinds(
    headers: &[&str],
    rows: &[Vec<String>],
    col_kinds: &[ColumnKind],
) {
    if headers.is_empty() {
        return;
    }

    let cols = headers.len();
    let term_width = terminal_width();

    // Compute natural (maximum content) width for each column.
    let mut natural: Vec<usize> = headers.iter().map(|h| h.len()).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate().take(cols) {
            natural[i] = natural[i].max(cell.chars().count());
        }
    }

    // Determine effective kind per column.
    let kinds: Vec<ColumnKind> = (0..cols)
        .map(|i| {
            col_kinds
                .get(i)
                .copied()
                .unwrap_or(ColumnKind::Fixed)
        })
        .collect();

    // Compute allocated widths.
    let separator_space = 2 * (cols.saturating_sub(1)); // 2 spaces between columns
    let fixed_total: usize = kinds
        .iter()
        .enumerate()
        .filter(|(_, k)| **k == ColumnKind::Fixed)
        .map(|(i, _)| natural[i])
        .sum();

    let variable_indices: Vec<usize> = kinds
        .iter()
        .enumerate()
        .filter(|(_, k)| **k == ColumnKind::Variable)
        .map(|(i, _)| i)
        .collect();

    let remaining = term_width
        .saturating_sub(fixed_total)
        .saturating_sub(separator_space);

    let mut widths = natural.clone();

    if !variable_indices.is_empty() && remaining > 0 {
        // Distribute remaining space among variable columns.
        let var_count = variable_indices.len();
        let per_var = remaining / var_count;
        let mut leftover = remaining % var_count;

        for &idx in &variable_indices {
            let alloc = per_var + if leftover > 0 { leftover = leftover.saturating_sub(1); 1 } else { 0 };
            // Don't expand beyond natural width, only cap.
            widths[idx] = natural[idx].min(alloc.max(6)); // minimum 6 chars per variable column
        }
    }

    // Render helper: truncate cells to allocated widths.
    let render_row = |cells: Vec<String>| -> String {
        let mut parts = Vec::new();
        for (i, cell) in cells.into_iter().enumerate().take(cols) {
            let width = widths.get(i).copied().unwrap_or(0);
            let display = if cell.chars().count() > width {
                truncate_field(&cell, width)
            } else {
                cell
            };
            parts.push(format!("{display:<width$}"));
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

        let headers = ["ID", "Engine", "Started", "Duration", "Tokens", "Model", "CWD"];
        let col_kinds = [
            ColumnKind::Fixed,    // ID
            ColumnKind::Fixed,    // Engine
            ColumnKind::Fixed,    // Started
            ColumnKind::Fixed,    // Duration
            ColumnKind::Fixed,    // Tokens
            ColumnKind::Variable, // Model
            ColumnKind::Variable, // CWD
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
                    format_timestamp(&session.started_at),
                    format_duration(session.duration_secs as i64),
                    tokens,
                    session.model.clone(),
                    session.cwd.clone(),
                ]
            })
            .collect();
        print_table_with_kinds(&headers, &rows, &col_kinds);
    }
}
