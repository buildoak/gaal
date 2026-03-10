use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use clap::{Args, ValueEnum};
use rusqlite::{named_params, Connection};
use serde::Serialize;
use serde_json::Value;

use crate::commands::active::{
    action_loop_detected, context_limit_tokens, pct_used, probe_runtime,
};
use crate::config::{load_config, StuckConfig};
use crate::db::open_db_readonly;
use crate::db::queries::{
    count_children, get_children, get_facts, get_handoff, get_session, get_tags, SessionRow,
};
use crate::discovery::active::{find_active_sessions, is_pid_alive};
use crate::error::GaalError;
use crate::model::{
    compute_session_status, CommandEntry, ErrorEntry, Fact, FactType, FileOps, GitOp,
    SessionRecord, SessionStatus, StatusParams, TokenUsage, IDLE_SECS,
};
use crate::output::json::print_json;
use crate::parser::types::Engine;

const STATUS_TAIL_LINES: usize = 700;

/// File-operation output mode for `gaal show --files`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum FilesMode {
    /// Include file reads only.
    Read,
    /// Include file writes/edits only.
    Write,
    /// Include both reads and writes/edits.
    All,
}

/// CLI arguments for `gaal show`.
#[derive(Debug, Clone, Args)]
pub struct ShowArgs {
    /// Session ID or ID prefix. Use `latest` to resolve the newest session.
    pub id: Option<String>,

    /// Include file operations (`read`, `write`, or `all`).
    #[arg(long, value_enum, num_args = 0..=1, default_missing_value = "all")]
    pub files: Option<FilesMode>,

    /// Include errors and non-zero exits.
    #[arg(long)]
    pub errors: bool,

    /// Include command execution entries.
    #[arg(long)]
    pub commands: bool,

    /// Include git operations.
    #[arg(long)]
    pub git: bool,

    /// Include token usage breakdown.
    #[arg(long)]
    pub tokens: bool,

    /// Include recursive child-session tree.
    #[arg(long)]
    pub tree: bool,

    /// Include direct child-session summaries inline.
    #[arg(long)]
    pub children: bool,

    /// Include full fact timeline.
    #[arg(long)]
    pub trace: bool,

    /// Include source JSONL path.
    #[arg(long)]
    pub source: bool,

    /// Render as session markdown (full conversation flow).
    #[arg(long)]
    pub markdown: bool,

    /// Batch mode session IDs (comma-separated prefixes).
    #[arg(long, conflicts_with = "tag", conflicts_with = "id")]
    pub ids: Option<String>,

    /// Batch mode tag filter.
    #[arg(long, conflicts_with = "ids", conflicts_with = "id")]
    pub tag: Option<String>,

    /// Render human-readable output.
    #[arg(short = 'H', long = "human")]
    pub human: bool,
}

#[derive(Debug, Clone, Serialize)]
struct TreeNode {
    id: String,
    intent: String,
    status: String,
    duration_secs: u64,
    children: Vec<TreeNode>,
}

#[derive(Debug, Clone, Serialize)]
struct ChildSummary {
    id: String,
    status: String,
    started_at: String,
    duration_secs: u64,
    headline: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct TokenBreakdown {
    input_total: u64,
    output_total: u64,
    turns: u32,
    avg_input_per_turn: u64,
    avg_output_per_turn: u64,
}

#[derive(Debug, Clone)]
struct ShowData {
    record: SessionRecord,
    trace: Option<Vec<Fact>>,
    tree: Option<TreeNode>,
    child_sessions: Option<Vec<ChildSummary>>,
    token_breakdown: Option<TokenBreakdown>,
}

#[derive(Default)]
struct LivePidIndex {
    by_id: HashMap<String, u32>,
    by_path: HashMap<String, u32>,
}

/// Execute the `gaal show` command.
pub fn run(args: ShowArgs) -> Result<(), GaalError> {
    let conn = open_db_readonly()?;
    let session_rows = resolve_sessions(&conn, &args)?;

    // Handle --markdown: render JSONL to markdown directly.
    if args.markdown {
        let row = session_rows
            .into_iter()
            .next()
            .ok_or(GaalError::NoResults)?;
        let jsonl_path = std::path::Path::new(&row.jsonl_path);
        if !jsonl_path.exists() {
            return Err(GaalError::NotFound(format!(
                "JSONL source file not found: {}",
                row.jsonl_path
            )));
        }
        let markdown = crate::render::session_md::render_session_markdown(jsonl_path)
            .map_err(|e| GaalError::Internal(format!("failed to render session markdown: {e}")))?;
        print!("{markdown}");
        return Ok(());
    }

    let live_pids = load_live_pid_index();
    let now = Utc::now();
    let stuck_config = load_config().stuck;

    let mut out = Vec::with_capacity(session_rows.len());
    for row in session_rows {
        out.push(build_show_data(
            &conn,
            &row,
            &args,
            &live_pids,
            &stuck_config,
            now,
        )?);
    }

    if args.human {
        print_human(&out, &args);
        return Ok(());
    }

    if args.ids.is_some() || args.tag.is_some() {
        let payload = out
            .into_iter()
            .map(|data| to_json_value(data, &args))
            .collect::<Result<Vec<_>, _>>()?;
        print_json(&payload).map_err(GaalError::from)?;
        return Ok(());
    }

    let data = out.into_iter().next().ok_or(GaalError::NoResults)?;

    if args.tree {
        let payload = serde_json::to_value(data.tree.ok_or_else(|| {
            GaalError::Internal("missing tree data for --tree output".to_string())
        })?)
        .map_err(|e| GaalError::Internal(format!("failed to serialize tree: {e}")))?;
        print_json(&payload).map_err(GaalError::from)?;
        return Ok(());
    }

    let payload = to_json_value(data, &args)?;
    print_json(&payload).map_err(GaalError::from)?;
    Ok(())
}

fn resolve_sessions(conn: &Connection, args: &ShowArgs) -> Result<Vec<SessionRow>, GaalError> {
    if let Some(raw_ids) = &args.ids {
        let ids = split_csv(raw_ids);
        if ids.is_empty() {
            return Err(GaalError::ParseError("--ids must not be empty".to_string()));
        }
        let mut rows = Vec::with_capacity(ids.len());
        for id in ids {
            rows.push(resolve_one(conn, &id)?);
        }
        return Ok(rows);
    }

    if let Some(tag) = &args.tag {
        let ids = find_session_ids_by_tag(conn, tag)?;
        if ids.is_empty() {
            return Err(GaalError::NoResults);
        }
        let mut rows = Vec::with_capacity(ids.len());
        for id in ids {
            rows.push(load_session_by_exact_id(conn, &id)?);
        }
        return Ok(rows);
    }

    let requested = args.id.clone().unwrap_or_else(|| "latest".to_string());
    Ok(vec![resolve_one(conn, &requested)?])
}

fn resolve_one(conn: &Connection, raw_id: &str) -> Result<SessionRow, GaalError> {
    if raw_id == "latest" {
        let latest_id = find_latest_session_id(conn)?;
        return load_session_by_exact_id(conn, &latest_id);
    }

    let matches = find_session_ids_by_prefix(conn, raw_id)?;
    match matches.len() {
        0 => Err(GaalError::NotFound(raw_id.to_string())),
        1 => load_session_by_exact_id(conn, &matches[0]),
        _ => Err(GaalError::AmbiguousId(raw_id.to_string())),
    }
}

fn find_latest_session_id(conn: &Connection) -> Result<String, GaalError> {
    conn.query_row(
        "SELECT id FROM sessions ORDER BY started_at DESC LIMIT 1",
        [],
        |row| row.get::<_, String>(0),
    )
    .map_err(|err| match err {
        rusqlite::Error::QueryReturnedNoRows => GaalError::NotFound("latest".to_string()),
        other => GaalError::Db(other),
    })
}

fn find_session_ids_by_prefix(conn: &Connection, prefix: &str) -> Result<Vec<String>, GaalError> {
    let like = format!("{prefix}%");
    let mut stmt = conn
        .prepare("SELECT id FROM sessions WHERE id LIKE :prefix ORDER BY started_at DESC")
        .map_err(GaalError::from)?;
    let mut rows = stmt
        .query(named_params! {":prefix": like})
        .map_err(GaalError::from)?;

    let mut out = Vec::new();
    while let Some(row) = rows.next().map_err(GaalError::from)? {
        out.push(row.get::<_, String>(0).map_err(GaalError::from)?);
    }
    Ok(out)
}

fn find_session_ids_by_tag(conn: &Connection, tag: &str) -> Result<Vec<String>, GaalError> {
    let mut stmt = conn
        .prepare(
            r#"
            SELECT s.id
            FROM sessions s
            INNER JOIN session_tags t ON t.session_id = s.id
            WHERE t.tag = :tag
            ORDER BY s.started_at DESC
            "#,
        )
        .map_err(GaalError::from)?;

    let mut rows = stmt
        .query(named_params! { ":tag": tag })
        .map_err(GaalError::from)?;

    let mut out = Vec::new();
    while let Some(row) = rows.next().map_err(GaalError::from)? {
        out.push(row.get::<_, String>(0).map_err(GaalError::from)?);
    }
    Ok(out)
}

fn load_session_by_exact_id(conn: &Connection, id: &str) -> Result<SessionRow, GaalError> {
    get_session(conn, id)?.ok_or_else(|| GaalError::NotFound(id.to_string()))
}

fn build_show_data(
    conn: &Connection,
    row: &SessionRow,
    args: &ShowArgs,
    live_pids: &LivePidIndex,
    stuck_config: &StuckConfig,
    now: DateTime<Utc>,
) -> Result<ShowData, GaalError> {
    let any_fact_filter = args.files.is_some() || args.commands || args.errors || args.git;
    let include_files = args.files.is_some() || !any_fact_filter;
    let include_commands = args.commands || !any_fact_filter;
    let include_errors = args.errors || !any_fact_filter;
    let include_git = args.git || !any_fact_filter;
    let include_trace = args.trace;

    let facts = get_facts(conn, &row.id, None)?;

    let handoff = get_handoff(conn, &row.id)?;
    let child_rows = get_children(conn, &row.id)?;
    let tags = get_tags(conn, &row.id)?;

    let files = if include_files {
        collect_files(&facts, args.files.unwrap_or(FilesMode::All))
    } else {
        FileOps {
            read: Vec::new(),
            written: Vec::new(),
            edited: Vec::new(),
        }
    };

    let commands = if include_commands {
        collect_commands(&facts)
    } else {
        Vec::new()
    };

    let errors = if include_errors {
        collect_errors(&facts)
    } else {
        Vec::new()
    };

    let git_ops = if include_git {
        collect_git_ops(&facts)
    } else {
        Vec::new()
    };

    let child_count = count_children(conn, &row.id)?;
    let child_ids = child_rows
        .iter()
        .map(|child| child.id.clone())
        .collect::<Vec<_>>();

    let record = SessionRecord {
        id: row.id.clone(),
        engine: row.engine.clone(),
        model: row.model.clone().unwrap_or_else(|| "unknown".to_string()),
        status: status_from_row(row, live_pids, stuck_config, now).to_string(),
        cwd: row.cwd.clone().unwrap_or_default(),
        started_at: row.started_at.clone(),
        ended_at: row.ended_at.clone(),
        duration_secs: duration_secs(row),
        parent_id: row.parent_id.clone(),
        child_count: child_count.max(0) as u32,
        children: child_ids,
        tokens: TokenUsage {
            input: row.total_input_tokens.max(0) as u64,
            output: row.total_output_tokens.max(0) as u64,
        },
        tools_used: row.total_tools.max(0) as u32,
        turns: row.total_turns.max(0) as u32,
        headline: handoff.as_ref().and_then(|h| h.headline.clone()),
        files,
        commands,
        errors,
        git_ops,
        jsonl_path: row.jsonl_path.clone(),
        last_event_at: row
            .last_event_at
            .clone()
            .unwrap_or_else(|| row.started_at.clone()),
        exit_signal: row.exit_signal.clone(),
        tags,
    };

    let trace = if include_trace {
        Some(facts.clone())
    } else {
        None
    };
    let tree = if args.tree {
        Some(build_tree(conn, row, live_pids, stuck_config, now)?)
    } else {
        None
    };
    let child_sessions = if args.children {
        Some(build_child_summaries(
            conn,
            &child_rows,
            live_pids,
            stuck_config,
            now,
        )?)
    } else {
        None
    };
    let token_breakdown = if args.tokens {
        Some(TokenBreakdown {
            input_total: record.tokens.input,
            output_total: record.tokens.output,
            turns: record.turns,
            avg_input_per_turn: avg_tokens(record.tokens.input, record.turns),
            avg_output_per_turn: avg_tokens(record.tokens.output, record.turns),
        })
    } else {
        None
    };

    Ok(ShowData {
        record,
        trace,
        tree,
        child_sessions,
        token_breakdown,
    })
}

fn to_json_value(data: ShowData, args: &ShowArgs) -> Result<Value, GaalError> {
    let mut map = match serde_json::to_value(data.record)
        .map_err(|e| GaalError::Internal(format!("failed to serialize session record: {e}")))?
    {
        Value::Object(map) => map,
        _ => {
            return Err(GaalError::Internal(
                "expected session record to serialize to object".to_string(),
            ))
        }
    };

    let any_fact_filter = args.files.is_some() || args.errors || args.commands || args.git;
    if any_fact_filter {
        if args.files.is_none() {
            map.remove("files");
        }
        if !args.commands {
            map.remove("commands");
        }
        if !args.errors {
            map.remove("errors");
        }
        if !args.git {
            map.remove("git_ops");
        }
    }

    if !args.source {
        map.remove("jsonl_path");
    }

    if let Some(trace) = data.trace {
        map.insert(
            "trace".to_string(),
            serde_json::to_value(trace)
                .map_err(|e| GaalError::Internal(format!("failed to serialize trace: {e}")))?,
        );
    }

    if let Some(tree) = data.tree {
        map.insert(
            "tree".to_string(),
            serde_json::to_value(tree)
                .map_err(|e| GaalError::Internal(format!("failed to serialize tree: {e}")))?,
        );
    }

    if let Some(children) = data.child_sessions {
        map.insert(
            "child_sessions".to_string(),
            serde_json::to_value(children).map_err(|e| {
                GaalError::Internal(format!("failed to serialize child sessions: {e}"))
            })?,
        );
    }

    if let Some(tokens) = data.token_breakdown {
        map.insert(
            "token_breakdown".to_string(),
            serde_json::to_value(tokens).map_err(|e| {
                GaalError::Internal(format!("failed to serialize token breakdown: {e}"))
            })?,
        );
    }

    Ok(Value::Object(map))
}

fn build_tree(
    conn: &Connection,
    row: &SessionRow,
    live_pids: &LivePidIndex,
    stuck_config: &StuckConfig,
    now: DateTime<Utc>,
) -> Result<TreeNode, GaalError> {
    let handoff = get_handoff(conn, &row.id)?;
    let children = get_children(conn, &row.id)?;
    let mut tree_children = Vec::with_capacity(children.len());

    for child in &children {
        tree_children.push(build_tree(conn, child, live_pids, stuck_config, now)?);
    }

    Ok(TreeNode {
        id: row.id.clone(),
        intent: handoff
            .and_then(|h| h.headline)
            .unwrap_or_else(|| "".to_string()),
        status: status_from_row(row, live_pids, stuck_config, now).to_string(),
        duration_secs: duration_secs(row),
        children: tree_children,
    })
}

fn build_child_summaries(
    conn: &Connection,
    child_rows: &[SessionRow],
    live_pids: &LivePidIndex,
    stuck_config: &StuckConfig,
    now: DateTime<Utc>,
) -> Result<Vec<ChildSummary>, GaalError> {
    let mut out = Vec::with_capacity(child_rows.len());
    for child in child_rows {
        let handoff = get_handoff(conn, &child.id)?;
        out.push(ChildSummary {
            id: child.id.clone(),
            status: status_from_row(child, live_pids, stuck_config, now).to_string(),
            started_at: child.started_at.clone(),
            duration_secs: duration_secs(child),
            headline: handoff.and_then(|h| h.headline),
        });
    }
    Ok(out)
}

fn collect_files(facts: &[Fact], mode: FilesMode) -> FileOps {
    let mut read = Vec::new();
    let mut written = Vec::new();
    let mut edited = Vec::new();

    let mut read_seen = HashSet::new();
    let mut written_seen = HashSet::new();
    let mut edited_seen = HashSet::new();

    for fact in facts {
        match fact.fact_type {
            FactType::FileRead if mode == FilesMode::Read || mode == FilesMode::All => {
                if let Some(path) = fact_path(fact) {
                    push_unique(&mut read, &mut read_seen, path);
                }
            }
            FactType::FileWrite if mode == FilesMode::Write || mode == FilesMode::All => {
                if let Some(path) = fact_path(fact) {
                    if is_edit_fact(fact) {
                        push_unique(&mut edited, &mut edited_seen, path);
                    } else {
                        push_unique(&mut written, &mut written_seen, path);
                    }
                }
            }
            _ => {}
        }
    }

    FileOps {
        read,
        written,
        edited,
    }
}

fn collect_commands(facts: &[Fact]) -> Vec<CommandEntry> {
    let mut out = Vec::new();
    for fact in facts {
        if !matches!(fact.fact_type, FactType::Command) {
            continue;
        }

        let cmd = fact
            .detail
            .clone()
            .or_else(|| fact.subject.clone())
            .unwrap_or_else(|| "".to_string());
        out.push(CommandEntry {
            cmd,
            exit_code: fact.exit_code.unwrap_or(0),
            ts: fact.ts.clone(),
        });
    }
    out
}

fn collect_errors(facts: &[Fact]) -> Vec<ErrorEntry> {
    let mut out = Vec::new();

    for fact in facts {
        if matches!(fact.fact_type, FactType::Error) {
            out.push(ErrorEntry {
                tool: fact.subject.clone().unwrap_or_else(|| "tool".to_string()),
                cmd: fact
                    .subject
                    .clone()
                    .or_else(|| fact.detail.clone())
                    .unwrap_or_else(|| "".to_string()),
                exit_code: fact.exit_code.unwrap_or(1),
                snippet: truncate(&fact.detail.clone().unwrap_or_else(|| "".to_string()), 280),
                ts: fact.ts.clone(),
            });
            continue;
        }

        if matches!(fact.fact_type, FactType::Command) && fact.exit_code.unwrap_or(0) != 0 {
            let cmd = fact
                .detail
                .clone()
                .or_else(|| fact.subject.clone())
                .unwrap_or_else(|| "".to_string());
            out.push(ErrorEntry {
                tool: "Bash".to_string(),
                cmd: cmd.clone(),
                exit_code: fact.exit_code.unwrap_or(1),
                snippet: truncate(&cmd, 280),
                ts: fact.ts.clone(),
            });
        }
    }

    out
}

fn collect_git_ops(facts: &[Fact]) -> Vec<GitOp> {
    let mut out = Vec::new();

    for fact in facts {
        if !matches!(fact.fact_type, FactType::GitOp) {
            continue;
        }

        let message = fact
            .detail
            .clone()
            .or_else(|| fact.subject.clone())
            .unwrap_or_else(|| "".to_string());
        out.push(GitOp {
            op: parse_git_op(&message),
            message,
            ts: fact.ts.clone(),
        });
    }

    out
}

fn parse_git_op(cmd: &str) -> String {
    let mut parts = cmd.split_whitespace();
    let first = parts.next();
    let second = parts.next();

    match (first, second) {
        (Some("git"), Some(op)) => op.to_string(),
        _ => "git".to_string(),
    }
}

fn fact_path(fact: &Fact) -> Option<String> {
    if let Some(subject) = fact.subject.clone() {
        let trimmed = subject.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }

    if let Some(detail) = fact.detail.as_ref() {
        return extract_path_from_detail(detail);
    }

    None
}

fn extract_path_from_detail(detail: &str) -> Option<String> {
    let parsed = serde_json::from_str::<Value>(detail).ok()?;
    ["file_path", "path", "target_file", "filepath"]
        .iter()
        .find_map(|key| {
            parsed
                .get(key)
                .and_then(Value::as_str)
                .map(|text| text.to_string())
        })
}

fn is_edit_fact(fact: &Fact) -> bool {
    let Some(detail) = fact.detail.as_ref() else {
        return false;
    };
    let Ok(parsed) = serde_json::from_str::<Value>(detail) else {
        return false;
    };

    parsed.get("old_string").is_some()
        || parsed.get("replace_all").is_some()
        || parsed.get("oldText").is_some()
        || parsed.get("newText").is_some()
}

fn split_csv(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

fn push_unique(vec: &mut Vec<String>, seen: &mut HashSet<String>, value: String) {
    if seen.insert(value.clone()) {
        vec.push(value);
    }
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

fn status_from_row(
    row: &SessionRow,
    live_pids: &LivePidIndex,
    stuck_config: &StuckConfig,
    now: DateTime<Utc>,
) -> &'static str {
    let stuck_silence_secs = stuck_config
        .silence_for_engine(parse_engine(&row.engine))
        .max(IDLE_SECS);
    match compute_session_status(&status_params_for_row(
        row,
        resolve_pid(live_pids, row),
        now,
        stuck_silence_secs,
    )) {
        SessionStatus::Active => "active",
        SessionStatus::Idle => "idle",
        SessionStatus::Stuck => "stuck",
        SessionStatus::Completed => "completed",
        SessionStatus::Failed => "failed",
        SessionStatus::Unknown => "unknown",
    }
}

fn status_params_for_row<'a>(
    row: &'a SessionRow,
    pid: Option<u32>,
    now: DateTime<Utc>,
    stuck_silence_secs: u64,
) -> StatusParams<'a> {
    let pid_alive = pid.map(is_pid_alive).unwrap_or(false);
    if row.ended_at.is_some() || !pid_alive {
        return StatusParams {
            ended_at: row.ended_at.as_deref(),
            exit_signal: row.exit_signal.as_deref(),
            pid_alive,
            silence_secs: 0,
            loop_detected: false,
            context_pct: 0.0,
            permission_blocked: false,
            stuck_silence_secs,
            executing_command: false,
        };
    }

    let (silence_secs, loop_detected, context_pct, permission_blocked, executing_command) =
        if let Some(engine) = parse_engine(&row.engine) {
            let runtime = probe_runtime(Path::new(&row.jsonl_path), engine, STATUS_TAIL_LINES);
            let silence_secs = runtime
                .last_event_ts
                .as_deref()
                .and_then(parse_ts)
                .or_else(|| row.last_event_at.as_deref().and_then(parse_ts))
                .map(|ts| now.signed_duration_since(ts).num_seconds().max(0) as u64)
                .unwrap_or(0);
            let loop_detected = action_loop_detected(&runtime.recent_actions);
            let permission_blocked = runtime.permission_blocked;
            let executing_command = runtime.executing_command;

            let tokens_used = (row.total_input_tokens + row.total_output_tokens).max(0);
            let tokens_limit = context_limit_tokens(engine, row.model.as_deref());
            let context_pct = pct_used(tokens_used, tokens_limit);
            (
                silence_secs,
                loop_detected,
                context_pct,
                permission_blocked,
                executing_command,
            )
        } else {
            let silence_secs = row
                .last_event_at
                .as_deref()
                .and_then(parse_ts)
                .map(|ts| now.signed_duration_since(ts).num_seconds().max(0) as u64)
                .unwrap_or(0);
            (silence_secs, false, 0.0, false, false)
        };

    StatusParams {
        ended_at: row.ended_at.as_deref(),
        exit_signal: row.exit_signal.as_deref(),
        pid_alive,
        silence_secs,
        loop_detected,
        context_pct,
        permission_blocked,
        stuck_silence_secs,
        executing_command,
    }
}

fn parse_engine(raw: &str) -> Option<Engine> {
    Engine::from_str(&raw.to_ascii_lowercase()).ok()
}

fn duration_secs(row: &SessionRow) -> u64 {
    let start = parse_ts(&row.started_at).unwrap_or_else(Utc::now);
    let end = row
        .ended_at
        .as_deref()
        .and_then(parse_ts)
        .or_else(|| row.last_event_at.as_deref().and_then(parse_ts))
        .unwrap_or_else(Utc::now);

    let secs = end.signed_duration_since(start).num_seconds();
    secs.max(0) as u64
}

fn parse_ts(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|ts| ts.with_timezone(&Utc))
}

fn avg_tokens(total: u64, turns: u32) -> u64 {
    if turns == 0 {
        return 0;
    }
    total / u64::from(turns)
}

fn print_human(records: &[ShowData], args: &ShowArgs) {
    let any_fact_filter = args.files.is_some() || args.commands || args.errors || args.git;

    for (idx, data) in records.iter().enumerate() {
        if idx > 0 {
            println!();
            println!("---");
            println!();
        }

        let record = &data.record;
        println!("ID: {}", record.id);
        println!("Engine: {}", record.engine);
        println!("Model: {}", record.model);
        println!("Status: {}", record.status);
        println!("Started: {}", record.started_at);
        if let Some(ended_at) = &record.ended_at {
            println!("Ended: {}", ended_at);
        }
        println!("Duration: {}s", record.duration_secs);
        println!("CWD: {}", record.cwd);
        println!(
            "Tokens: in={} out={}",
            record.tokens.input, record.tokens.output
        );
        println!("Turns: {}", record.turns);
        println!("Tools: {}", record.tools_used);
        println!("Children: {}", record.child_count);

        if let Some(headline) = &record.headline {
            println!("Headline: {}", headline);
        }

        if !record.tags.is_empty() {
            println!("Tags: {}", record.tags.join(", "));
        }

        if let Some(exit_signal) = &record.exit_signal {
            println!("Exit: {}", exit_signal);
        }

        println!("Last Event: {}", record.last_event_at);

        if !record.children.is_empty() {
            println!("Child IDs: {}", record.children.join(", "));
        }

        if args.source {
            println!("Source: {}", record.jsonl_path);
        }

        // Show files: when explicitly requested OR when no fact filter is set (show all)
        let show_files = args.files.is_some() || !any_fact_filter;
        if show_files {
            print_path_group("Files read", &record.files.read);
            print_path_group("Files written", &record.files.written);
            print_path_group("Files edited", &record.files.edited);
        }

        // Show commands: when explicitly requested OR when no fact filter is set
        let show_commands = args.commands || !any_fact_filter;
        if show_commands && !record.commands.is_empty() {
            println!("Commands:");
            for cmd in &record.commands {
                println!("  $ {} (exit {})", cmd.cmd, cmd.exit_code);
            }
        }

        // Show errors: when explicitly requested OR when no fact filter is set
        let show_errors = args.errors || !any_fact_filter;
        if show_errors && !record.errors.is_empty() {
            println!("Errors:");
            for err in &record.errors {
                println!(
                    "  - [{}] {} exit={} {}",
                    err.ts, err.tool, err.exit_code, err.snippet
                );
            }
        }

        // Show git ops: when explicitly requested OR when no fact filter is set
        let show_git = args.git || !any_fact_filter;
        if show_git && !record.git_ops.is_empty() {
            println!("Git ops:");
            for op in &record.git_ops {
                println!("  - {} {}", op.op, op.message);
            }
        }

        if args.tokens {
            if let Some(tokens) = &data.token_breakdown {
                println!(
                    "Token breakdown: turns={} avg_in/turn={} avg_out/turn={}",
                    tokens.turns, tokens.avg_input_per_turn, tokens.avg_output_per_turn
                );
            }
        }

        if args.children {
            if let Some(children) = &data.child_sessions {
                if !children.is_empty() {
                    println!("Child sessions:");
                    for child in children {
                        if let Some(headline) = &child.headline {
                            println!(
                                "  - {} {} {}s {}",
                                child.id, child.status, child.duration_secs, headline
                            );
                        } else {
                            println!("  - {} {} {}s", child.id, child.status, child.duration_secs);
                        }
                    }
                }
            }
        }

        if args.tree {
            if let Some(tree) = &data.tree {
                println!("Tree:");
                print_tree(tree, 0);
            }
        }

        if args.trace {
            if let Some(trace) = &data.trace {
                println!("Trace:");
                for fact in trace {
                    let kind = fact.fact_type.as_str();
                    let subject = fact.subject.clone().unwrap_or_else(|| "".to_string());
                    let detail = fact
                        .detail
                        .as_ref()
                        .map(|s| truncate(s, 120))
                        .unwrap_or_else(|| "".to_string());
                    println!("  - [{}] {} {} {}", fact.ts, kind, subject, detail);
                }
            }
        }
    }
}

fn print_path_group(title: &str, values: &[String]) {
    if values.is_empty() {
        return;
    }
    println!("{}:", title);
    for value in values {
        println!("  - {}", value);
    }
}

fn print_tree(node: &TreeNode, depth: usize) {
    let indent = "  ".repeat(depth);
    println!(
        "{}- {} [{}] {}s {}",
        indent, node.id, node.status, node.duration_secs, node.intent
    );
    for child in &node.children {
        print_tree(child, depth + 1);
    }
}

fn truncate(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_string();
    }
    value.chars().take(max).collect::<String>()
}
