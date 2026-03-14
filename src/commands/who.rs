use std::collections::HashMap;

use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc};
use clap::Args;
use serde::Serialize;

use crate::db::open_db_readonly;
use crate::db::queries::{query_who, FactType, WhoFilter, WhoResult};
use crate::error::GaalError;
use crate::output::human::{format_timestamp, print_table_with_kinds, ColumnKind};
use crate::output::json::print_json;

/// CLI arguments for `gaal who`.
#[derive(Debug, Clone, Args)]
pub struct WhoArgs {
    /// Inverted query verb: read, wrote, ran, touched, changed, deleted.
    pub verb: String,
    /// Optional query target (file path, command fragment, package, etc.).
    pub target: Option<String>,
    /// Lower time bound (duration like `7d` or absolute timestamp/date).
    #[arg(long, default_value = "7d")]
    pub since: String,
    /// Upper time bound (absolute timestamp/date).
    #[arg(long)]
    pub before: Option<String>,
    /// Restrict to sessions where cwd contains this value.
    #[arg(long)]
    pub cwd: Option<String>,
    /// Restrict to one engine (`claude` or `codex`).
    #[arg(long)]
    pub engine: Option<String>,
    /// Restrict to one session tag.
    #[arg(long)]
    pub tag: Option<String>,
    /// Restrict to failed facts (`exit_code != 0` or `success = false`).
    #[arg(long)]
    pub failed: bool,
    /// Maximum number of matching rows to return.
    #[arg(long, default_value_t = 10)]
    pub limit: i64,
    /// Print human-readable table output instead of JSON.
    #[arg(short = 'H')]
    pub human: bool,
    /// Show full per-fact output including detail fields (default: brief grouped by session).
    #[arg(short = 'F', long)]
    pub full: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
enum MatchMode {
    Subject,
    Detail,
    /// Match against extracted command names from shell pipelines, not the full detail.
    CommandName,
    SubjectOrDetail,
    Installed,
    Deleted,
}

#[derive(Debug, Clone)]
struct VerbSpec {
    fact_types: Vec<FactType>,
    mode: MatchMode,
}

#[derive(Debug, Clone, Serialize)]
struct WhoRow {
    session_id: String,
    engine: String,
    ts: String,
    fact_type: String,
    subject: Option<String>,
    detail: Option<String>,
    session_headline: Option<String>,
}

impl From<WhoResult> for WhoRow {
    fn from(value: WhoResult) -> Self {
        let subject = value
            .subject
            .as_deref()
            .map(normalize_subject)
            .map(str::to_string);
        Self {
            session_id: value.session_id,
            engine: value.engine,
            ts: value.ts,
            fact_type: value.fact_type,
            subject,
            detail: value.detail,
            session_headline: value.session_headline,
        }
    }
}

/// Compact summary row grouped by session (default output).
#[derive(Debug, Clone, Serialize)]
struct WhoSummaryRow {
    session_id: String,
    engine: String,
    latest_ts: String,
    fact_count: usize,
    subjects: Vec<String>,
    headline: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct QueryWindow {
    from: String,
    to: String,
}

/// Wrapper for JSON output metadata.
#[derive(Debug, Clone, Serialize)]
struct WhoOutput<T: Serialize> {
    query_window: QueryWindow,
    shown: usize,
    total: usize,
    sessions: T,
}

/// Execute `gaal who` and print matching facts as JSON or a compact table.
pub fn run(args: WhoArgs) -> Result<(), GaalError> {
    let verb_raw = args.verb.trim();
    if verb_raw.is_empty() {
        print_no_args_help();
        return Ok(());
    }
    let verb = verb_raw.to_ascii_lowercase();
    let spec = verb_spec(&verb)?;
    let note = verb_note(&verb);
    let full = args.full;
    let limit = args.limit.max(1);
    let query_limit = limit.saturating_mul(8);
    let target = args
        .target
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let is_folder_target = target.as_deref().is_some_and(|value| value.ends_with('/'));
    let since = normalize_since(&args.since)?;
    let before = args.before.as_deref().map(normalize_before).transpose()?;
    let query_window = build_query_window(Some(since.as_str()), before.as_deref());

    // Compute human-readable search window for display.
    let since_date = query_window.from.clone();
    let before_date = query_window.to.clone();
    let since_label = format_since_label(&args.since);
    let search_window_hint = format!(
        "Searching {since_label} ({since_date} \u{2192} {before_date}) \u{00b7} Use --since 30d for wider range"
    );

    let conn = open_db_readonly()?;
    let filter = WhoFilter {
        fact_types: spec.fact_types.clone(),
        subject_pattern: target.clone(),
        since: Some(since),
        before,
        cwd: args.cwd,
        engine: args.engine,
        tag: args.tag,
        failed_only: args.failed,
        limit: Some(query_limit),
    };

    let rows = query_who(&conn, &filter)?;
    let filtered: Vec<WhoResult> = rows
        .into_iter()
        .filter(|row| matches_verb(row, spec.mode, target.as_deref(), is_folder_target))
        .collect();
    let total_matches = filtered.len();
    let matches: Vec<WhoRow> = filtered
        .into_iter()
        .take(limit as usize)
        .map(WhoRow::from)
        .collect();
    if matches.is_empty() {
        if args.human {
            eprintln!("No results found.");
        } else {
            let empty_output = serde_json::json!({
                "query_window": query_window,
                "shown": 0,
                "total": 0,
                "sessions": []
            });
            print_json(&empty_output).map_err(GaalError::from)?;
        }
        return Ok(());
    }
    let was_truncated = matches.len() < total_matches;

    if full {
        // --full: per-fact output with full detail (old behavior)
        if args.human {
            eprintln!("{search_window_hint}");
            if !note.is_empty() {
                eprintln!("({note})");
            }
            print_human_full(&matches);
            if was_truncated {
                eprintln!(
                    "Showing {} of {} results \u{2014} use --limit N for more",
                    matches.len(),
                    total_matches
                );
            }
        } else {
            let output = WhoOutput {
                query_window: query_window.clone(),
                shown: matches.len(),
                total: total_matches,
                sessions: &matches,
            };
            print_json(&output).map_err(GaalError::from)?;
        }
        return Ok(());
    }

    // Default: brief output grouped by session
    let summaries = group_by_session(&matches);
    if args.human {
        eprintln!("{search_window_hint}");
        if !note.is_empty() {
            eprintln!("({note})");
        }
        print_human_brief(&summaries);
        if was_truncated {
            eprintln!(
                "Showing {} of {} results \u{2014} use --limit N for more",
                matches.len(),
                total_matches
            );
        }
    } else {
        let output = WhoOutput {
            query_window,
            shown: matches.len(),
            total: total_matches,
            sessions: &summaries,
        };
        print_json(&output).map_err(GaalError::from)?;
    }
    Ok(())
}

/// Return detail-pattern expansions for semantic verbs.
fn expand_verb(verb: &str) -> Vec<&'static str> {
    match verb {
        "installed" => vec![
            "install",
            "add ",
            "brew ",
            "cargo add",
            "pip install",
            "npm install",
            "apt install",
            "go get",
        ],
        "deleted" => vec!["rm ", "rm -", "unlink", "remove", "del "],
        _ => vec![],
    }
}

fn verb_spec(verb: &str) -> Result<VerbSpec, GaalError> {
    let spec = match verb {
        "read" => VerbSpec {
            fact_types: vec![FactType::FileRead],
            mode: MatchMode::Subject,
        },
        "wrote" => VerbSpec {
            fact_types: vec![FactType::FileWrite],
            mode: MatchMode::Subject,
        },
        "ran" => VerbSpec {
            fact_types: vec![FactType::Command],
            mode: MatchMode::CommandName,
        },
        "touched" => VerbSpec {
            fact_types: vec![FactType::FileRead, FactType::FileWrite, FactType::Command],
            mode: MatchMode::SubjectOrDetail,
        },
        "installed" => VerbSpec {
            fact_types: vec![FactType::Command],
            mode: MatchMode::Installed,
        },
        "changed" => VerbSpec {
            fact_types: vec![FactType::FileWrite, FactType::GitOp],
            mode: MatchMode::SubjectOrDetail,
        },
        "deleted" => VerbSpec {
            fact_types: vec![FactType::Command, FactType::FileWrite],
            mode: MatchMode::Deleted,
        },
        _ => {
            return Err(GaalError::ParseError(format!(
                "invalid who verb: {verb} (expected: read|wrote|ran|touched|changed|deleted)"
            )));
        }
    };
    Ok(spec)
}

fn print_no_args_help() {
    println!("Usage: gaal who <verb> [target] [--since <time>] [--before <time>] [--cwd <path>] [--engine <engine>] [--tag <tag>] [--failed] [--limit <n>] [-F] [-H]");
    println!("Available verbs: read, wrote, ran, touched, changed, deleted");
}

/// Return a one-line disclaimer about what the verb covers (and what it misses).
fn verb_note(verb: &str) -> &'static str {
    match verb {
        "wrote" => "Covers Write/Edit tool operations only. Files created via Bash commands may not appear.",
        "read" => "Covers Read tool operations only. Files read via cat/head in Bash may not appear.",
        "ran" => "Matches command program names, not arguments.",
        _ => "",
    }
}

/// Format the --since flag as a human-readable label like "last 7 days".
fn format_since_label(since_raw: &str) -> String {
    let value = since_raw.trim();
    if value.len() < 2 {
        return format!("since {value}");
    }
    let (number, unit) = value.split_at(value.len().saturating_sub(1));
    if let Ok(amount) = number.parse::<i64>() {
        let unit_word = match unit {
            "s" => if amount == 1 { "second" } else { "seconds" },
            "m" => if amount == 1 { "minute" } else { "minutes" },
            "h" => if amount == 1 { "hour" } else { "hours" },
            "d" => if amount == 1 { "day" } else { "days" },
            "w" => if amount == 1 { "week" } else { "weeks" },
            _ => return format!("since {value}"),
        };
        return format!("last {amount} {unit_word}");
    }
    format!("since {value}")
}

fn matches_verb(result: &WhoResult, mode: MatchMode, target: Option<&str>, folder: bool) -> bool {
    match mode {
        MatchMode::Subject => target
            .map(|value| subject_matches(result.subject.as_deref(), value, folder))
            .unwrap_or(true),
        MatchMode::Detail => target
            .map(|value| contains_ci(result.detail.as_deref(), value))
            .unwrap_or(true),
        MatchMode::CommandName => target
            .map(|value| command_name_matches(result.detail.as_deref(), value))
            .unwrap_or(true),
        MatchMode::SubjectOrDetail => target
            .map(|value| {
                subject_matches(result.subject.as_deref(), value, folder)
                    || contains_ci(result.detail.as_deref(), value)
            })
            .unwrap_or(true),
        MatchMode::Installed => semantic_match(result.detail.as_deref(), target, "installed"),
        MatchMode::Deleted => {
            let semantic_deleted = semantic_match(result.detail.as_deref(), target, "deleted");
            let file_write_subject_match = result.fact_type == "file_write"
                && target
                    .map(|value| subject_matches(result.subject.as_deref(), value, folder))
                    .unwrap_or(false);
            semantic_deleted || file_write_subject_match
        }
    }
}

/// Match the target against command names extracted from a shell command string.
///
/// Splits the command on shell separators (`&&`, `||`, `|`, `;`) and matches
/// the target against the first token (program name) of each segment. This
/// avoids false positives from the target appearing as a file path argument
/// in a long command.
fn command_name_matches(detail: Option<&str>, target: &str) -> bool {
    let Some(detail) = detail else {
        return false;
    };
    let target_lower = target.to_ascii_lowercase();
    for name in extract_command_names(detail) {
        let name_lower = name.to_ascii_lowercase();
        if name_lower.contains(&target_lower) {
            return true;
        }
    }
    false
}

/// Extract program names from a shell command string.
///
/// Splits on `&&`, `||`, `|`, `;` then takes the first non-variable-assignment
/// token from each segment. Handles common patterns like:
/// - `cd /path && cargo build --release`  → ["cd", "cargo"]
/// - `FOO=bar baz --flag`                 → ["baz"]
/// - `rg -n pattern | head -5`            → ["rg", "head"]
fn extract_command_names(cmd: &str) -> Vec<&str> {
    let mut names = Vec::new();
    // Split on shell operators. Use a simple char-scanning approach.
    for segment in split_shell_segments(cmd) {
        let trimmed = segment.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Skip leading variable assignments (VAR=value) and env prefixes
        let mut tokens = trimmed.split_whitespace();
        loop {
            let Some(tok) = tokens.next() else {
                break;
            };
            // Skip variable assignments like FOO=bar
            if tok.contains('=') && !tok.starts_with('-') && !tok.starts_with('/') {
                continue;
            }
            // Skip common shell builtins that wrap another command
            if tok == "env" || tok == "sudo" || tok == "nohup" || tok == "time" || tok == "exec" {
                continue;
            }
            // This is the program name — strip any path prefix
            let program = tok.rsplit('/').next().unwrap_or(tok);
            names.push(program);
            break;
        }
    }
    names
}

/// Split a command string on shell separators: `&&`, `||`, `|`, `;`.
fn split_shell_segments(cmd: &str) -> Vec<&str> {
    let mut segments = Vec::new();
    let mut start = 0;
    let bytes = cmd.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    // Track quote state to avoid splitting inside strings
    let mut in_single_quote = false;
    let mut in_double_quote = false;

    while i < len {
        let b = bytes[i];
        if b == b'\'' && !in_double_quote {
            in_single_quote = !in_single_quote;
            i += 1;
            continue;
        }
        if b == b'"' && !in_single_quote {
            in_double_quote = !in_double_quote;
            i += 1;
            continue;
        }
        if in_single_quote || in_double_quote {
            i += 1;
            continue;
        }

        if b == b';' {
            segments.push(&cmd[start..i]);
            start = i + 1;
            i += 1;
        } else if b == b'|' {
            if i + 1 < len && bytes[i + 1] == b'|' {
                // ||
                segments.push(&cmd[start..i]);
                start = i + 2;
                i += 2;
            } else {
                // |
                segments.push(&cmd[start..i]);
                start = i + 1;
                i += 1;
            }
        } else if b == b'&' && i + 1 < len && bytes[i + 1] == b'&' {
            // &&
            segments.push(&cmd[start..i]);
            start = i + 2;
            i += 2;
        } else {
            i += 1;
        }
    }
    if start < len {
        segments.push(&cmd[start..]);
    }
    segments
}

fn semantic_match(detail: Option<&str>, target: Option<&str>, verb: &str) -> bool {
    let has_semantic_verb = expand_verb(verb)
        .iter()
        .any(|pattern| contains_ci(detail, pattern));
    if !has_semantic_verb {
        return false;
    }
    target
        .map(|value| contains_ci(detail, value))
        .unwrap_or(true)
}

fn subject_matches(subject: Option<&str>, target: &str, folder: bool) -> bool {
    let Some(subject) = subject else {
        return false;
    };
    if folder {
        starts_with_ci(subject, target)
    } else {
        contains_ci(Some(subject), target)
    }
}

fn contains_ci(value: Option<&str>, needle: &str) -> bool {
    let Some(value) = value else {
        return false;
    };
    value
        .to_ascii_lowercase()
        .contains(&needle.to_ascii_lowercase())
}

fn starts_with_ci(value: &str, prefix: &str) -> bool {
    value
        .to_ascii_lowercase()
        .starts_with(&prefix.to_ascii_lowercase())
}

fn normalize_subject(subject: &str) -> &str {
    if let Some(marker_idx) = subject.find("*** Update File:") {
        let after_marker = &subject[marker_idx + "*** Update File:".len()..];
        let end_newline = after_marker.find('\n');
        let end_escaped = after_marker.find("\\n");
        let end = match (end_newline, end_escaped) {
            (Some(a), Some(b)) => a.min(b),
            (Some(a), None) => a,
            (None, Some(b)) => b,
            (None, None) => after_marker.len(),
        };
        let path = after_marker[..end].trim().trim_matches('"');
        if !path.is_empty() {
            return path;
        }
    }
    subject
}

/// Group flat fact rows into per-session summaries for brief output.
fn group_by_session(rows: &[WhoRow]) -> Vec<WhoSummaryRow> {
    let mut map: HashMap<String, WhoSummaryRow> = HashMap::new();
    // Track insertion order since HashMap doesn't preserve it.
    let mut order: Vec<String> = Vec::new();

    for row in rows {
        let is_new = !map.contains_key(&row.session_id);
        let entry = map
            .entry(row.session_id.clone())
            .or_insert_with(|| WhoSummaryRow {
                session_id: row.session_id.clone(),
                engine: row.engine.clone(),
                latest_ts: row.ts.clone(),
                fact_count: 0,
                subjects: Vec::new(),
                headline: row.session_headline.clone(),
            });
        if is_new {
            order.push(row.session_id.clone());
        }
        entry.fact_count += 1;
        // Update latest_ts if this fact is newer
        if row.ts > entry.latest_ts {
            entry.latest_ts = row.ts.clone();
        }
        // Collect unique subjects, truncated to filename
        if let Some(ref subj) = row.subject {
            let short = truncate_to_filename(subj);
            if !entry.subjects.contains(&short) {
                entry.subjects.push(short);
            }
        } else if let Some(ref detail) = row.detail {
            let normalized = normalize_subject(detail);
            let short = if normalized != detail {
                normalized.to_string()
            } else {
                // For commands, extract first line / first 80 chars
                detail
                    .lines()
                    .next()
                    .unwrap_or(detail)
                    .chars()
                    .take(80)
                    .collect::<String>()
            };
            if !entry.subjects.contains(&short) {
                entry.subjects.push(short);
            }
        }
        // Cap subjects list to avoid unbounded growth
        if entry.subjects.len() > 5 {
            entry.subjects.truncate(5);
        }
        // Prefer non-None headline
        if entry.headline.is_none() && row.session_headline.is_some() {
            entry.headline = row.session_headline.clone();
        }
    }
    order
        .into_iter()
        .filter_map(|id| map.remove(&id))
        .collect()
}

/// Extract just the filename from a full path.
fn truncate_to_filename(path: &str) -> String {
    path.rsplit('/')
        .next()
        .unwrap_or(path)
        .to_string()
}

fn print_human_full(rows: &[WhoRow]) {
    if rows.is_empty() {
        println!("No matches.");
        return;
    }

    let headers = [
        "Session", "Engine", "When", "Fact", "Subject", "Detail", "Headline",
    ];
    let col_kinds = [
        ColumnKind::Fixed,    // Session
        ColumnKind::Fixed,    // Engine
        ColumnKind::Fixed,    // When
        ColumnKind::Fixed,    // Fact
        ColumnKind::Variable, // Subject
        ColumnKind::Variable, // Detail
        ColumnKind::Variable, // Headline
    ];
    let table_rows: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            vec![
                row.session_id.chars().take(8).collect(),
                row.engine.clone(),
                format_timestamp(&row.ts),
                row.fact_type.clone(),
                row.subject.as_deref().unwrap_or("-").replace('\n', " "),
                row.detail.as_deref().unwrap_or("-").replace('\n', " "),
                row.session_headline.as_deref().unwrap_or("-").replace('\n', " "),
            ]
        })
        .collect();
    print_table_with_kinds(&headers, &table_rows, &col_kinds);
}

fn print_human_brief(rows: &[WhoSummaryRow]) {
    if rows.is_empty() {
        println!("No matches.");
        return;
    }

    let headers = ["Session", "Engine", "When", "Facts", "Subjects", "Headline"];
    let col_kinds = [
        ColumnKind::Fixed,    // Session
        ColumnKind::Fixed,    // Engine
        ColumnKind::Fixed,    // When
        ColumnKind::Fixed,    // Facts
        ColumnKind::Variable, // Subjects
        ColumnKind::Variable, // Headline
    ];
    let table_rows: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            let subjects_display = if row.subjects.is_empty() {
                "-".to_string()
            } else {
                row.subjects.join(", ")
            };
            vec![
                row.session_id.chars().take(8).collect(),
                row.engine.clone(),
                format_timestamp(&row.latest_ts),
                row.fact_count.to_string(),
                subjects_display,
                row.headline.as_deref().unwrap_or("-").replace('\n', " "),
            ]
        })
        .collect();
    print_table_with_kinds(&headers, &table_rows, &col_kinds);
}


fn normalize_since(raw: &str) -> Result<String, GaalError> {
    normalize_bound(raw, BoundKind::Since)
}

fn normalize_before(raw: &str) -> Result<String, GaalError> {
    normalize_bound(raw, BoundKind::Before)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BoundKind {
    Since,
    Before,
}

fn normalize_bound(raw: &str, kind: BoundKind) -> Result<String, GaalError> {
    let value = raw.trim();
    if value.is_empty() {
        return Err(GaalError::ParseError("empty time filter".to_string()));
    }

    if value.eq_ignore_ascii_case("today") {
        let now = Utc::now();
        let ts = match kind {
            BoundKind::Since => {
                let start = now.date_naive().and_hms_opt(0, 0, 0).ok_or_else(|| {
                    GaalError::ParseError("failed to compute 'today'".to_string())
                })?;
                Utc.from_utc_datetime(&start).to_rfc3339()
            }
            BoundKind::Before => now.to_rfc3339(),
        };
        return Ok(ts);
    }

    if let Some(relative) = parse_relative(value) {
        return Ok(relative);
    }

    parse_absolute(value, kind)
}

fn parse_relative(value: &str) -> Option<String> {
    if value.len() < 2 {
        return None;
    }
    let (number, unit) = value.split_at(value.len().saturating_sub(1));
    let amount = number.parse::<i64>().ok()?;
    if amount < 0 {
        return None;
    }

    let duration = match unit {
        "s" => chrono::TimeDelta::try_seconds(amount)?,
        "m" => chrono::TimeDelta::try_minutes(amount)?,
        "h" => chrono::TimeDelta::try_hours(amount)?,
        "d" => chrono::TimeDelta::try_days(amount)?,
        "w" => chrono::TimeDelta::try_weeks(amount)?,
        _ => return None,
    };
    Some((Utc::now() - duration).to_rfc3339())
}

fn parse_absolute(value: &str, kind: BoundKind) -> Result<String, GaalError> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(value) {
        return Ok(dt.with_timezone(&Utc).to_rfc3339());
    }

    if let Ok(date) = NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        let dt = match kind {
            BoundKind::Since => date.and_hms_opt(0, 0, 0),
            BoundKind::Before => date.and_hms_opt(23, 59, 59),
        }
        .ok_or_else(|| GaalError::ParseError(format!("invalid date: {value}")))?;
        return Ok(Utc.from_utc_datetime(&dt).to_rfc3339());
    }

    for fmt in [
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M",
    ] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(value, fmt) {
            return Ok(Utc.from_utc_datetime(&naive).to_rfc3339());
        }
    }

    Err(GaalError::ParseError(format!(
        "invalid time value: {value} (use 7d, 24h, today, YYYY-MM-DD, or RFC3339)"
    )))
}

fn build_query_window(since: Option<&str>, before: Option<&str>) -> QueryWindow {
    let today = Utc::now().format("%Y-%m-%d").to_string();
    let from = since
        .and_then(extract_date)
        .unwrap_or_else(|| today.clone());
    let to = before
        .and_then(extract_date)
        .unwrap_or_else(|| today.clone());
    QueryWindow { from, to }
}

fn extract_date(value: &str) -> Option<String> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(value) {
        return Some(dt.with_timezone(&Utc).format("%Y-%m-%d").to_string());
    }

    if let Ok(date) = NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        return Some(date.format("%Y-%m-%d").to_string());
    }

    for fmt in [
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M",
    ] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(value, fmt) {
            return Some(Utc.from_utc_datetime(&naive).format("%Y-%m-%d").to_string());
        }
    }

    None
}

