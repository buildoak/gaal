use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::str::FromStr;

use chrono::{DateTime, NaiveDate, NaiveDateTime, SecondsFormat, Utc};
use clap::{ArgAction, Args, ValueEnum};
use rusqlite::Connection;
use serde::Serialize;

use crate::commands::active::probe_runtime;
use crate::db::open_db_readonly;
use crate::db::queries::{self, count_sessions, ListFilter, SessionRow};
use crate::discovery::active::{find_active_sessions, is_pid_alive, probe_pid};
use crate::error::GaalError;
use crate::model::{compute_session_status, SessionStatus, StatusParams, TokenUsage};
use crate::output::human::{format_duration, format_timestamp, format_tokens, print_table};
use crate::output::{self, HumanReadable, OutputFormat};
use crate::parser::types::Engine;

const STATUS_TAIL_LINES: usize = 700;

/// CLI arguments for `gaal ls`.
#[derive(Debug, Clone, Args)]
pub struct LsArgs {
    /// Filter by computed session status (repeatable).
    #[arg(long, value_enum, action = ArgAction::Append)]
    pub status: Vec<LsStatus>,
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
    #[arg(long, default_value_t = 50)]
    pub limit: i64,
    /// Include child/worker sessions.
    #[arg(long, action = ArgAction::SetTrue)]
    pub children: bool,
    /// Return totals instead of per-session rows.
    #[arg(long, action = ArgAction::SetTrue)]
    pub aggregate: bool,
    /// Render human-readable output.
    #[arg(short = 'H', action = ArgAction::SetTrue)]
    pub human_readable: bool,
}

/// Supported `gaal ls --status` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, ValueEnum)]
#[value(rename_all = "lower")]
pub enum LsStatus {
    Active,
    Idle,
    Completed,
    Failed,
    Interrupted,
    Unknown,
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
    Status,
    Cost,
}

/// JSON summary row for `gaal ls`.
#[derive(Debug, Clone, Serialize)]
pub struct SessionSummary {
    pub id: String,
    pub engine: String,
    pub model: String,
    pub status: String,
    pub cwd: String,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub duration_secs: u64,
    pub parent_id: Option<String>,
    pub child_count: u32,
    pub tokens: TokenUsage,
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
    by_status: HashMap<String, i64>,
}

/// Run `gaal ls`.
pub fn run(args: LsArgs) -> Result<(), GaalError> {
    let conn = open_db_readonly()?;
    let requested_statuses = resolve_requested_statuses(&args);
    let filter = build_filter(&args, &requested_statuses)?;

    if args.aggregate {
        let aggregate = queries::get_aggregate(&conn, &filter)?;
        let mut payload = AggregateJson {
            sessions: aggregate.sessions,
            total_input_tokens: aggregate.total_input_tokens,
            total_output_tokens: aggregate.total_output_tokens,
            estimated_cost_usd: aggregate.estimated_cost_usd,
            by_engine: aggregate.by_engine,
            by_status: aggregate.by_status,
        };

        if requires_precise_aggregate(&args, &requested_statuses) {
            payload = build_precise_aggregate(&conn, &filter, &args.tag, &requested_statuses)?;
        }
        output::json::print_json(&payload).map_err(GaalError::from)?;
        return Ok(());
    }

    let mut rows = queries::list_sessions(&conn, &filter)?;
    rows = filter_rows_by_all_tags(&conn, rows, &args.tag)?;

    let live_pids = load_live_pid_index();
    let now = Utc::now();

    let mut summaries = Vec::with_capacity(rows.len());
    for row in rows {
        let pid = resolve_pid(&live_pids, &row);
        let status = compute_session_status(&status_params_for_row(&row, pid, now));
        if !matches_status_filter(&status, &requested_statuses) {
            continue;
        }
        summaries.push(build_summary(&conn, row, status, now)?);
    }
    if summaries.is_empty() {
        return Err(GaalError::NoResults);
    }

    let shown = summaries.len();
    let total = count_sessions(&conn, &filter)? as usize;

    let format = if args.human_readable {
        OutputFormat::Human
    } else {
        OutputFormat::Json
    };
    output::print_output(&summaries, format).map_err(GaalError::from)?;

    if shown < total {
        if args.human_readable {
            eprintln!(
                "Showing {} of {} sessions \u{2014} use --limit N for more",
                shown, total
            );
        } else {
            let footer = serde_json::json!({
                "shown": shown,
                "total": total,
                "note": format!("Showing {} of {} sessions — use --limit N for more", shown, total)
            });
            eprintln!("{}", footer);
        }
    }

    Ok(())
}

fn requires_precise_aggregate(args: &LsArgs, statuses: &HashSet<LsStatus>) -> bool {
    args.tag.len() > 1
        || statuses.contains(&LsStatus::Idle)
        || statuses.contains(&LsStatus::Unknown)
}

fn build_precise_aggregate(
    conn: &Connection,
    filter: &ListFilter,
    tags: &[String],
    requested_statuses: &HashSet<LsStatus>,
) -> Result<AggregateJson, GaalError> {
    let mut all_filter = filter.clone();
    all_filter.limit = None;

    let mut rows = queries::list_sessions(conn, &all_filter)?;
    rows = filter_rows_by_all_tags(conn, rows, tags)?;

    let live_pids = load_live_pid_index();
    let now = Utc::now();

    let mut by_engine: HashMap<String, i64> = HashMap::new();
    let mut by_status: HashMap<String, i64> = HashMap::new();
    let mut total_input_tokens = 0_i64;
    let mut total_output_tokens = 0_i64;
    let mut sessions = 0_i64;

    for row in rows {
        let pid = resolve_pid(&live_pids, &row);
        let status = compute_session_status(&status_params_for_row(&row, pid, now));
        if !matches_status_filter(&status, requested_statuses) {
            continue;
        }

        sessions += 1;
        total_input_tokens += row.total_input_tokens;
        total_output_tokens += row.total_output_tokens;
        *by_engine.entry(row.engine.clone()).or_insert(0) += 1;
        *by_status.entry(status.to_string()).or_insert(0) += 1;
    }

    Ok(AggregateJson {
        sessions,
        total_input_tokens,
        total_output_tokens,
        estimated_cost_usd: estimate_cost_usd(total_input_tokens, total_output_tokens),
        by_engine,
        by_status,
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
            Self::Status => "status",
            Self::Cost => "cost",
        }
    }
}

#[derive(Default)]
struct LivePidIndex {
    by_id: HashMap<String, u32>,
    by_path: HashMap<String, u32>,
}

fn resolve_requested_statuses(args: &LsArgs) -> HashSet<LsStatus> {
    args.status.iter().copied().collect()
}

fn build_filter(args: &LsArgs, statuses: &HashSet<LsStatus>) -> Result<ListFilter, GaalError> {
    let status = db_status_prefilter(statuses);
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
        status,
        since,
        before,
        cwd: args.cwd.clone(),
        tag,
        sort_by: args.sort.map(|sort| sort.as_str().to_string()),
        limit,
        include_children: args.children,
    })
}

fn db_status_prefilter(statuses: &HashSet<LsStatus>) -> Option<Vec<String>> {
    if statuses.is_empty() {
        return None;
    }

    let mut out: HashSet<String> = HashSet::new();
    for status in statuses {
        match status {
            LsStatus::Completed => {
                out.insert("completed".to_string());
            }
            LsStatus::Failed => {
                out.insert("failed".to_string());
            }
            LsStatus::Active | LsStatus::Idle | LsStatus::Unknown | LsStatus::Interrupted => {
                out.insert("unknown".to_string());
                out.insert("interrupted".to_string());
            }
        }
    }

    let mut vec: Vec<String> = out.into_iter().collect();
    vec.sort_unstable();
    Some(vec)
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

fn load_live_pid_index() -> LivePidIndex {
    let mut index = LivePidIndex::default();
    let sessions = match find_active_sessions() {
        Ok(sessions) => sessions,
        Err(_) => return index,
    };

    for active in sessions {
        if let Some(id) = active.id {
            index.by_id.insert(id, active.pid);
        }
        if let Some(path) = active.jsonl_path {
            index
                .by_path
                .insert(path.to_string_lossy().into_owned(), active.pid);
        }
    }

    index
}

fn resolve_pid(index: &LivePidIndex, row: &SessionRow) -> Option<u32> {
    index
        .by_id
        .get(&row.id)
        .copied()
        .or_else(|| index.by_path.get(&row.jsonl_path).copied())
}

fn status_params_for_row<'a>(
    row: &'a SessionRow,
    pid: Option<u32>,
    now: DateTime<Utc>,
) -> StatusParams<'a> {
    let pid_alive = pid.map(is_pid_alive).unwrap_or(false);
    if row.ended_at.is_some() || !pid_alive {
        return StatusParams {
            ended_at: row.ended_at.as_deref(),
            exit_signal: row.exit_signal.as_deref(),
            pid_alive,
            silence_secs: 0,
            cpu_pct: 0.0,
        };
    }

    let silence_secs = if let Some(engine) = parse_engine(&row.engine) {
        let runtime = probe_runtime(Path::new(&row.jsonl_path), engine, STATUS_TAIL_LINES);
        runtime
            .last_event_ts
            .as_deref()
            .and_then(parse_timestamp)
            .or_else(|| row.last_event_at.as_deref().and_then(parse_timestamp))
            .map(|ts| now.signed_duration_since(ts).num_seconds().max(0) as u64)
            .unwrap_or(0)
    } else {
        row.last_event_at
            .as_deref()
            .and_then(parse_timestamp)
            .map(|ts| now.signed_duration_since(ts).num_seconds().max(0) as u64)
            .unwrap_or(0)
    };

    let cpu_pct = pid.and_then(probe_pid).map(|info| info.cpu_pct).unwrap_or(0.0);

    StatusParams {
        ended_at: row.ended_at.as_deref(),
        exit_signal: row.exit_signal.as_deref(),
        pid_alive,
        silence_secs,
        cpu_pct,
    }
}

fn parse_engine(raw: &str) -> Option<Engine> {
    Engine::from_str(&raw.to_ascii_lowercase()).ok()
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

fn matches_status_filter(status: &SessionStatus, requested: &HashSet<LsStatus>) -> bool {
    if requested.is_empty() {
        return true;
    }

    let comparable = match status {
        SessionStatus::Active => LsStatus::Active,
        SessionStatus::Idle => LsStatus::Idle,
        SessionStatus::Completed => LsStatus::Completed,
        SessionStatus::Failed => LsStatus::Failed,
        SessionStatus::Interrupted => LsStatus::Interrupted,
        SessionStatus::Starting => LsStatus::Active,
        SessionStatus::Unknown => LsStatus::Unknown,
    };
    requested.contains(&comparable)
}

fn build_summary(
    conn: &Connection,
    row: SessionRow,
    status: SessionStatus,
    now: DateTime<Utc>,
) -> Result<SessionSummary, GaalError> {
    let child_count = queries::count_children(conn, &row.id)?;
    let headline = queries::get_handoff(conn, &row.id)?.and_then(|handoff| handoff.headline);

    let duration_secs = compute_duration_secs(&row.started_at, row.ended_at.as_deref(), now);

    Ok(SessionSummary {
        id: row.id,
        engine: row.engine,
        model: row.model.unwrap_or_else(|| "unknown".to_string()),
        status: status.to_string(),
        cwd: row.cwd.unwrap_or_default(),
        started_at: row.started_at,
        ended_at: row.ended_at,
        duration_secs,
        parent_id: row.parent_id,
        child_count: clamp_i32_to_u32(child_count),
        tokens: TokenUsage {
            input: clamp_i64_to_u64(row.total_input_tokens),
            output: clamp_i64_to_u64(row.total_output_tokens),
        },
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

fn clamp_i32_to_u32(value: i32) -> u32 {
    if value <= 0 {
        0
    } else {
        value as u32
    }
}

fn u64_to_i64_saturating(value: u64) -> i64 {
    if value > i64::MAX as u64 {
        i64::MAX
    } else {
        value as i64
    }
}

fn estimate_cost_usd(total_input_tokens: i64, total_output_tokens: i64) -> f64 {
    const INPUT_USD_PER_MTOK: f64 = 3.0;
    const OUTPUT_USD_PER_MTOK: f64 = 15.0;
    let cost = (total_input_tokens as f64 / 1_000_000.0) * INPUT_USD_PER_MTOK
        + (total_output_tokens as f64 / 1_000_000.0) * OUTPUT_USD_PER_MTOK;
    (cost * 100.0).round() / 100.0
}

impl HumanReadable for Vec<SessionSummary> {
    fn print_human(&self) {
        if self.is_empty() {
            println!("No sessions.");
            return;
        }

        let headers = [
            "ID", "Engine", "Status", "Started", "Duration", "Tokens", "Tools", "Children",
            "Model", "CWD",
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
                vec![
                    id,
                    session.engine.clone(),
                    session.status.clone(),
                    format_timestamp(&session.started_at),
                    format_duration(u64_to_i64_saturating(session.duration_secs)),
                    tokens,
                    session.tools_used.to_string(),
                    session.child_count.to_string(),
                    session.model.clone(),
                    session.cwd.clone(),
                ]
            })
            .collect();
        print_table(&headers, &rows);
    }
}
