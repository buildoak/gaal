use std::collections::HashSet;
use std::path::Path;
use std::thread;
use std::time::Duration;

use chrono::Utc;
use clap::Args;
use rusqlite::{named_params, Connection};
use serde::Serialize;

use crate::commands::active::{
    action_loop_detected, age_from_ts, context_limit_tokens, count_actions_in_window,
    facts_loop_detected, latest_action_from_facts, parse_ts, pct_used, probe_runtime,
    tokens_per_minute_from_samples,
};
use crate::config::load_config;
use crate::db::open_db_readonly;
use crate::db::queries::{get_facts, get_session, list_sessions, ListFilter, SessionRow};
use crate::discovery::active::{find_active_sessions, ActiveSession};
use crate::error::GaalError;
use crate::model::{
    compute_session_status, Fact, FactType, SessionStatus, StatusParams, StuckSignals, IDLE_SECS,
};
use crate::parser::parse_session;
use crate::parser::types::Engine;

const INSPECT_TAIL_LINES: usize = 900;

/// Arguments for `gaal inspect`.
#[derive(Debug, Clone, Args)]
pub struct InspectArgs {
    /// Session ID (or `latest`).
    pub id: Option<String>,
    /// Re-poll every 2s and refresh output.
    #[arg(long)]
    pub watch: bool,
    /// Inspect all currently running sessions.
    #[arg(long)]
    pub active: bool,
    /// Inspect multiple comma-delimited IDs.
    #[arg(long, value_delimiter = ',')]
    pub ids: Vec<String>,
    /// Restrict to one session tag.
    #[arg(long)]
    pub tag: Option<String>,
    /// Human-readable mode (pretty JSON for now).
    #[arg(short = 'H', long = "human")]
    pub human: bool,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum InspectPayload {
    One(Box<InspectOutput>),
    Many(Vec<InspectOutput>),
}

#[derive(Debug, Serialize)]
struct InspectOutput {
    id: String,
    status: SessionStatus,
    pid: Option<u32>,
    engine: Engine,
    model: Option<String>,
    uptime_secs: u64,
    process: Option<InspectProcess>,
    context: InspectContext,
    #[serde(skip_serializing_if = "Option::is_none")]
    current_turn: Option<TurnSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_turn: Option<TurnSnapshot>,
    velocity: Velocity,
    stuck_signals: StuckSignals,
    recent_errors: Vec<RecentError>,
}

#[derive(Debug, Serialize)]
struct InspectProcess {
    cpu_pct: f64,
    rss_mb: f64,
}

#[derive(Debug, Serialize)]
struct InspectContext {
    /// Cumulative input + output tokens across all turns (for cost tracking).
    total_tokens: i64,
    /// Peak input tokens seen in any single API turn (approximates context window usage).
    context_window: i64,
    /// Context window limit for this engine/model.
    context_limit: i64,
    /// Percentage of context window used (context_window / context_limit).
    pct_context: f64,
}

#[derive(Debug, Serialize)]
struct TurnSnapshot {
    number: i32,
    started_at: String,
    elapsed_secs: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_action: Option<InspectAction>,
    actions_this_turn: usize,
}

#[derive(Debug, Serialize)]
struct InspectAction {
    kind: String,
    summary: String,
}

#[derive(Debug, Serialize)]
struct Velocity {
    actions_per_minute_5m: f64,
    tokens_per_minute_5m: f64,
}

#[derive(Debug, Serialize)]
struct RecentError {
    tool: String,
    cmd: String,
    exit_code: i32,
    age_secs: u64,
}

/// Run `gaal inspect`.
pub fn run(args: InspectArgs) -> Result<(), GaalError> {
    loop {
        let payload = collect_payload(&args)?;
        if args.watch {
            print!("\x1B[2J\x1B[H");
        }
        print_json(&payload, args.human)?;

        if !args.watch {
            break;
        }
        thread::sleep(Duration::from_secs(2));
    }

    Ok(())
}

fn collect_payload(args: &InspectArgs) -> Result<InspectPayload, GaalError> {
    let conn = open_db_readonly().ok();
    let active_sessions = find_active_sessions().map_err(GaalError::from)?;
    let tagged_ids = tagged_session_ids(conn.as_ref(), args.tag.as_deref())?;

    if args.active {
        let mut items = Vec::new();
        for active in &active_sessions {
            let item = inspect_live(active, conn.as_ref())?;
            if matches_tag_filter(item.id.as_str(), tagged_ids.as_ref()) {
                items.push(item);
            }
        }
        if items.is_empty() {
            return Err(GaalError::NoResults);
        }
        return Ok(InspectPayload::Many(items));
    }

    if !args.ids.is_empty() {
        let mut items = Vec::new();
        for requested in &args.ids {
            let id = resolve_requested_id(requested, &active_sessions, conn.as_ref())?;
            let item = inspect_one(id.as_str(), &active_sessions, conn.as_ref())?;
            if matches_tag_filter(item.id.as_str(), tagged_ids.as_ref()) {
                items.push(item);
            }
        }
        if items.is_empty() {
            return Err(GaalError::NoResults);
        }
        return Ok(InspectPayload::Many(items));
    }

    let Some(requested) = args.id.as_deref() else {
        return Err(GaalError::ParseError(
            "inspect requires an id unless --active or --ids is provided".to_string(),
        ));
    };

    let resolved = resolve_requested_id(requested, &active_sessions, conn.as_ref())?;
    let item = inspect_one(resolved.as_str(), &active_sessions, conn.as_ref())?;
    if !matches_tag_filter(item.id.as_str(), tagged_ids.as_ref()) {
        return Err(GaalError::NoResults);
    }
    Ok(InspectPayload::One(Box::new(item)))
}

fn matches_tag_filter(id: &str, tagged_ids: Option<&HashSet<String>>) -> bool {
    tagged_ids.map(|ids| ids.contains(id)).unwrap_or(true)
}

fn tagged_session_ids(
    conn: Option<&Connection>,
    tag: Option<&str>,
) -> Result<Option<HashSet<String>>, GaalError> {
    let Some(tag) = tag else {
        return Ok(None);
    };
    let Some(conn) = conn else {
        return Ok(Some(HashSet::new()));
    };

    let mut stmt = conn
        .prepare("SELECT session_id FROM session_tags WHERE tag = :tag")
        .map_err(GaalError::from)?;
    let mut rows = stmt
        .query(named_params! { ":tag": tag })
        .map_err(GaalError::from)?;

    let mut ids = HashSet::new();
    while let Some(row) = rows.next().map_err(GaalError::from)? {
        let id: String = row.get(0).map_err(GaalError::from)?;
        ids.insert(id);
    }
    Ok(Some(ids))
}

fn resolve_requested_id(
    requested: &str,
    active_sessions: &[ActiveSession],
    conn: Option<&Connection>,
) -> Result<String, GaalError> {
    if requested != "latest" {
        return Ok(requested.to_string());
    }

    if let Some(id) = resolve_latest_active_id(active_sessions, conn) {
        return Ok(id);
    }

    if let Some(conn) = conn {
        let filter = ListFilter {
            sort_by: Some("started".to_string()),
            limit: Some(1),
            include_children: true,
            ..ListFilter::default()
        };
        if let Some(row) = list_sessions(conn, &filter)?.into_iter().next() {
            return Ok(row.id);
        }
    }

    Err(GaalError::NoResults)
}

fn resolve_latest_active_id(
    active_sessions: &[ActiveSession],
    conn: Option<&Connection>,
) -> Option<String> {
    let mut best: Option<(String, String)> = None;

    for active in active_sessions {
        let parsed = active
            .jsonl_path
            .as_deref()
            .and_then(|path| parse_session(path).ok());

        let mut id = active
            .id
            .clone()
            .or_else(|| parsed.as_ref().map(|p| p.meta.id.clone()));
        if id.is_none() {
            let runtime = active
                .jsonl_path
                .as_deref()
                .map(|path| probe_runtime(path, active.engine, 100))
                .unwrap_or_default();
            id = runtime.session_id;
        }
        let id = id?;

        let started_at = conn
            .and_then(|c| get_session(c, id.as_str()).ok().flatten())
            .map(|row| row.started_at)
            .or_else(|| parsed.as_ref().map(|p| p.meta.started_at.clone()))
            .unwrap_or_else(|| "1970-01-01T00:00:00Z".to_string());

        match &best {
            Some((_, best_started)) if started_at <= *best_started => {}
            _ => best = Some((id, started_at)),
        }
    }

    best.map(|(id, _)| id)
}

fn inspect_one(
    id: &str,
    active_sessions: &[ActiveSession],
    conn: Option<&Connection>,
) -> Result<InspectOutput, GaalError> {
    if let Some(active) = find_active_by_id(active_sessions, id) {
        return inspect_live(active, conn);
    }

    let Some(conn) = conn else {
        return Err(GaalError::NotFound(id.to_string()));
    };

    let Some(row) = get_session(conn, id)? else {
        return Err(GaalError::NotFound(id.to_string()));
    };

    inspect_archived(&row, conn)
}

fn find_active_by_id<'a>(
    active_sessions: &'a [ActiveSession],
    id: &str,
) -> Option<&'a ActiveSession> {
    active_sessions.iter().find(|session| {
        if session.id.as_deref() == Some(id) {
            return true;
        }
        let Some(path) = session.jsonl_path.as_deref() else {
            return false;
        };
        parse_session(path)
            .ok()
            .map(|parsed| parsed.meta.id == id)
            .unwrap_or(false)
    })
}

fn inspect_live(
    active: &ActiveSession,
    conn: Option<&Connection>,
) -> Result<InspectOutput, GaalError> {
    let stuck_silence_secs = load_config()
        .stuck
        .silence_for_engine(Some(active.engine))
        .max(IDLE_SECS);

    let parsed = active
        .jsonl_path
        .as_deref()
        .and_then(|path| parse_session(path).ok());

    let runtime = active
        .jsonl_path
        .as_deref()
        .map(|path| probe_runtime(path, active.engine, INSPECT_TAIL_LINES))
        .unwrap_or_default();

    let mut id = active
        .id
        .clone()
        .or_else(|| parsed.as_ref().map(|p| p.meta.id.clone()))
        .or_else(|| runtime.session_id.clone())
        .unwrap_or_else(|| format!("pid-{}", active.pid));

    let row = conn.and_then(|c| get_session(c, id.as_str()).ok().flatten());
    if id.starts_with("pid-") {
        if let Some(db_id) = row.as_ref().map(|s| s.id.clone()) {
            id = db_id;
        }
    }

    let facts = session_facts(
        conn,
        id.as_str(),
        parsed.as_ref().map(|p| p.facts.as_slice()),
    );

    let model = row
        .as_ref()
        .and_then(|s| s.model.clone())
        .or_else(|| parsed.as_ref().and_then(|p| p.meta.model.clone()));

    let total_tokens = row
        .as_ref()
        .map(|s| s.total_input_tokens + s.total_output_tokens)
        .or_else(|| {
            parsed
                .as_ref()
                .map(|p| p.total_input_tokens + p.total_output_tokens)
        })
        .unwrap_or(0)
        .max(0);

    let context_window = peak_input_tokens(&runtime.usage_samples);
    let tokens_limit = context_limit_tokens(active.engine, model.as_deref());
    let context_pct = pct_used(context_window, tokens_limit);

    let now = Utc::now();
    let last_event_ts = runtime
        .last_event_ts
        .clone()
        .or_else(|| row.as_ref().and_then(|s| s.last_event_at.clone()))
        .or_else(|| parsed.as_ref().and_then(|p| p.last_event_at.clone()));
    let silence_secs = last_event_ts
        .as_deref()
        .and_then(|ts| age_from_ts(ts, now))
        .unwrap_or(0);

    let loop_detected = if action_loop_detected(&runtime.recent_actions) {
        true
    } else {
        facts_loop_detected(&facts)
    };
    let permission_blocked = runtime.permission_blocked;

    let status = compute_session_status(&StatusParams {
        ended_at: None,
        exit_signal: None,
        pid_alive: true,
        silence_secs,
        loop_detected,
        context_pct,
        permission_blocked,
        stuck_silence_secs,
        executing_command: runtime.executing_command,
        executing_agent: runtime.executing_agent,
        cpu_pct: active.process.cpu_pct,
    });

    let uptime_secs = row
        .as_ref()
        .map(|s| s.started_at.as_str())
        .or_else(|| parsed.as_ref().map(|p| p.meta.started_at.as_str()))
        .and_then(|ts| age_from_ts(ts, now))
        .unwrap_or(0);

    let turn = turn_snapshot(
        &facts,
        runtime.last_action.as_ref().map(|a| InspectAction {
            kind: a.kind.clone(),
            summary: a.summary.clone(),
        }),
        now,
    );

    let velocity = build_velocity(&facts, &runtime, total_tokens, uptime_secs, now, true);
    let recent_errors = build_recent_errors(&facts, now);

    Ok(InspectOutput {
        id,
        status,
        pid: Some(active.pid),
        engine: active.engine,
        model,
        uptime_secs,
        process: Some(InspectProcess {
            cpu_pct: round1(active.process.cpu_pct),
            rss_mb: round1(active.process.rss_mb),
        }),
        context: InspectContext {
            total_tokens,
            context_window,
            context_limit: tokens_limit,
            pct_context: context_pct,
        },
        current_turn: turn,
        last_turn: None,
        velocity,
        stuck_signals: StuckSignals {
            silence_secs,
            loop_detected,
            context_pct,
            permission_blocked,
        },
        recent_errors,
    })
}

fn inspect_archived(row: &SessionRow, conn: &Connection) -> Result<InspectOutput, GaalError> {
    let engine = parse_engine(&row.engine);
    let path = Path::new(&row.jsonl_path);
    let parsed = parse_session(path).ok();
    let runtime = if path.exists() {
        probe_runtime(path, engine, INSPECT_TAIL_LINES)
    } else {
        Default::default()
    };

    let facts = session_facts(
        Some(conn),
        row.id.as_str(),
        parsed.as_ref().map(|p| p.facts.as_slice()),
    );
    let model = row
        .model
        .clone()
        .or_else(|| parsed.as_ref().and_then(|p| p.meta.model.clone()));

    let total_tokens = (row.total_input_tokens + row.total_output_tokens)
        .max(0)
        .max(
            parsed
                .as_ref()
                .map(|p| p.total_input_tokens + p.total_output_tokens)
                .unwrap_or(0),
        );
    let context_window = peak_input_tokens(&runtime.usage_samples);
    let tokens_limit = context_limit_tokens(engine, model.as_deref());
    let context_pct = pct_used(context_window, tokens_limit);

    let now = Utc::now();
    let anchor = row
        .last_event_at
        .as_deref()
        .and_then(parse_ts)
        .or_else(|| row.ended_at.as_deref().and_then(parse_ts))
        .or_else(|| {
            parsed
                .as_ref()
                .and_then(|p| p.last_event_at.as_deref().and_then(parse_ts))
        })
        .unwrap_or(now);

    let uptime_secs = duration_between(
        row.started_at.as_str(),
        row.ended_at
            .as_deref()
            .or(row.last_event_at.as_deref())
            .unwrap_or(row.started_at.as_str()),
    )
    .unwrap_or(0);

    let status = archived_status(row, parsed.as_ref());
    let velocity = build_velocity(&facts, &runtime, total_tokens, uptime_secs, anchor, false);
    let recent_errors = build_recent_errors(&facts, now);

    let turn = turn_snapshot(
        &facts,
        runtime
            .last_action
            .as_ref()
            .map(|a| InspectAction {
                kind: a.kind.clone(),
                summary: a.summary.clone(),
            })
            .or_else(|| {
                latest_action_from_facts(&facts).map(|action| InspectAction {
                    kind: action.kind,
                    summary: action.summary,
                })
            }),
        anchor,
    );

    Ok(InspectOutput {
        id: row.id.clone(),
        status,
        pid: None,
        engine,
        model,
        uptime_secs,
        process: None,
        context: InspectContext {
            total_tokens,
            context_window,
            context_limit: tokens_limit,
            pct_context: context_pct,
        },
        current_turn: None,
        last_turn: turn,
        velocity,
        stuck_signals: StuckSignals {
            silence_secs: 0,
            loop_detected: facts_loop_detected(&facts),
            context_pct,
            permission_blocked: false,
        },
        recent_errors,
    })
}

fn session_facts(
    conn: Option<&Connection>,
    id: &str,
    parsed_fallback: Option<&[Fact]>,
) -> Vec<Fact> {
    if let Some(conn) = conn {
        if let Ok(facts) = get_facts(conn, id, None) {
            return facts;
        }
    }
    parsed_fallback
        .map(|facts| facts.to_vec())
        .unwrap_or_default()
}

fn turn_snapshot(
    facts: &[Fact],
    runtime_last_action: Option<InspectAction>,
    anchor: chrono::DateTime<Utc>,
) -> Option<TurnSnapshot> {
    if facts.is_empty() {
        return None;
    }

    let turn_number = facts
        .iter()
        .filter_map(|fact| fact.turn_number)
        .max()
        .unwrap_or(0);

    let selected = if turn_number > 0 {
        facts
            .iter()
            .filter(|fact| fact.turn_number == Some(turn_number))
            .collect::<Vec<_>>()
    } else {
        facts.iter().collect::<Vec<_>>()
    };

    if selected.is_empty() {
        return None;
    }

    let started_at = selected
        .iter()
        .map(|fact| fact.ts.as_str())
        .find(|ts| !ts.is_empty())
        .unwrap_or("1970-01-01T00:00:00Z")
        .to_string();

    let elapsed_secs = age_from_ts(started_at.as_str(), anchor).unwrap_or(0);
    let actions_this_turn = selected
        .iter()
        .filter(|fact| {
            matches!(
                fact.fact_type,
                FactType::FileRead
                    | FactType::FileWrite
                    | FactType::Command
                    | FactType::TaskSpawn
                    | FactType::GitOp
            )
        })
        .count();

    let last_action = runtime_last_action.or_else(|| {
        selected
            .iter()
            .rev()
            .find_map(|fact| match fact.fact_type {
                FactType::FileRead => Some(("Read", fact.subject.clone(), fact.detail.clone())),
                FactType::FileWrite => Some(("Write", fact.subject.clone(), fact.detail.clone())),
                FactType::Command => Some(("Bash", fact.subject.clone(), fact.detail.clone())),
                FactType::TaskSpawn => Some(("Task", fact.subject.clone(), fact.detail.clone())),
                FactType::GitOp => Some(("Git", fact.subject.clone(), fact.detail.clone())),
                _ => None,
            })
            .map(|(kind, subject, detail)| InspectAction {
                kind: kind.to_string(),
                summary: truncate(
                    detail
                        .or(subject)
                        .unwrap_or_else(|| "action".to_string())
                        .as_str(),
                    120,
                ),
            })
    });

    Some(TurnSnapshot {
        number: turn_number,
        started_at,
        elapsed_secs,
        last_action,
        actions_this_turn,
    })
}

fn build_velocity(
    facts: &[Fact],
    runtime: &crate::commands::active::RuntimeProbe,
    tokens_used: i64,
    uptime_secs: u64,
    anchor: chrono::DateTime<Utc>,
    active: bool,
) -> Velocity {
    let actions_in_5m = count_actions_in_window(facts, anchor, 5);
    let actions_per_minute_5m = round1(actions_in_5m as f64 / 5.0);

    let mut tokens_per_minute_5m =
        tokens_per_minute_from_samples(&runtime.usage_samples, anchor, 5);
    if tokens_per_minute_5m <= 0.0 {
        let minutes = if active {
            (uptime_secs as f64 / 60.0).max(1.0)
        } else {
            let recent_minutes = 5.0;
            if facts.is_empty() {
                (uptime_secs as f64 / 60.0).max(1.0)
            } else {
                recent_minutes
            }
        };
        tokens_per_minute_5m = round1(tokens_used.max(0) as f64 / minutes);
    }

    Velocity {
        actions_per_minute_5m,
        tokens_per_minute_5m,
    }
}

fn build_recent_errors(facts: &[Fact], now: chrono::DateTime<Utc>) -> Vec<RecentError> {
    let mut out = Vec::new();

    for fact in facts.iter().rev() {
        if !matches!(fact.fact_type, FactType::Error | FactType::Command) {
            continue;
        }

        let has_failure = matches!(fact.fact_type, FactType::Error)
            || fact.exit_code.unwrap_or(0) != 0
            || fact.success == Some(false);
        if !has_failure {
            continue;
        }

        let (tool, cmd) = infer_tool_and_cmd(fact);
        let exit_code = fact.exit_code.unwrap_or(1);
        let age_secs = age_from_ts(fact.ts.as_str(), now).unwrap_or(0);

        out.push(RecentError {
            tool,
            cmd,
            exit_code,
            age_secs,
        });

        if out.len() >= 5 {
            break;
        }
    }

    out
}

fn infer_tool_and_cmd(fact: &Fact) -> (String, String) {
    let detail = fact.detail.clone().unwrap_or_default();
    let subject = fact.subject.clone().unwrap_or_default();

    let tool = if matches!(fact.fact_type, FactType::Command) {
        "Bash".to_string()
    } else if subject.to_ascii_lowercase().contains("git") {
        "Git".to_string()
    } else {
        "Tool".to_string()
    };

    let cmd = if !detail.trim().is_empty() {
        truncate(detail.as_str(), 200)
    } else if !subject.trim().is_empty() {
        truncate(subject.as_str(), 200)
    } else {
        "error".to_string()
    };

    (tool, cmd)
}

fn archived_status(
    row: &SessionRow,
    parsed: Option<&crate::parser::types::ParsedSession>,
) -> SessionStatus {
    let stuck_silence_secs = load_config()
        .stuck
        .silence_for_engine(Some(parse_engine(&row.engine)))
        .max(IDLE_SECS);
    let signal = row
        .exit_signal
        .as_deref()
        .or_else(|| parsed.and_then(|p| p.exit_signal.as_deref()))
        .unwrap_or_default();

    compute_session_status(&StatusParams {
        ended_at: row.ended_at.as_deref(),
        exit_signal: Some(signal),
        pid_alive: false,
        silence_secs: 0,
        loop_detected: false,
        context_pct: 0.0,
        permission_blocked: false,
        stuck_silence_secs,
        executing_command: false,
        executing_agent: false,
        cpu_pct: 0.0,
    })
}

fn parse_engine(raw: &str) -> Engine {
    match raw.parse::<Engine>() {
        Ok(engine) => engine,
        Err(_) => {
            if raw.eq_ignore_ascii_case("claude") {
                Engine::Claude
            } else {
                Engine::Codex
            }
        }
    }
}

fn duration_between(start: &str, end: &str) -> Option<u64> {
    let start = parse_ts(start)?;
    let end = parse_ts(end)?;
    if end < start {
        return Some(0);
    }
    u64::try_from(end.signed_duration_since(start).num_seconds()).ok()
}

/// Return the highest input_tokens value from any single usage sample.
/// This approximates peak context window usage — the largest prompt the
/// model had to process in a single turn.
fn peak_input_tokens(samples: &[crate::commands::active::UsageSample]) -> i64 {
    samples.iter().map(|s| s.input_tokens).max().unwrap_or(0)
}

fn round1(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

fn truncate(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    value.chars().take(max_chars).collect()
}

fn print_json<T: Serialize>(value: &T, pretty: bool) -> Result<(), GaalError> {
    let rendered = if pretty {
        serde_json::to_string_pretty(value)
    } else {
        serde_json::to_string(value)
    }
    .map_err(|e| GaalError::Internal(format!("failed to serialize output: {e}")))?;
    println!("{rendered}");
    Ok(())
}
