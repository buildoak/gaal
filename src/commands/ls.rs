use std::collections::{HashMap, HashSet};
use std::path::Path;

use chrono::{DateTime, NaiveDate, NaiveDateTime, SecondsFormat, Utc};
use clap::{ArgAction, Args, ValueEnum};
use rusqlite::Connection;
use serde::Serialize;

use crate::db::open_db_readonly;
use crate::db::queries::{self, count_sessions, ListFilter, SessionRow};
use crate::error::GaalError;
use crate::model::TokenUsage;
use crate::output::human::{
    format_cwd, format_duration, format_timestamp, format_tokens, print_table_with_kinds,
    ColumnKind,
};
use crate::output::{self, HumanReadable, OutputFormat};

/// CLI arguments for `gaal ls`.
#[derive(Debug, Clone, Args)]
pub struct LsArgs {
    /// Filter by engine name.
    #[arg(long, value_enum)]
    pub engine: Option<LsEngine>,
    /// Lower time bound (`1h`, `3d`, `2w`, `today`, `YYYY-MM-DD`, RFC3339).
    #[arg(long)]
    pub since: Option<String>,
    /// Upper time bound (`YYYY-MM-DD`, `YYYY-MM-DDTHH:MM`, RFC3339).
    #[arg(long)]
    pub before: Option<String>,
    /// Filter by working-directory substring.
    #[arg(long)]
    pub cwd: Option<String>,
    /// Filter by tag; repeat for AND semantics.
    #[arg(long, action = ArgAction::Append)]
    pub tag: Vec<String>,
    /// Sort field.
    #[arg(long, value_enum)]
    pub sort: Option<LsSort>,
    /// Maximum number of rows returned.
    #[arg(long, default_value_t = 10)]
    pub limit: i64,
    /// Return totals instead of per-session rows.
    #[arg(long, action = ArgAction::SetTrue)]
    pub aggregate: bool,
    /// Render human-readable output.
    #[arg(short = 'H', action = ArgAction::SetTrue)]
    pub human_readable: bool,
    /// Show all sessions including noise (0 tool calls and <30s duration).
    #[arg(long, action = ArgAction::SetTrue)]
    pub all: bool,
}

/// Supported `gaal ls --engine` values.
#[derive(Debug, Clone, Copy, ValueEnum)]
#[value(rename_all = "lower")]
pub enum LsEngine {
    Claude,
    Codex,
}

/// Supported `gaal ls --sort` fields.
#[derive(Debug, Clone, Copy, ValueEnum)]
#[value(rename_all = "lower")]
pub enum LsSort {
    Started,
    Ended,
    Tokens,
    Duration,
    Cost,
}

/// JSON summary row for `gaal ls`.
#[derive(Debug, Clone, Serialize)]
pub struct SessionSummary {
    pub id: String,
    pub engine: String,
    pub model: String,
    pub cwd: String,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub duration_secs: u64,
    pub tokens: TokenUsage,
    pub peak_context: u64,
    pub tools_used: u64,
    pub headline: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct AggregateJson {
    sessions: i64,
    total_input_tokens: i64,
    total_output_tokens: i64,
    estimated_cost_usd: f64,
    by_engine: HashMap<String, i64>,
}

/// JSON envelope for `gaal ls` output (non-aggregate mode)
#[derive(Debug, Clone, Serialize)]
struct LsEnvelope {
    query_window: QueryWindow,
    #[serde(skip_serializing_if = "Option::is_none")]
    filter: Option<String>,
    shown: usize,
    total: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    total_unfiltered: Option<usize>,
    sessions: Vec<SessionSummary>,
}

/// Time window for queries
#[derive(Debug, Clone, Serialize)]
struct QueryWindow {
    from: String,
    to: String,
}

/// Run `gaal ls`.
pub fn run(args: LsArgs) -> Result<(), GaalError> {
    let conn = open_db_readonly()?;
    let filter = build_filter(&args)?;

    if args.aggregate {
        let aggregate = queries::get_aggregate(&conn, &filter)?;
        let mut payload = AggregateJson {
            sessions: aggregate.sessions,
            total_input_tokens: aggregate.total_input_tokens,
            total_output_tokens: aggregate.total_output_tokens,
            estimated_cost_usd: aggregate.estimated_cost_usd,
            by_engine: aggregate.by_engine,
        };

        if requires_precise_aggregate(&args) {
            payload = build_precise_aggregate(&conn, &filter, &args.tag)?;
        }
        output::json::print_json(&payload).map_err(GaalError::from)?;
        return Ok(());
    }

    let mut rows = queries::list_sessions(&conn, &filter)?;
    rows = filter_rows_by_all_tags(&conn, rows, &args.tag)?;

    let now = Utc::now();

    let mut summaries = Vec::with_capacity(rows.len());
    for row in rows {
        summaries.push(build_summary(&conn, row, now)?);
    }
    if summaries.is_empty() {
        return Err(GaalError::NoResults);
    }

    // Apply noise filter: hide sessions with 0 tool calls and <30s duration
    let total_unfiltered = count_sessions(&conn, &filter)? as usize;
    let (summaries, is_filtered) = if args.all {
        (summaries, false)
    } else {
        let before_len = summaries.len();
        let filtered: Vec<SessionSummary> = summaries
            .into_iter()
            .filter(|s| !(s.tools_used == 0 && s.duration_secs < 30))
            .collect();
        let did_filter = filtered.len() != before_len || total_unfiltered != filtered.len();
        (filtered, did_filter)
    };

    if summaries.is_empty() {
        return Err(GaalError::NoResults);
    }

    let shown = summaries.len();
    let total = shown; // total reflects filtered count

    if args.human_readable {
        output::print_output(&summaries, OutputFormat::Human).map_err(GaalError::from)?;
        if is_filtered {
            eprintln!(
                "(filtered: hiding sessions with 0 tool calls and <30s duration. Use --all to show everything)"
            );
        }
        if shown < total_unfiltered {
            eprintln!(
                "Showing {} of {} sessions \u{2014} use --limit N for more",
                shown, total_unfiltered
            );
        }
    } else {
        // JSON mode: output envelope with query_window
        let query_window = build_query_window(&conn, &filter)?;
        let envelope = LsEnvelope {
            query_window,
            filter: if is_filtered {
                Some("hiding sessions with 0 tool calls and <30s duration".to_string())
            } else {
                None
            },
            shown,
            total,
            total_unfiltered: if is_filtered {
                Some(total_unfiltered)
            } else {
                None
            },
            sessions: summaries,
        };
        output::json::print_json(&envelope).map_err(GaalError::from)?;

        // Note goes to stderr, not stdout
        if shown < total_unfiltered {
            eprintln!("Showing {} of {} sessions — use --limit N for more", shown, total_unfiltered);
        }
    }

    Ok(())
}

fn requires_precise_aggregate(args: &LsArgs) -> bool {
    args.tag.len() > 1
}

fn build_precise_aggregate(
    conn: &Connection,
    filter: &ListFilter,
    tags: &[String],
) -> Result<AggregateJson, GaalError> {
    let mut all_filter = filter.clone();
    all_filter.limit = Some(i64::MAX);

    let mut rows = queries::list_sessions(conn, &all_filter)?;
    rows = filter_rows_by_all_tags(conn, rows, tags)?;

    let mut by_engine: HashMap<String, i64> = HashMap::new();
    let mut total_input_tokens = 0_i64;
    let mut total_output_tokens = 0_i64;

    for row in &rows {
        total_input_tokens += row.total_input_tokens;
        total_output_tokens += row.total_output_tokens;
        *by_engine.entry(row.engine.clone()).or_insert(0) += 1;
    }

    Ok(AggregateJson {
        sessions: rows.len() as i64,
        total_input_tokens,
        total_output_tokens,
        estimated_cost_usd: estimate_cost_usd_for_rows(&rows),
        by_engine,
    })
}

impl LsEngine {
    fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }
}

impl LsSort {
    fn as_str(self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::Ended => "ended",
            Self::Tokens => "tokens",
            Self::Duration => "duration",
            Self::Cost => "cost",
        }
    }
}

fn build_filter(args: &LsArgs) -> Result<ListFilter, GaalError> {
    let since = args
        .since
        .as_deref()
        .map(|raw| parse_time_bound(raw, false))
        .transpose()?;
    let before = args
        .before
        .as_deref()
        .map(|raw| parse_time_bound(raw, true))
        .transpose()?;
    let limit = Some(args.limit.max(1));
    let tag = args.tag.first().cloned();

    Ok(ListFilter {
        engine: args.engine.map(|engine| engine.as_str().to_string()),
        since,
        before,
        cwd: args.cwd.clone(),
        tag,
        sort_by: args.sort.map(|sort| sort.as_str().to_string()),
        limit,
    })
}

fn parse_time_bound(raw: &str, upper_bound: bool) -> Result<String, GaalError> {
    let value = raw.trim();
    if value.is_empty() {
        return Err(GaalError::ParseError("empty time bound".to_string()));
    }

    if let Some(relative_dt) = parse_relative_datetime(value, upper_bound) {
        return Ok(format_rfc3339(relative_dt));
    }

    if let Ok(dt) = DateTime::parse_from_rfc3339(value) {
        return Ok(format_rfc3339(dt.with_timezone(&Utc)));
    }

    for fmt in ["%Y-%m-%dT%H:%M:%S", "%Y-%m-%dT%H:%M"] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(value, fmt) {
            return Ok(format_rfc3339(DateTime::<Utc>::from_naive_utc_and_offset(
                naive, Utc,
            )));
        }
    }

    if let Ok(date) = NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        let naive = if upper_bound {
            date.and_hms_opt(23, 59, 59)
        } else {
            date.and_hms_opt(0, 0, 0)
        };
        if let Some(ts) = naive {
            return Ok(format_rfc3339(DateTime::<Utc>::from_naive_utc_and_offset(
                ts, Utc,
            )));
        }
    }

    Err(GaalError::ParseError(format!(
        "invalid time bound `{value}` (expected duration like 1h, date, or RFC3339)"
    )))
}

fn parse_relative_datetime(raw: &str, upper_bound: bool) -> Option<DateTime<Utc>> {
    let lower = raw.to_ascii_lowercase();
    let now = Utc::now();

    if lower == "today" {
        let date = now.date_naive();
        let naive = if upper_bound {
            date.and_hms_opt(23, 59, 59)?
        } else {
            date.and_hms_opt(0, 0, 0)?
        };
        return Some(DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc));
    }

    if lower == "yesterday" {
        let date = now.date_naive() - chrono::TimeDelta::try_days(1)?;
        let naive = if upper_bound {
            date.and_hms_opt(23, 59, 59)?
        } else {
            date.and_hms_opt(0, 0, 0)?
        };
        return Some(DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc));
    }

    let split_idx = lower.find(|c: char| !c.is_ascii_digit())?;
    if split_idx == 0 || split_idx >= lower.len() {
        return None;
    }

    let amount = lower[..split_idx].parse::<i64>().ok()?;
    if amount < 0 {
        return None;
    }

    let delta = match &lower[split_idx..] {
        "s" => chrono::TimeDelta::try_seconds(amount)?,
        "m" => chrono::TimeDelta::try_minutes(amount)?,
        "h" => chrono::TimeDelta::try_hours(amount)?,
        "d" => chrono::TimeDelta::try_days(amount)?,
        "w" => chrono::TimeDelta::try_weeks(amount)?,
        _ => return None,
    };
    Some(now - delta)
}

fn format_rfc3339(dt: DateTime<Utc>) -> String {
    dt.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn filter_rows_by_all_tags(
    conn: &Connection,
    rows: Vec<SessionRow>,
    tags: &[String],
) -> Result<Vec<SessionRow>, GaalError> {
    if tags.len() <= 1 {
        return Ok(rows);
    }

    let required: HashSet<&str> = tags.iter().map(String::as_str).collect();
    let mut filtered = Vec::with_capacity(rows.len());

    for row in rows {
        let row_tags = queries::get_tags(conn, &row.id)?;
        let tag_set: HashSet<&str> = row_tags.iter().map(String::as_str).collect();
        if required.iter().all(|tag| tag_set.contains(tag)) {
            filtered.push(row);
        }
    }

    Ok(filtered)
}

fn parse_timestamp(value: &str) -> Option<DateTime<Utc>> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(value) {
        return Some(dt.with_timezone(&Utc));
    }

    for fmt in ["%Y-%m-%dT%H:%M:%S", "%Y-%m-%dT%H:%M"] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(value, fmt) {
            return Some(DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc));
        }
    }

    None
}

fn build_summary(
    conn: &Connection,
    row: SessionRow,
    now: DateTime<Utc>,
) -> Result<SessionSummary, GaalError> {
    let headline = queries::get_handoff(conn, &row.id)?.and_then(|handoff| handoff.headline);

    let duration_secs = compute_duration_secs(&row.started_at, row.ended_at.as_deref(), now);

    Ok(SessionSummary {
        id: row.id,
        engine: row.engine,
        model: row.model.unwrap_or_else(|| "unknown".to_string()),
        cwd: truncate_cwd(&row.cwd.unwrap_or_default()),
        started_at: row.started_at,
        ended_at: row.ended_at,
        duration_secs,
        tokens: TokenUsage {
            input: clamp_i64_to_u64(row.total_input_tokens),
            output: clamp_i64_to_u64(row.total_output_tokens),
        },
        peak_context: clamp_i64_to_u64(row.peak_context),
        tools_used: clamp_i64_to_u64(row.total_tools),
        headline,
    })
}

fn compute_duration_secs(started_at: &str, ended_at: Option<&str>, now: DateTime<Utc>) -> u64 {
    let Some(start) = parse_timestamp(started_at) else {
        return 0;
    };

    let end = ended_at
        .and_then(parse_timestamp)
        .unwrap_or(now)
        .signed_duration_since(start)
        .num_seconds()
        .max(0);

    clamp_i64_to_u64(end)
}

fn clamp_i64_to_u64(value: i64) -> u64 {
    if value <= 0 {
        0
    } else {
        value as u64
    }
}

fn u64_to_i64_saturating(value: u64) -> i64 {
    if value > i64::MAX as u64 {
        i64::MAX
    } else {
        value as i64
    }
}

fn estimate_cost_usd_for_rows(rows: &[SessionRow]) -> f64 {
    let total: f64 = rows.iter().map(|r| queries::estimate_session_cost(r)).sum();
    (total * 100.0).round() / 100.0
}

/// Truncate cwd to show only the last path component (no slashes)
fn truncate_cwd(path: &str) -> String {
    if path.is_empty() {
        return String::new();
    }

    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(path)
        .to_string()
}

/// Build query window based on filter parameters
fn build_query_window(conn: &Connection, filter: &ListFilter) -> Result<QueryWindow, GaalError> {
    let now = Utc::now();

    let from = if let Some(since) = &filter.since {
        since.clone()
    } else {
        // Get earliest session date from DB, default to 10 days ago if none
        match get_earliest_session_date(conn) {
            Ok(Some(earliest)) => earliest,
            _ => format_rfc3339(now - chrono::TimeDelta::try_days(10).unwrap_or_default()),
        }
    };

    let to = filter.before.clone().unwrap_or_else(|| format_rfc3339(now));

    Ok(QueryWindow { from, to })
}

/// Get the earliest session date from the database
fn get_earliest_session_date(conn: &Connection) -> Result<Option<String>, GaalError> {
    let mut stmt = conn.prepare("SELECT started_at FROM sessions ORDER BY started_at ASC LIMIT 1")?;
    let mut rows = stmt.query_map([], |row| {
        Ok(row.get::<_, String>("started_at")?)
    })?;

    if let Some(row) = rows.next() {
        Ok(Some(row?))
    } else {
        Ok(None)
    }
}

impl HumanReadable for Vec<SessionSummary> {
    fn print_human(&self) {
        if self.is_empty() {
            println!("No sessions.");
            return;
        }

        let headers = [
            "ID", "Engine", "Started", "Duration", "Tokens", "Peak", "Tools", "Model",
            "CWD",
        ];
        let col_kinds = [
            ColumnKind::Fixed,    // ID
            ColumnKind::Fixed,    // Engine
            ColumnKind::Fixed,    // Started
            ColumnKind::Fixed,    // Duration
            ColumnKind::Fixed,    // Tokens
            ColumnKind::Fixed,    // Peak
            ColumnKind::Fixed,    // Tools
            ColumnKind::Variable, // Model
            ColumnKind::Variable, // CWD
        ];
        let rows: Vec<Vec<String>> = self
            .iter()
            .map(|session| {
                let id = session.id.chars().take(8).collect::<String>();
                let tokens = format!(
                    "{} / {}",
                    format_tokens(u64_to_i64_saturating(session.tokens.input)),
                    format_tokens(u64_to_i64_saturating(session.tokens.output))
                );
                let peak = if session.peak_context > 0 {
                    format!("{}",
                        format_tokens(u64_to_i64_saturating(session.peak_context))
                    )
                } else {
                    "-".to_string()
                };
                vec![
                    id,
                    session.engine.clone(),
                    format_timestamp(&session.started_at),
                    format_duration(u64_to_i64_saturating(session.duration_secs)),
                    tokens,
                    peak,
                    session.tools_used.to_string(),
                    session.model.clone(),
                    format_cwd(&session.cwd, 40),
                ]
            })
            .collect();
        print_table_with_kinds(&headers, &rows, &col_kinds);
    }
}
