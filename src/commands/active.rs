use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::Path;
use std::thread;
use std::time::Duration;

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use clap::Args;
use rusqlite::Connection;
use serde::Serialize;
use serde_json::Value;

use crate::config::load_config;
use crate::db::open_db_readonly;
use crate::db::queries::{get_facts, get_session, SessionRow};
use crate::discovery::active::{find_active_sessions, ActiveSession};
use crate::error::GaalError;
use crate::model::{compute_session_status, Fact, FactType, SessionStatus, StatusParams};
use crate::output::human::{format_duration, print_table};
use crate::parser::parse_session;
use crate::parser::types::Engine;

#[allow(unused_imports)]
pub(crate) use crate::model::{IDLE_SECS, STUCK_SILENCE_SECS};
const LOOP_WINDOW: usize = 10;
const TAIL_LINES: usize = 700;

/// Arguments for `gaal active`.
#[derive(Debug, Clone, Args)]
pub struct ActiveArgs {
    /// Restrict output to one engine.
    #[arg(long)]
    pub engine: Option<Engine>,
    /// Re-poll every 2s and refresh output.
    #[arg(long)]
    pub watch: bool,
    /// Human-readable table output.
    #[arg(short = 'H', long = "human")]
    pub human: bool,
    /// Flat list (no tree nesting).
    #[arg(long)]
    pub flat: bool,
}

#[derive(Debug, Serialize)]
struct ActiveOutput {
    id: String,
    engine: Engine,
    model: Option<String>,
    pid: u32,
    /// All PIDs associated with this session (after dedup).
    all_pids: Vec<u32>,
    /// Number of OS processes for this session.
    process_count: usize,
    /// PID of the parent session's process, if this is a child worker.
    parent_pid: Option<u32>,
    /// Session IDs of children (populated during tree assembly).
    children: Vec<String>,
    cwd: String,
    uptime_secs: u64,
    cpu_pct: f64,
    rss_mb: f64,
    context_pct: f64,
    status: SessionStatus,
    last_action: Option<String>,
    last_action_age_secs: Option<u64>,
    tmux_session: Option<String>,
    /// One-line summary of what the session is doing.
    summary: Option<String>,
    stuck_signals: ActiveStuckSignals,
}

#[derive(Debug, Serialize)]
struct ActiveStuckSignals {
    silence_secs: u64,
    stuck_silence_secs: u64,
    loop_detected: bool,
    permission_blocked: bool,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct RuntimeProbe {
    pub session_id: Option<String>,
    pub last_event_ts: Option<String>,
    pub last_action: Option<ActionEvent>,
    pub recent_actions: Vec<ActionEvent>,
    pub permission_blocked: bool,
    pub executing_command: bool,
    /// True when the last action is an Agent/Task tool dispatch waiting for subagent completion.
    pub executing_agent: bool,
    pub usage_samples: Vec<UsageSample>,
}

#[derive(Debug, Clone)]
pub(crate) struct UsageSample {
    pub ts: String,
    pub tokens: i64,
    pub input_tokens: i64,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ActionEvent {
    pub ts: Option<String>,
    pub kind: String,
    pub subject: String,
    pub summary: String,
}

impl ActionEvent {
    pub(crate) fn signature_key(&self) -> (String, String) {
        (
            self.kind.to_ascii_lowercase(),
            self.subject.to_ascii_lowercase(),
        )
    }
}

/// Run `gaal active`.
pub fn run(args: ActiveArgs) -> Result<(), GaalError> {
    loop {
        let payload = collect_active(&args)?;
        if payload.is_empty() {
            return Err(GaalError::NoResults);
        }
        if args.watch {
            print!("\x1B[2J\x1B[H");
        }
        if args.human {
            print_active_tree(&payload, args.flat);
        } else {
            print_json(&payload)?;
        }

        if !args.watch {
            break;
        }
        thread::sleep(Duration::from_secs(2));
    }

    Ok(())
}

fn collect_active(args: &ActiveArgs) -> Result<Vec<ActiveOutput>, GaalError> {
    let mut sessions = find_active_sessions().map_err(GaalError::from)?;
    if let Some(engine) = args.engine {
        sessions.retain(|s| s.engine == engine);
    }
    sessions.sort_by_key(|s| s.pid);

    let conn = open_db_readonly().ok();
    let stuck_config = load_config().stuck;
    let mut out = Vec::with_capacity(sessions.len());

    for session in &sessions {
        let stuck_silence_secs = stuck_config
            .silence_for_engine(Some(session.engine))
            .max(IDLE_SECS);
        let mut row = build_active_row(
            session,
            conn.as_ref(),
            stuck_silence_secs,
        )?;
        // I30: Mark pid=0 API sessions as "starting".
        if session.pid == 0 && row.status == SessionStatus::Active {
            row.status = SessionStatus::Starting;
        }
        out.push(row);
    }

    // I28: Dedup by final resolved session ID.
    // The discovery-layer dedup (in find_active_sessions) runs before IDs are
    // fully enriched (parsed JSONL, runtime probe, DB fallback).  Multiple OS
    // processes for the same session may have id=None at that stage and slip
    // through.  Dedup again here with the authoritative IDs.
    out = dedup_active_output(out);

    // Populate children lists for tree view.
    // Build a map from PID → session ID for parent resolution.
    let pid_to_id: HashMap<u32, String> = out
        .iter()
        .flat_map(|o| o.all_pids.iter().map(move |&pid| (pid, o.id.clone())))
        .collect();
    // For each session with a parent_pid, find the parent's ID and add self to its children.
    let child_parents: Vec<(usize, String)> = out
        .iter()
        .enumerate()
        .filter_map(|(idx, o)| {
            o.parent_pid
                .and_then(|ppid| pid_to_id.get(&ppid))
                .map(|parent_id| (idx, parent_id.clone()))
        })
        .collect();
    for (child_idx, parent_id) in &child_parents {
        let child_id = out[*child_idx].id.clone();
        if let Some(parent) = out.iter_mut().find(|o| o.id == *parent_id) {
            parent.children.push(child_id);
        }
    }

    Ok(out)
}

/// I28: Deduplicate `ActiveOutput` rows sharing the same session ID.
/// Keeps the entry with the highest CPU%, merges all_pids.
fn dedup_active_output(rows: Vec<ActiveOutput>) -> Vec<ActiveOutput> {
    let mut by_id: HashMap<String, Vec<ActiveOutput>> = HashMap::new();

    for row in rows {
        by_id.entry(row.id.clone()).or_default().push(row);
    }

    let mut result = Vec::new();
    for (_id, mut group) in by_id {
        if group.len() == 1 {
            result.push(group.remove(0));
            continue;
        }
        // Pick entry with highest CPU as representative.
        group.sort_by(|a, b| {
            b.cpu_pct
                .partial_cmp(&a.cpu_pct)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let mut best = group.remove(0);
        // Merge PIDs and metadata from duplicates.
        for other in &group {
            for &pid in &other.all_pids {
                if !best.all_pids.contains(&pid) {
                    best.all_pids.push(pid);
                }
            }
            if best.parent_pid.is_none() && other.parent_pid.is_some() {
                best.parent_pid = other.parent_pid;
            }
            if best.summary.is_none() && other.summary.is_some() {
                best.summary = other.summary.clone();
            }
            if best.tmux_session.is_none() && other.tmux_session.is_some() {
                best.tmux_session = other.tmux_session.clone();
            }
            if best.model.is_none() && other.model.is_some() {
                best.model = other.model.clone();
            }
        }
        best.process_count = best.all_pids.len().max(1);
        result.push(best);
    }

    // Preserve stable ordering by PID.
    result.sort_by_key(|r| r.pid);
    result
}

fn build_active_row(
    session: &ActiveSession,
    conn: Option<&Connection>,
    stuck_silence_secs: u64,
) -> Result<ActiveOutput, GaalError> {
    let parsed = session
        .jsonl_path
        .as_deref()
        .and_then(|path| parse_session(path).ok());
    let runtime = session
        .jsonl_path
        .as_deref()
        .map(|path| probe_runtime(path, session.engine, TAIL_LINES))
        .unwrap_or_default();

    let mut id = session
        .id
        .clone()
        .or_else(|| parsed.as_ref().map(|p| p.meta.id.clone()))
        .or_else(|| runtime.session_id.clone());
    let db_row = fetch_session_row(conn, id.as_deref())?;
    if id.is_none() {
        id = db_row.as_ref().map(|row| row.id.clone());
    }
    let id = id.unwrap_or_else(|| format!("pid-{}", session.pid));

    let db_facts = fetch_facts(conn, Some(id.as_str()));

    let model = db_row
        .as_ref()
        .and_then(|row| row.model.clone())
        .or_else(|| parsed.as_ref().and_then(|p| p.meta.model.clone()));

    let peak_input = runtime
        .usage_samples
        .iter()
        .map(|sample| sample.input_tokens)
        .max()
        .unwrap_or(0);
    let tokens_limit = context_limit_tokens(session.engine, model.as_deref());
    let context_pct = pct_used(peak_input, tokens_limit);

    let last_event_ts = runtime
        .last_event_ts
        .clone()
        .or_else(|| db_row.as_ref().and_then(|row| row.last_event_at.clone()))
        .or_else(|| parsed.as_ref().and_then(|p| p.last_event_at.clone()));

    let silence_secs = last_event_ts
        .as_deref()
        .and_then(|ts| age_from_ts(ts, Utc::now()))
        .unwrap_or(0);

    let loop_detected = if action_loop_detected(&runtime.recent_actions) {
        true
    } else {
        facts_loop_detected(db_facts.as_deref().unwrap_or(&[]))
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
        cpu_pct: session.process.cpu_pct,
    });

    let last_action_event = runtime.last_action.clone().or_else(|| {
        db_facts
            .as_ref()
            .and_then(|facts| latest_action_from_facts(facts))
    });
    let last_action = last_action_event
        .as_ref()
        .map(|action| format_action(action.kind.as_str(), action.summary.as_str()));
    let last_action_age_secs = last_action_event
        .as_ref()
        .and_then(|action| action.ts.as_deref())
        .and_then(|ts| age_from_ts(ts, Utc::now()));

    let uptime_secs = db_row
        .as_ref()
        .map(|row| row.started_at.as_str())
        .or_else(|| parsed.as_ref().map(|p| p.meta.started_at.as_str()))
        .and_then(|ts| age_from_ts(ts, Utc::now()))
        .unwrap_or(0);

    let process_count = session.all_pids.len().max(1);

    Ok(ActiveOutput {
        id,
        engine: session.engine,
        model,
        pid: session.pid,
        all_pids: session.all_pids.clone(),
        process_count,
        parent_pid: session.parent_pid,
        children: Vec::new(), // populated later during tree assembly
        cwd: session.cwd.clone(),
        uptime_secs,
        cpu_pct: round1(session.process.cpu_pct),
        rss_mb: round1(session.process.rss_mb),
        context_pct,
        status,
        last_action,
        last_action_age_secs,
        tmux_session: session.tmux_session.clone(),
        summary: session.summary.clone(),
        stuck_signals: ActiveStuckSignals {
            silence_secs,
            stuck_silence_secs,
            loop_detected,
            permission_blocked,
        },
    })
}

fn fetch_session_row(
    conn: Option<&Connection>,
    id: Option<&str>,
) -> Result<Option<SessionRow>, GaalError> {
    match (conn, id) {
        (Some(conn), Some(id)) => get_session(conn, id),
        _ => Ok(None),
    }
}

fn fetch_facts(conn: Option<&Connection>, id: Option<&str>) -> Option<Vec<Fact>> {
    let conn = conn?;
    let id = id?;
    get_facts(conn, id, None).ok()
}

pub(crate) fn probe_runtime(path: &Path, engine: Engine, max_lines: usize) -> RuntimeProbe {
    let lines = read_tail_lines(path, max_lines);
    if lines.is_empty() {
        return RuntimeProbe::default();
    }

    let mut session_id: Option<String> = None;
    let mut last_event_ts: Option<String> = None;
    let mut last_action: Option<ActionEvent> = None;
    let mut recent_actions: Vec<ActionEvent> = Vec::new();
    let mut usage_samples: Vec<UsageSample> = Vec::new();

    let mut pending_calls: HashSet<String> = HashSet::new();
    let mut last_tool_use_id: Option<String> = None;
    let mut last_tool_use_kind: Option<String> = None;

    for line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(record) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };

        let ts = record_timestamp(&record);
        if ts.is_some() {
            last_event_ts = ts.clone();
        }
        if session_id.is_none() {
            session_id = extract_session_id(&record, engine);
        }

        if let Some(sample) = extract_usage_sample(&record, engine) {
            usage_samples.push(sample);
        }

        let actions = extract_actions(&record, engine, ts.clone());
        for action in actions {
            if !action.kind.is_empty() {
                last_action = Some(action.clone());
                recent_actions.push(action);
            }
        }

        match engine {
            Engine::Claude => {
                if record.get("type").and_then(Value::as_str) == Some("assistant") {
                    if let Some(blocks) =
                        record.pointer("/message/content").and_then(Value::as_array)
                    {
                        for block in blocks {
                            if block.get("type").and_then(Value::as_str) == Some("tool_use") {
                                if let Some(id) = block.get("id").and_then(Value::as_str) {
                                    pending_calls.insert(id.to_string());
                                    last_tool_use_id = Some(id.to_string());
                                    last_tool_use_kind = block
                                        .get("name")
                                        .and_then(Value::as_str)
                                        .map(str::to_string);
                                }
                            }
                        }
                    }
                }
                if record.get("type").and_then(Value::as_str) == Some("user") {
                    if let Some(blocks) =
                        record.pointer("/message/content").and_then(Value::as_array)
                    {
                        for block in blocks {
                            if block.get("type").and_then(Value::as_str) == Some("tool_result") {
                                if let Some(id) = block.get("tool_use_id").and_then(Value::as_str) {
                                    pending_calls.remove(id);
                                }
                            }
                        }
                    }
                }
            }
            Engine::Codex => {
                if record.get("type").and_then(Value::as_str) == Some("response_item") {
                    let payload_type = record
                        .pointer("/payload/type")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    if matches!(payload_type, "function_call" | "custom_tool_call") {
                        if let Some(call_id) =
                            record.pointer("/payload/call_id").and_then(Value::as_str)
                        {
                            pending_calls.insert(call_id.to_string());
                            last_tool_use_id = Some(call_id.to_string());
                            last_tool_use_kind = record
                                .pointer("/payload/name")
                                .and_then(Value::as_str)
                                .map(str::to_string);
                        }
                    }
                    if matches!(
                        payload_type,
                        "function_call_output" | "custom_tool_call_output"
                    ) {
                        if let Some(call_id) =
                            record.pointer("/payload/call_id").and_then(Value::as_str)
                        {
                            pending_calls.remove(call_id);
                        }
                    }
                }
            }
        }
    }

    if recent_actions.len() > 64 {
        let keep_from = recent_actions.len().saturating_sub(64);
        recent_actions = recent_actions.split_off(keep_from);
    }

    let permission_blocked = if pending_calls.len() == 1 {
        let dominated_by_self = last_tool_use_id
            .as_ref()
            .map(|id| pending_calls.contains(id))
            .unwrap_or(false)
            && last_tool_use_kind
                .as_ref()
                .map(|kind| {
                    let kind_lower = kind.to_ascii_lowercase();
                    kind_lower == "bash" || kind_lower == "exec_command"
                })
                .unwrap_or(false);
        !dominated_by_self
    } else {
        !pending_calls.is_empty()
    };

    let executing_command = last_action
        .as_ref()
        .map(|action| {
            let kind_lower = action.kind.to_ascii_lowercase();
            kind_lower == "bash" || kind_lower == "exec_command"
        })
        .unwrap_or(false)
        && !pending_calls.is_empty();

    let executing_agent = last_action
        .as_ref()
        .map(|action| {
            let kind_lower = action.kind.to_ascii_lowercase();
            kind_lower == "agent" || kind_lower == "task" || kind_lower == "mcp"
        })
        .unwrap_or(false)
        && !pending_calls.is_empty();

    RuntimeProbe {
        session_id,
        last_event_ts,
        last_action,
        recent_actions,
        permission_blocked,
        executing_command,
        executing_agent,
        usage_samples,
    }
}

fn extract_session_id(record: &Value, engine: Engine) -> Option<String> {
    match engine {
        Engine::Claude => record
            .get("sessionId")
            .and_then(Value::as_str)
            .map(str::to_string),
        Engine::Codex => record
            .pointer("/payload/id")
            .or_else(|| record.get("session_id"))
            .and_then(Value::as_str)
            .map(str::to_string),
    }
}

fn record_timestamp(record: &Value) -> Option<String> {
    record
        .get("timestamp")
        .and_then(Value::as_str)
        .or_else(|| record.pointer("/payload/timestamp").and_then(Value::as_str))
        .map(str::to_string)
}

fn extract_usage_sample(record: &Value, engine: Engine) -> Option<UsageSample> {
    let ts = record_timestamp(record)?;
    let (input_tokens, tokens) = match engine {
        Engine::Claude => {
            // I33: Claude API splits tokens into input_tokens (non-cached),
            // cache_read_input_tokens, and cache_creation_input_tokens.
            // Total context usage = sum of all three.
            let input = as_i64(record.pointer("/message/usage/input_tokens"));
            let cache_read = as_i64(record.pointer("/message/usage/cache_read_input_tokens"));
            let cache_creation = as_i64(record.pointer("/message/usage/cache_creation_input_tokens"));
            let total_input = input + cache_read + cache_creation;
            let output = as_i64(record.pointer("/message/usage/output_tokens"));
            (total_input, total_input + output)
        }
        Engine::Codex => {
            let payload_type = record.pointer("/payload/type").and_then(Value::as_str);
            if payload_type == Some("token_count") {
                let input = as_i64(record.pointer("/payload/info/last_token_usage/input_tokens"));
                let output = as_i64(record.pointer("/payload/info/last_token_usage/output_tokens"));
                (input, input + output)
            } else {
                let input = as_i64(record.pointer("/payload/usage/input_tokens"));
                let output = as_i64(record.pointer("/payload/usage/output_tokens"));
                (input, input + output)
            }
        }
    };

    (tokens > 0).then_some(UsageSample {
        ts,
        tokens,
        input_tokens,
    })
}

fn as_i64(value: Option<&Value>) -> i64 {
    value
        .and_then(|v| {
            v.as_i64()
                .or_else(|| v.as_u64().and_then(|n| i64::try_from(n).ok()))
        })
        .unwrap_or(0)
}

fn extract_actions(record: &Value, engine: Engine, ts: Option<String>) -> Vec<ActionEvent> {
    match engine {
        Engine::Claude => extract_claude_actions(record, ts),
        Engine::Codex => extract_codex_actions(record, ts),
    }
}

fn extract_claude_actions(record: &Value, ts: Option<String>) -> Vec<ActionEvent> {
    if record.get("type").and_then(Value::as_str) != Some("assistant") {
        return Vec::new();
    }

    let Some(blocks) = record.pointer("/message/content").and_then(Value::as_array) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for block in blocks {
        if block.get("type").and_then(Value::as_str) != Some("tool_use") {
            continue;
        }

        let kind = block
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("tool")
            .to_string();
        let input = block.get("input").cloned().unwrap_or(Value::Null);
        let subject = extract_action_subject(&input);
        let summary = extract_action_summary(&kind, &input, subject.as_deref());

        out.push(ActionEvent {
            ts: ts.clone(),
            kind,
            subject: subject.unwrap_or_default(),
            summary,
        });
    }

    out
}

fn extract_codex_actions(record: &Value, ts: Option<String>) -> Vec<ActionEvent> {
    if record.get("type").and_then(Value::as_str) != Some("response_item") {
        return Vec::new();
    }

    let payload_type = record
        .pointer("/payload/type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !matches!(payload_type, "function_call" | "custom_tool_call") {
        return Vec::new();
    }

    let kind = record
        .pointer("/payload/name")
        .and_then(Value::as_str)
        .unwrap_or("tool")
        .to_string();

    let input = if payload_type == "function_call" {
        parse_json_or_null(record.pointer("/payload/arguments").and_then(Value::as_str))
    } else {
        parse_json_or_null(record.pointer("/payload/input").and_then(Value::as_str))
    };

    let subject = extract_action_subject(&input);
    let summary = extract_action_summary(&kind, &input, subject.as_deref());

    vec![ActionEvent {
        ts,
        kind,
        subject: subject.unwrap_or_default(),
        summary,
    }]
}

fn parse_json_or_null(raw: Option<&str>) -> Value {
    raw.and_then(|s| serde_json::from_str::<Value>(s).ok())
        .unwrap_or(Value::Null)
}

fn extract_action_subject(input: &Value) -> Option<String> {
    [
        "file_path",
        "path",
        "file",
        "command",
        "cmd",
        "url",
        "query",
        "directory",
        "dir",
        "cwd",
    ]
    .iter()
    .find_map(|key| input.get(*key).and_then(Value::as_str).map(str::to_string))
}

fn extract_action_summary(kind: &str, input: &Value, subject: Option<&str>) -> String {
    if matches!(kind, "Bash" | "exec_command") {
        if let Some(cmd) = input
            .get("command")
            .or_else(|| input.get("cmd"))
            .and_then(Value::as_str)
        {
            return truncate(cmd, 120);
        }
    }

    if let Some(subject) = subject {
        return truncate(subject, 120);
    }

    if input.is_null() {
        return "tool call".to_string();
    }

    let raw = serde_json::to_string(input).unwrap_or_else(|_| "tool call".to_string());
    truncate(raw.as_str(), 120)
}

fn read_tail_lines(path: &Path, max_lines: usize) -> Vec<String> {
    if max_lines == 0 {
        return Vec::new();
    }

    let Ok(mut file) = File::open(path) else {
        return Vec::new();
    };

    let Ok(file_len) = file.metadata().map(|meta| meta.len()) else {
        return Vec::new();
    };
    if file_len == 0 {
        return Vec::new();
    }

    let mut ends_with_newline = false;
    if file.seek(SeekFrom::End(-1)).is_ok() {
        let mut last_byte = [0_u8; 1];
        if file.read_exact(&mut last_byte).is_ok() {
            ends_with_newline = last_byte[0] == b'\n';
        }
    }

    let target_newlines = max_lines.saturating_add(usize::from(ends_with_newline));
    let mut start = 0_u64;
    let mut pos = file_len;
    let mut seen_newlines = 0_usize;
    let mut found = false;
    let mut chunk = vec![0_u8; 64 * 1024];

    while pos > 0 {
        let read_size_u64 = pos.min(chunk.len() as u64);
        let read_size = read_size_u64 as usize;
        let new_pos = pos - read_size_u64;

        if file.seek(SeekFrom::Start(new_pos)).is_err() {
            return Vec::new();
        }
        if file.read_exact(&mut chunk[..read_size]).is_err() {
            return Vec::new();
        }

        for idx in (0..read_size).rev() {
            if chunk[idx] == b'\n' {
                seen_newlines += 1;
                if seen_newlines == target_newlines {
                    let Ok(idx_u64) = u64::try_from(idx) else {
                        return Vec::new();
                    };
                    start = new_pos + idx_u64 + 1;
                    found = true;
                    break;
                }
            }
        }

        if found {
            break;
        }
        pos = new_pos;
    }

    if file.seek(SeekFrom::Start(start)).is_err() {
        return Vec::new();
    }

    let reader = BufReader::new(file);
    let mut lines = Vec::with_capacity(max_lines);
    for line in reader.lines().map_while(Result::ok) {
        if lines.len() == max_lines {
            break;
        }
        lines.push(line);
    }

    lines
}

pub(crate) fn action_loop_detected(actions: &[ActionEvent]) -> bool {
    if actions.len() < LOOP_WINDOW {
        return false;
    }

    let window: Vec<&ActionEvent> = actions.iter().rev().take(LOOP_WINDOW).collect();

    let mut unique = HashSet::new();
    for action in &window {
        unique.insert(action.signature_key());
    }
    if unique.len() > 2 {
        return false;
    }

    let now = Utc::now();
    window.iter().all(|action| {
        action
            .ts
            .as_deref()
            .and_then(|ts| DateTime::parse_from_rfc3339(ts).ok())
            .map(|ts| {
                now.signed_duration_since(ts.with_timezone(&Utc))
                    .num_seconds()
                    <= 300
            })
            .unwrap_or(false)
    })
}

pub(crate) fn facts_loop_detected(facts: &[Fact]) -> bool {
    let actions = facts
        .iter()
        .filter_map(fact_to_action)
        .collect::<Vec<ActionEvent>>();
    action_loop_detected(&actions)
}

pub(crate) fn latest_action_from_facts(facts: &[Fact]) -> Option<ActionEvent> {
    facts.iter().rev().find_map(fact_to_action)
}

fn fact_to_action(fact: &Fact) -> Option<ActionEvent> {
    let kind = match fact.fact_type {
        FactType::FileRead => "Read",
        FactType::FileWrite => "Write",
        FactType::Command => "Bash",
        FactType::TaskSpawn => "Task",
        FactType::GitOp => "Git",
        _ => return None,
    }
    .to_string();

    let subject = fact
        .subject
        .clone()
        .or_else(|| fact.detail.clone())
        .unwrap_or_default();
    let summary = fact
        .detail
        .clone()
        .or_else(|| fact.subject.clone())
        .unwrap_or_else(|| "action".to_string());

    Some(ActionEvent {
        ts: Some(fact.ts.clone()),
        kind,
        subject: truncate(subject.as_str(), 120),
        summary: truncate(summary.as_str(), 120),
    })
}

pub(crate) fn format_action(kind: &str, summary: &str) -> String {
    format!("{kind}: {summary}")
}

pub(crate) fn context_limit_tokens(engine: Engine, model: Option<&str>) -> i64 {
    let model_lower = model.unwrap_or_default().to_ascii_lowercase();
    if model_lower.contains("claude") {
        return 200_000;
    }
    if model_lower.contains("codex") {
        return 128_000;
    }

    match engine {
        Engine::Claude => 200_000,
        Engine::Codex => 128_000,
    }
}

pub(crate) fn pct_used(used_tokens: i64, limit_tokens: i64) -> f64 {
    if limit_tokens <= 0 || used_tokens <= 0 {
        return -1.0; // I33: Unknown — no usage data or invalid limit.
    }
    let pct = (used_tokens as f64 / limit_tokens as f64) * 100.0;
    // I33: Clamp to 0-100 range — values above 100% indicate a calculation error.
    round1(pct.clamp(0.0, 100.0))
}

pub(crate) fn parse_ts(ts: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(ts)
        .map(|dt| dt.with_timezone(&Utc))
        .ok()
}

pub(crate) fn age_from_ts(ts: &str, now: DateTime<Utc>) -> Option<u64> {
    let ts = parse_ts(ts)?;
    if ts > now {
        return Some(0);
    }
    let delta = now.signed_duration_since(ts);
    u64::try_from(delta.num_seconds()).ok()
}

pub(crate) fn count_actions_in_window(
    facts: &[Fact],
    anchor: DateTime<Utc>,
    minutes: i64,
) -> usize {
    let start = anchor - ChronoDuration::minutes(minutes.max(1));
    facts
        .iter()
        .filter(|fact| is_action_fact(&fact.fact_type))
        .filter_map(|fact| parse_ts(&fact.ts))
        .filter(|ts| *ts >= start && *ts <= anchor)
        .count()
}

pub(crate) fn tokens_per_minute_from_samples(
    samples: &[UsageSample],
    anchor: DateTime<Utc>,
    minutes: i64,
) -> f64 {
    let start = anchor - ChronoDuration::minutes(minutes.max(1));
    let total = samples
        .iter()
        .filter_map(|sample| parse_ts(&sample.ts).map(|ts| (ts, sample.tokens)))
        .filter(|(ts, _)| *ts >= start && *ts <= anchor)
        .map(|(_, tokens)| tokens.max(0))
        .sum::<i64>();
    round1(total as f64 / minutes.max(1) as f64)
}

fn is_action_fact(fact_type: &FactType) -> bool {
    matches!(
        fact_type,
        FactType::FileRead
            | FactType::FileWrite
            | FactType::Command
            | FactType::TaskSpawn
            | FactType::GitOp
    )
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

fn print_json<T: Serialize>(value: &T) -> Result<(), GaalError> {
    let rendered = serde_json::to_string(value)
        .map_err(|e| GaalError::Internal(format!("failed to serialize output: {e}")))?;
    println!("{rendered}");
    Ok(())
}

/// I21: Tree view for active sessions with fleet summary.
/// Default: tree with children indented under parents.
/// --flat: original flat list.
fn print_active_tree(sessions: &[ActiveOutput], flat: bool) {
    if sessions.is_empty() {
        println!("No active sessions.");
        return;
    }

    // Fleet summary header.
    let mut active_count = 0usize;
    let mut idle_count = 0usize;
    let mut stuck_count = 0usize;
    let mut starting_count = 0usize;
    for s in sessions {
        match s.status {
            SessionStatus::Active => active_count += 1,
            SessionStatus::Idle => idle_count += 1,
            SessionStatus::Stuck => stuck_count += 1,
            SessionStatus::Starting => starting_count += 1,
            _ => {}
        }
    }
    let mut fleet_parts = Vec::new();
    if active_count > 0 {
        fleet_parts.push(format!("{active_count} active"));
    }
    if idle_count > 0 {
        fleet_parts.push(format!("{idle_count} idle"));
    }
    if stuck_count > 0 {
        fleet_parts.push(format!("{stuck_count} stuck"));
    }
    if starting_count > 0 {
        fleet_parts.push(format!("{starting_count} starting"));
    }
    println!("FLEET: {}", fleet_parts.join(", "));
    println!();

    if flat {
        // Flat mode: original table with summary column.
        print_flat_table(sessions);
        return;
    }

    // Tree mode: top-level first, children indented.
    let child_ids: HashSet<&str> = sessions
        .iter()
        .filter(|s| s.parent_pid.is_some())
        .map(|s| s.id.as_str())
        .collect();

    let top_level: Vec<&ActiveOutput> = sessions
        .iter()
        .filter(|s| !child_ids.contains(s.id.as_str()))
        .collect();

    for parent in &top_level {
        println!("{}", format_session_line(parent, ""));
        // Find children of this parent.
        for child_id in &parent.children {
            if let Some(child) = sessions.iter().find(|s| &s.id == child_id) {
                println!("{}", format_session_line(child, "  "));
            }
        }
    }
}

/// Format a single session as a compact one-line display.
fn format_session_line(s: &ActiveOutput, indent: &str) -> String {
    let id = s.id.chars().take(8).collect::<String>();
    let engine = format!("{:6}", s.engine.to_string());
    let status = format!("{:8}", s.status.to_string());
    let duration = format!("{:>6}", format_duration(s.uptime_secs as i64));
    let ctx = if s.context_pct < 0.0 {
        format!("{:>4} ctx", "-")
    } else {
        format!("{:>4.0}% ctx", s.context_pct)
    };

    let summary_part = s
        .summary
        .as_deref()
        .map(|sum| format!("  \"{}\"", truncate(sum, 50)))
        .unwrap_or_default();

    let pids_part = if s.process_count > 1 {
        format!("  [{} PIDs]", s.process_count)
    } else {
        String::new()
    };

    format!(
        "{indent}{id}  {engine}  {status}  {duration}  {ctx}{summary_part}{pids_part}"
    )
}

/// Flat table view (--flat flag).
fn print_flat_table(sessions: &[ActiveOutput]) {
    let headers = [
        "ID",
        "Engine",
        "Status",
        "Duration",
        "Ctx%",
        "Summary",
        "Last Action",
        "Stuck",
        "CWD",
    ];

    let rows: Vec<Vec<String>> = sessions
        .iter()
        .map(|s| {
            let id = s.id.chars().take(8).collect::<String>();
            let engine = format!("{}", s.engine);
            let status = s.status.to_string();
            let duration = format_duration(s.uptime_secs as i64);
            let ctx = if s.context_pct < 0.0 {
                "-".to_string()
            } else {
                format!("{:.0}%", s.context_pct)
            };

            let summary = s
                .summary
                .as_deref()
                .map(|sum| truncate(sum, 40))
                .unwrap_or_else(|| "-".to_string());

            let last_action = s
                .last_action
                .as_deref()
                .map(|a| truncate(a, 40))
                .unwrap_or_else(|| "-".to_string());

            let stuck = stuck_reason(
                &s.stuck_signals,
                &s.status,
                s.stuck_signals.stuck_silence_secs,
            );

            let cwd = truncate_cwd(&s.cwd, 2);

            vec![id, engine, status, duration, ctx, summary, last_action, stuck, cwd]
        })
        .collect();

    print_table(&headers, &rows);
}

/// Derive a short stuck-reason label from signals and status.
fn stuck_reason(
    signals: &ActiveStuckSignals,
    status: &SessionStatus,
    stuck_silence_secs: u64,
) -> String {
    if !matches!(status, SessionStatus::Stuck) {
        return "-".to_string();
    }
    if signals.permission_blocked {
        return "permission".to_string();
    }
    if signals.loop_detected {
        return "loop".to_string();
    }
    if signals.silence_secs >= stuck_silence_secs {
        return format!("silence ({}s)", signals.silence_secs);
    }
    // context_pct >= 95% is the remaining trigger
    "context".to_string()
}

/// Truncate a path to its last `n` components for readability.
fn truncate_cwd(path: &str, components: usize) -> String {
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    if parts.len() <= components {
        return path.to_string();
    }
    format!(".../{}", parts[parts.len() - components..].join("/"))
}
