use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc};
use clap::Args;
use serde::Serialize;

use crate::db::open_db;
use crate::db::queries::{query_who, FactType, WhoFilter, WhoResult};
use crate::error::GaalError;
use crate::output::human::{format_timestamp, print_table};
use crate::output::json::print_json;

/// CLI arguments for `gaal who`.
#[derive(Debug, Clone, Args)]
pub struct WhoArgs {
    /// Inverted query verb: read, wrote, ran, touched, installed, changed, deleted.
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MatchMode {
    Subject,
    Detail,
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
        Self {
            session_id: value.session_id,
            engine: value.engine,
            ts: value.ts,
            fact_type: value.fact_type,
            subject: value.subject,
            detail: value.detail,
            session_headline: value.session_headline,
        }
    }
}

/// Execute `gaal who` and print matching facts as JSON or a compact table.
pub fn run(args: WhoArgs) -> Result<(), GaalError> {
    let verb = args.verb.to_ascii_lowercase();
    let spec = verb_spec(&verb)?;
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

    let conn = open_db()?;
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
    let matches: Vec<WhoRow> = rows
        .into_iter()
        .filter(|row| matches_verb(row, spec.mode, target.as_deref(), is_folder_target))
        .take(limit as usize)
        .map(WhoRow::from)
        .collect();
    if matches.is_empty() {
        return Err(GaalError::NoResults);
    }

    if args.human {
        print_human(&matches);
        return Ok(());
    }

    print_json(&matches).map_err(GaalError::from)
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
            mode: MatchMode::Detail,
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
                "invalid who verb: {verb} (expected: read|wrote|ran|touched|installed|changed|deleted)"
            )));
        }
    };
    Ok(spec)
}

fn matches_verb(result: &WhoResult, mode: MatchMode, target: Option<&str>, folder: bool) -> bool {
    match mode {
        MatchMode::Subject => target
            .map(|value| subject_matches(result.subject.as_deref(), value, folder))
            .unwrap_or(true),
        MatchMode::Detail => target
            .map(|value| contains_ci(result.detail.as_deref(), value))
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

fn print_human(rows: &[WhoRow]) {
    if rows.is_empty() {
        println!("No matches.");
        return;
    }

    let headers = [
        "Session", "Engine", "When", "Fact", "Subject", "Detail", "Headline",
    ];
    let table_rows: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            vec![
                row.session_id.chars().take(8).collect(),
                row.engine.clone(),
                format_timestamp(&row.ts),
                row.fact_type.clone(),
                compact(row.subject.as_deref(), 42),
                compact(row.detail.as_deref(), 52),
                compact(row.session_headline.as_deref(), 48),
            ]
        })
        .collect();
    print_table(&headers, &table_rows);
}

fn compact(value: Option<&str>, max_len: usize) -> String {
    let text = value.unwrap_or("-").replace('\n', " ");
    if text.chars().count() <= max_len {
        return text;
    }
    let keep = max_len.saturating_sub(3);
    let head: String = text.chars().take(keep).collect();
    format!("{head}...")
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
