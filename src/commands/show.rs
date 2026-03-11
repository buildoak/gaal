use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use clap::{Args, ValueEnum};
use rusqlite::{named_params, Connection};
use serde::Serialize;
use serde_json::{json, Value};

use crate::commands::active::probe_runtime;
use crate::db::{open_db, open_db_readonly};
use crate::db::queries::{
    count_children, get_children, get_facts, get_handoff, get_session, get_tags, SessionRow,
};
use crate::commands::index::{index_discovered_session, IndexOutcome};
use crate::discovery::active::{find_active_sessions, is_pid_alive, probe_pid};
use crate::error::GaalError;
use crate::model::{
    compute_session_status, CommandEntry, ErrorEntry, Fact, FactType, FileOps, GitOp,
    SessionRecord, SessionStatus, StatusParams, TokenUsage,
};
use crate::output::human::format_cwd;
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

    /// Include all arrays and fields (full output).
    #[arg(short = 'F', long)]
    pub full: bool,

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

#[derive(Debug, Clone, Serialize)]
struct FileCount {
    read: usize,
    written: usize,
    edited: usize,
}

#[derive(Debug, Clone)]
struct ShowData {
    record: SessionRecord,
    trace: Option<Vec<Fact>>,
    tree: Option<TreeNode>,
    child_sessions: Option<Vec<ChildSummary>>,
    token_breakdown: Option<TokenBreakdown>,
    file_count: Option<FileCount>,
    command_count: Option<usize>,
    error_count: Option<usize>,
    git_op_count: Option<usize>,
}

#[derive(Default)]
struct LivePidIndex {
    by_id: HashMap<String, u32>,
    by_path: HashMap<String, u32>,
}

/// Execute the `gaal show` command.
pub fn run(args: ShowArgs) -> Result<(), GaalError> {
    let conn = open_db_readonly()?;
    let session_rows = match resolve_sessions(&conn, &args) {
        Ok(rows) => rows,
        Err(GaalError::NotFound(ref id)) => {
            // Session not in DB — check if it's an active (un-indexed) session.
            if let Some(rows) = try_index_active_session(id) {
                rows
            } else {
                return Err(GaalError::NotFound(id.clone()));
            }
        }
        Err(err) => return Err(err),
    };

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

    let mut out = Vec::with_capacity(session_rows.len());
    for row in session_rows {
        out.push(build_show_data(
            &conn,
            &row,
            &args,
            &live_pids,
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
    now: DateTime<Utc>,
) -> Result<ShowData, GaalError> {
    let any_fact_filter = args.files.is_some() || args.commands || args.errors || args.git;
    // I36: human mode respects summary-by-default. Only --full overrides.
    let summary_mode = !args.full && !any_fact_filter;
    let include_all_facts = !any_fact_filter && args.full;
    let include_files = args.files.is_some() || include_all_facts;
    let include_commands = args.commands || include_all_facts;
    let include_errors = args.errors || include_all_facts;
    let include_git = args.git || include_all_facts;
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

    let file_count = if summary_mode {
        let all_files = collect_files(&facts, FilesMode::All);
        Some(FileCount {
            read: all_files.read.len(),
            written: all_files.written.len(),
            edited: all_files.edited.len(),
        })
    } else {
        None
    };
    let command_count = if summary_mode {
        Some(collect_commands(&facts).len())
    } else {
        None
    };
    let error_count = if summary_mode {
        Some(collect_errors(&facts).len())
    } else {
        None
    };
    let git_op_count = if summary_mode {
        Some(collect_git_ops(&facts).len())
    } else {
        None
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
        status: status_from_row(row, live_pids, now).to_string(),
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
        Some(build_tree(conn, row, live_pids, now)?)
    } else {
        None
    };
    let child_sessions = if args.children {
        Some(build_child_summaries(conn, &child_rows, live_pids, now)?)
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
        file_count,
        command_count,
        error_count,
        git_op_count,
    })
}

fn to_json_value(data: ShowData, args: &ShowArgs) -> Result<Value, GaalError> {
    let ShowData {
        record,
        trace,
        tree,
        child_sessions,
        token_breakdown,
        file_count,
        command_count,
        error_count,
        git_op_count,
    } = data;

    let mut map = match serde_json::to_value(record)
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
    let summary_mode = !args.full && !any_fact_filter;
    if summary_mode {
        map.remove("files");
        map.remove("commands");
        map.remove("errors");
        map.remove("git_ops");

        map.insert(
            "file_count".to_string(),
            serde_json::to_value(file_count.unwrap_or(FileCount {
                read: 0,
                written: 0,
                edited: 0,
            }))
            .map_err(|e| GaalError::Internal(format!("failed to serialize file count: {e}")))?,
        );
        map.insert(
            "command_count".to_string(),
            json!(command_count.unwrap_or(0)),
        );
        map.insert("error_count".to_string(), json!(error_count.unwrap_or(0)));
        map.insert("git_op_count".to_string(), json!(git_op_count.unwrap_or(0)));

        map.remove("children");
        map.remove("parent_id");
        map.remove("child_count");
        map.remove("ended_at");
        map.remove("last_event_at");
        map.remove("exit_signal");
    } else if any_fact_filter {
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

    if let Some(trace) = trace {
        map.insert(
            "trace".to_string(),
            serde_json::to_value(trace)
                .map_err(|e| GaalError::Internal(format!("failed to serialize trace: {e}")))?,
        );
    }

    if let Some(tree) = tree {
        map.insert(
            "tree".to_string(),
            serde_json::to_value(tree)
                .map_err(|e| GaalError::Internal(format!("failed to serialize tree: {e}")))?,
        );
    }

    if let Some(children) = child_sessions {
        map.insert(
            "child_sessions".to_string(),
            serde_json::to_value(children).map_err(|e| {
                GaalError::Internal(format!("failed to serialize child sessions: {e}"))
            })?,
        );
    }

    if let Some(tokens) = token_breakdown {
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
    now: DateTime<Utc>,
) -> Result<TreeNode, GaalError> {
    let handoff = get_handoff(conn, &row.id)?;
    let children = get_children(conn, &row.id)?;
    let mut tree_children = Vec::with_capacity(children.len());

    for child in &children {
        tree_children.push(build_tree(conn, child, live_pids, now)?);
    }

    Ok(TreeNode {
        id: row.id.clone(),
        intent: handoff
            .and_then(|h| h.headline)
            .unwrap_or_else(|| "".to_string()),
        status: status_from_row(row, live_pids, now).to_string(),
        duration_secs: duration_secs(row),
        children: tree_children,
    })
}

fn build_child_summaries(
    conn: &Connection,
    child_rows: &[SessionRow],
    live_pids: &LivePidIndex,
    now: DateTime<Utc>,
) -> Result<Vec<ChildSummary>, GaalError> {
    let mut out = Vec::with_capacity(child_rows.len());
    for child in child_rows {
        let handoff = get_handoff(conn, &child.id)?;
        out.push(ChildSummary {
            id: child.id.clone(),
            status: status_from_row(child, live_pids, now).to_string(),
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
    now: DateTime<Utc>,
) -> &'static str {
    match compute_session_status(&status_params_for_row(
        row,
        resolve_pid(live_pids, row),
        now,
    )) {
        SessionStatus::Active => "active",
        SessionStatus::Idle => "idle",
        SessionStatus::Completed => "completed",
        SessionStatus::Failed => "failed",
        SessionStatus::Interrupted => "interrupted",
        SessionStatus::Starting => "starting",
        SessionStatus::Unknown => "unknown",
    }
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
            .and_then(parse_ts)
            .or_else(|| row.last_event_at.as_deref().and_then(parse_ts))
            .map(|ts| now.signed_duration_since(ts).num_seconds().max(0) as u64)
            .unwrap_or(0)
    } else {
        row.last_event_at
            .as_deref()
            .and_then(parse_ts)
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
    // I36: summary mode by default — full detail only with --full
    let summary_mode = !args.full && !any_fact_filter;

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
        println!("Duration: {}s", record.duration_secs);
        println!("CWD: {}", format_cwd(&record.cwd, 80));

        if let Some(headline) = &record.headline {
            println!("Headline: {}", headline);
        }

        if summary_mode {
            // Brief summary card: counts only, no full lists.
            let fc = data.file_count.as_ref();
            let files_read = fc.map(|f| f.read).unwrap_or(0);
            let files_written = fc.map(|f| f.written).unwrap_or(0);
            let files_edited = fc.map(|f| f.edited).unwrap_or(0);
            let cmds = data.command_count.unwrap_or(0);
            let errs = data.error_count.unwrap_or(0);
            let git_ops = data.git_op_count.unwrap_or(0);
            println!(
                "Files: read={} written={} edited={}",
                files_read, files_written, files_edited
            );
            println!(
                "Ops: commands={} errors={} git={}",
                cmds, errs, git_ops
            );
            println!(
                "Tokens: in={} out={}  Turns: {}  Tools: {}",
                record.tokens.input, record.tokens.output, record.turns, record.tools_used
            );
        } else {
            // Full detail mode (--full or explicit fact filter).
            println!(
                "Tokens: in={} out={}",
                record.tokens.input, record.tokens.output
            );
            println!("Turns: {}", record.turns);
            println!("Tools: {}", record.tools_used);
            println!("Children: {}", record.child_count);

            if !record.tags.is_empty() {
                println!("Tags: {}", record.tags.join(", "));
            }

            if let Some(exit_signal) = &record.exit_signal {
                println!("Exit: {}", exit_signal);
            }

            if let Some(ended_at) = &record.ended_at {
                println!("Ended: {}", ended_at);
            }

            println!("Last Event: {}", record.last_event_at);

            if !record.children.is_empty() {
                println!("Child IDs: {}", record.children.join(", "));
            }

            // Show files: when explicitly requested OR --full
            let show_files = args.files.is_some() || args.full;
            if show_files {
                print_path_group("Files read", &record.files.read);
                print_path_group("Files written", &record.files.written);
                print_path_group("Files edited", &record.files.edited);
            }

            // Show commands: when explicitly requested OR --full
            let show_commands = args.commands || args.full;
            if show_commands && !record.commands.is_empty() {
                println!("Commands:");
                for cmd in &record.commands {
                    println!("  $ {} (exit {})", cmd.cmd, cmd.exit_code);
                }
            }

            // Show errors: when explicitly requested OR --full
            let show_errors = args.errors || args.full;
            if show_errors && !record.errors.is_empty() {
                println!("Errors:");
                for err in &record.errors {
                    println!(
                        "  - [{}] {} exit={} {}",
                        err.ts, err.tool, err.exit_code, err.snippet
                    );
                }
            }

            // Show git ops: when explicitly requested OR --full
            let show_git = args.git || args.full;
            if show_git && !record.git_ops.is_empty() {
                println!("Git ops:");
                for op in &record.git_ops {
                    println!("  - {} {}", op.op, op.message);
                }
            }
        }

        if args.source {
            println!("Source: {}", record.jsonl_path);
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

/// Attempt to find an active (un-indexed) session and index it on-the-fly.
///
/// When `gaal show <id>` returns NotFound from the DB, this checks if the session
/// is currently running (discovered by `find_active_sessions`). If found, indexes
/// the JSONL on-the-fly and returns the DB row.
fn try_index_active_session(id: &str) -> Option<Vec<SessionRow>> {
    use crate::discovery::{discover_sessions, DiscoveredSession};

    // Check active sessions for a matching ID prefix.
    let active_sessions = find_active_sessions().ok()?;
    let matched = active_sessions
        .iter()
        .find(|s| {
            s.id.as_ref()
                .map(|sid| sid.starts_with(id) || id.starts_with(sid.as_str()))
                .unwrap_or(false)
        });

    if let Some(active) = matched {
        let jsonl_path = active.jsonl_path.as_ref()?;
        let meta = std::fs::metadata(jsonl_path).ok()?;
        let engine = active.engine.clone();
        let session_id = active.id.as_ref()?;
        let short_id = truncate_session_id(session_id, &engine);

        let discovered = DiscoveredSession {
            id: short_id.clone(),
            engine,
            path: jsonl_path.clone(),
            model: None,
            cwd: Some(active.cwd.clone()),
            started_at: None,
            file_size: meta.len(),
        };

        let mut conn = open_db().ok()?;
        match index_discovered_session(&mut conn, &discovered, true) {
            Ok(IndexOutcome::Indexed) => {
                eprintln!("[live -- indexed on-the-fly] Session {short_id}");
            }
            Ok(IndexOutcome::Skipped) => {
                eprintln!("[live -- already indexed] Session {short_id}");
            }
            Err(err) => {
                eprintln!("On-the-fly indexing failed: {err}");
                return None;
            }
        }

        // Re-open read-only and resolve.
        let conn = open_db_readonly().ok()?;
        let rows = find_session_ids_by_prefix(&conn, &short_id).ok()?;
        if rows.is_empty() {
            return None;
        }
        let mut result = Vec::new();
        for row_id in rows {
            if let Ok(row) = load_session_by_exact_id(&conn, &row_id) {
                result.push(row);
            }
        }
        if result.is_empty() {
            None
        } else {
            Some(result)
        }
    } else {
        // Also check discovered (on-disk but not active) sessions.
        let discovered = discover_sessions(None).ok()?;
        let matched = discovered
            .iter()
            .find(|s| s.id.starts_with(id) || id.starts_with(&s.id));

        let session = matched?;
        let short_id = session.id.clone();

        let mut conn = open_db().ok()?;
        match index_discovered_session(&mut conn, session, true) {
            Ok(IndexOutcome::Indexed) => {
                eprintln!("[not yet indexed -- indexed on-the-fly] Session {short_id}");
            }
            Ok(IndexOutcome::Skipped) => {}
            Err(err) => {
                eprintln!("On-the-fly indexing failed: {err}");
                return None;
            }
        }

        let conn = open_db_readonly().ok()?;
        let rows = find_session_ids_by_prefix(&conn, &short_id).ok()?;
        let mut result = Vec::new();
        for row_id in rows {
            if let Ok(row) = load_session_by_exact_id(&conn, &row_id) {
                result.push(row);
            }
        }
        if result.is_empty() {
            None
        } else {
            Some(result)
        }
    }
}

/// Truncate a session ID to match the short-ID convention used by the indexer.
fn truncate_session_id(raw: &str, engine: &Engine) -> String {
    match engine {
        Engine::Claude => raw.chars().take(8).collect(),
        Engine::Codex => {
            let hex: String = raw.chars().filter(|c| *c != '-').collect();
            if hex.len() > 8 {
                hex[hex.len() - 8..].to_string()
            } else {
                hex
            }
        }
    }
}
