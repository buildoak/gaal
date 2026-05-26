use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use chrono::{DateTime, NaiveDate, NaiveDateTime, SecondsFormat, Utc};
use serde::Serialize;
use serde_json::Value;

use crate::commands::inspect::resolve_one;
use crate::config::gaal_home;
use crate::db::open_db_readonly;
use crate::db::queries::{list_sessions_overlapping_window, ActivityFilter};
use crate::error::GaalError;
use crate::output::json::print_json;
use crate::render::session_md::{render_session_activity_markdown_with_db, TimeWindow};

#[derive(Debug, Clone)]
pub struct ActivityArgs {
    pub since: String,
    pub before: Option<String>,
    pub engine: Option<String>,
    pub cwd: Option<String>,
    pub session: Option<String>,
    pub skip_subagents: bool,
    pub force: bool,
    pub stdout: bool,
    pub limit: i64,
    pub human: bool,
}

#[derive(Debug, Serialize)]
struct ActivityResult {
    path: String,
    size_bytes: u64,
    estimated_tokens: u64,
    warning: String,
    query_window: QueryWindow,
    sessions_rendered: Vec<ActivitySessionResult>,
    skipped: Vec<ActivitySkipped>,
    degraded: Vec<String>,
}

#[derive(Debug, Serialize)]
struct QueryWindow {
    since: String,
    before: String,
    semantics: &'static str,
}

#[derive(Debug, Serialize)]
struct ActivitySessionResult {
    id: String,
    engine: String,
    source_path: String,
    carried_in: bool,
    continues_after: bool,
    session_latest_event_at: Option<String>,
    warnings: Vec<String>,
    dispatch_ids: Vec<String>,
}

#[derive(Debug, Serialize)]
struct ActivitySkipped {
    id: String,
    engine: String,
    reason: String,
}

pub fn run(args: ActivityArgs) -> Result<(), GaalError> {
    let since = parse_time_bound(&args.since, false)?;
    let before = args
        .before
        .as_deref()
        .map(|raw| parse_time_bound(raw, false))
        .transpose()?
        .unwrap_or_else(|| format_rfc3339(Utc::now()));

    if !timestamp_before(&since, &before) {
        return Err(GaalError::ParseError(
            "`--since` must be earlier than `--before`".to_string(),
        ));
    }

    let window = TimeWindow {
        since: since.clone(),
        before: before.clone(),
    };

    let conn = open_db_readonly()?;
    let candidates = if let Some(raw_id) = args.session.as_deref() {
        vec![resolve_one(&conn, raw_id)?]
    } else {
        list_sessions_overlapping_window(
            &conn,
            &ActivityFilter {
                engine: args.engine.clone(),
                since: since.clone(),
                before: before.clone(),
                cwd: args.cwd.clone(),
                include_subagents: !args.skip_subagents,
                limit: Some(args.limit),
            },
        )?
    };

    let mut rendered = Vec::new();
    let mut rendered_sessions = Vec::new();
    let mut skipped = Vec::new();
    let mut degraded = Vec::new();
    let agent_mux_links = load_agent_mux_links();

    for session in candidates {
        let source = Path::new(&session.jsonl_path);
        if !source.exists() {
            skipped.push(ActivitySkipped {
                id: session.id,
                engine: session.engine,
                reason: format!("source file not found: {}", source.display()),
            });
            continue;
        }

        let before_stat = fs::metadata(source)
            .ok()
            .map(|m| (m.len(), m.modified().ok()));
        let mut activity = match if session.engine == "hermes" {
            crate::render::session_md::render_hermes_session_activity_markdown(
                source,
                &conn,
                &session.id,
                &window,
            )
        } else {
            render_session_activity_markdown_with_db(
                source,
                Some(&conn),
                Some(&session.id),
                &window,
            )
        } {
            Ok(activity) => activity,
            Err(err) => {
                skipped.push(ActivitySkipped {
                    id: session.id,
                    engine: session.engine,
                    reason: format!("render failed: {err}"),
                });
                continue;
            }
        };
        let after_stat = fs::metadata(source)
            .ok()
            .map(|m| (m.len(), m.modified().ok()));
        if before_stat != after_stat {
            activity
                .warnings
                .push("source file changed while rendering".to_string());
            degraded.push(session.id.clone());
        }

        if !activity.has_activity {
            skipped.push(ActivitySkipped {
                id: session.id,
                engine: session.engine,
                reason: "no source-proven events in window".to_string(),
            });
            continue;
        }

        rendered.push(activity.markdown);
        let dispatch_ids = agent_mux_links
            .iter()
            .filter(|link| {
                link.engine == session.engine
                    && normalize_session_id(&session.engine, &link.session_id) == session.id
            })
            .map(|link| link.dispatch_id.clone())
            .collect();
        rendered_sessions.push(ActivitySessionResult {
            id: session.id,
            engine: session.engine,
            source_path: session.jsonl_path,
            carried_in: activity.carried_in,
            continues_after: activity.continues_after,
            session_latest_event_at: activity.session_latest_event_at,
            warnings: activity.warnings,
            dispatch_ids,
        });
    }

    if rendered_sessions.is_empty() {
        return Err(GaalError::NoResults);
    }

    let markdown = render_activity_bundle(&window, &rendered);
    if args.stdout {
        print!("{markdown}");
        return Ok(());
    }

    let path = activity_path(&window, &args);
    let _ = args.force;
    write_markdown_file(&path, &markdown)?;

    let size_bytes = fs::metadata(&path)
        .map(|metadata| metadata.len())
        .map_err(GaalError::Io)?;
    let estimated_tokens = size_bytes / 4;
    let result = ActivityResult {
        path: absolute_display_path(&path)?,
        size_bytes,
        estimated_tokens,
        warning: build_warning(estimated_tokens),
        query_window: QueryWindow {
            since,
            before,
            semantics: "[since,before)",
        },
        sessions_rendered: rendered_sessions,
        skipped,
        degraded,
    };

    if args.human {
        println!("Activity: {}", result.path);
        println!(
            "Window: {} -> {}",
            result.query_window.since, result.query_window.before
        );
        println!("Sessions rendered: {}", result.sessions_rendered.len());
        println!("Skipped: {}", result.skipped.len());
        println!("Size: {} bytes", result.size_bytes);
        println!("Estimated tokens: {}", result.estimated_tokens);
        println!("{}", result.warning);
        return Ok(());
    }

    print_json(&result).map_err(GaalError::from)
}

fn render_activity_bundle(window: &TimeWindow, session_markdowns: &[String]) -> String {
    let mut parts = vec![
        "---".to_string(),
        "render_kind: activity_bundle".to_string(),
        format!("slice_since: {}", window.since),
        format!("slice_before: {}", window.before),
        "window_semantics: \"[since,before)\"".to_string(),
        format!("sessions: {}", session_markdowns.len()),
        "---".to_string(),
        String::new(),
        format!("# Activity: {} -> {}", window.since, window.before),
        String::new(),
    ];

    for (idx, markdown) in session_markdowns.iter().enumerate() {
        parts.push(markdown.trim().to_string());
        if idx + 1 < session_markdowns.len() {
            parts.push(String::new());
            parts.push("---".to_string());
            parts.push(String::new());
        }
    }

    parts.join("\n")
}

fn activity_path(window: &TimeWindow, args: &ActivityArgs) -> PathBuf {
    let mut hasher = DefaultHasher::new();
    window.since.hash(&mut hasher);
    window.before.hash(&mut hasher);
    args.engine.hash(&mut hasher);
    args.cwd.hash(&mut hasher);
    args.session.hash(&mut hasher);
    args.skip_subagents.hash(&mut hasher);
    let hash = hasher.finish();

    let date = window
        .since
        .get(0..10)
        .filter(|date| date.len() == 10)
        .unwrap_or("unknown");
    let (year, month, day) = date_parts(date);
    let safe_since = safe_path_component(&window.since);
    let safe_before = safe_path_component(&window.before);

    gaal_home()
        .join("data")
        .join("activity")
        .join(year)
        .join(month)
        .join(day)
        .join(format!("{safe_since}_{safe_before}_{hash:016x}.md"))
}

fn write_markdown_file(path: &Path, content: &str) -> Result<(), GaalError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(GaalError::Io)?;
    }
    crate::util::atomic_write(path, content).map_err(|err| {
        GaalError::Internal(format!(
            "failed to write activity file at {}: {}",
            path.display(),
            err
        ))
    })
}

fn absolute_display_path(path: &Path) -> Result<String, GaalError> {
    if path.is_absolute() {
        return Ok(path.to_string_lossy().to_string());
    }
    let cwd = std::env::current_dir().map_err(GaalError::Io)?;
    Ok(cwd.join(path).to_string_lossy().to_string())
}

fn build_warning(estimated_tokens: u64) -> String {
    let tokens_k = estimated_tokens / 1_000;
    format!("~{tokens_k}K tokens. Recommend reading via subagent for large windows.")
}

fn date_parts(date: &str) -> (String, String, String) {
    let mut parts = date.split('-');
    (
        parts.next().unwrap_or("unknown").to_string(),
        parts.next().unwrap_or("unknown").to_string(),
        parts.next().unwrap_or("unknown").to_string(),
    )
}

fn safe_path_component(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

fn parse_time_bound(raw: &str, upper_bound: bool) -> Result<String, GaalError> {
    let value = raw.trim();
    if value.is_empty() {
        return Err(GaalError::ParseError("empty time bound".to_string()));
    }

    if let Some(relative_dt) = parse_relative_datetime(value) {
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

fn parse_relative_datetime(raw: &str) -> Option<DateTime<Utc>> {
    let lower = raw.to_ascii_lowercase();
    let now = Utc::now();
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

fn timestamp_before(left: &str, right: &str) -> bool {
    match (
        DateTime::parse_from_rfc3339(left),
        DateTime::parse_from_rfc3339(right),
    ) {
        (Ok(left), Ok(right)) => left < right,
        _ => left < right,
    }
}

#[derive(Debug, Clone)]
struct AgentMuxLink {
    dispatch_id: String,
    engine: String,
    session_id: String,
}

fn load_agent_mux_links() -> Vec<AgentMuxLink> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let root = home.join(".agent-mux").join("dispatches");
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path().join("meta.json");
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        let Some(session_id) = value.get("session_id").and_then(Value::as_str) else {
            continue;
        };
        let engine = value
            .get("engine")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if engine.is_empty() {
            continue;
        }
        let dispatch_id = value
            .get("dispatch_id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                entry
                    .path()
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(str::to_string)
            })
            .unwrap_or_default();
        if dispatch_id.is_empty() {
            continue;
        }
        out.push(AgentMuxLink {
            dispatch_id,
            engine,
            session_id: session_id.to_string(),
        });
    }
    out
}

fn normalize_session_id(engine: &str, raw: &str) -> String {
    match engine {
        "codex" => {
            let hex: String = raw.chars().filter(|c| *c != '-').collect();
            if hex.len() > 8 {
                hex[hex.len() - 8..].to_string()
            } else {
                hex
            }
        }
        "claude" => raw.chars().take(8).collect(),
        _ => raw.to_string(),
    }
}
