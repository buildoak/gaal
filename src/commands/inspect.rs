use std::collections::HashSet;
use std::path::Path;

use chrono::{DateTime, Utc};
use clap::{Args, ValueEnum};
use rusqlite::{named_params, Connection};
use serde::Serialize;
use serde_json::{json, Value};

use crate::db::open_db_readonly;
use crate::db::queries::{get_facts, get_handoff, get_session, get_tags, SessionRow};
use crate::error::GaalError;
use crate::model::{
    CommandEntry, ErrorEntry, Fact, FactType, FileOps, GitOp, SessionRecord, TokenUsage,
};
use crate::output::human::{
    format_cwd, format_duration, format_tokens, print_table_with_kinds, ColumnKind,
};
use crate::output::json::print_json;
use crate::parser::event::EventKind;

/// File-operation output mode for `gaal inspect --files`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum FilesMode {
    /// Include file reads only.
    Read,
    /// Include file writes/edits only.
    Write,
    /// Include both reads and writes/edits.
    All,
}

/// CLI arguments for `gaal inspect`.
#[derive(Debug, Clone, Args)]
pub struct InspectArgs {
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

    /// Include full fact timeline.
    #[arg(long)]
    pub trace: bool,

    /// Include source JSONL path.
    #[arg(long)]
    pub source: bool,

    /// Include empty/low-signal subagents in coordinator views.
    #[arg(long)]
    pub include_empty: bool,

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
struct TokenBreakdown {
    input_total: u64,
    input_total_note: &'static str,
    output_total: u64,
    cache_read_input_tokens: i64,
    cache_creation_input_tokens: i64,
    peak_context: u64,
    peak_context_note: &'static str,
    reasoning_tokens: i64,
    estimated_cost_usd: f64,
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

#[derive(Debug, Clone, Serialize)]
struct SubagentSummary {
    id: String,
    model: String,
    total_tokens: i64,
    duration: String,
    description: String,
}

#[derive(Debug, Clone)]
struct InspectData {
    record: SessionRecord,
    parent_id: Option<String>,
    session_type: String,
    subagent_type: Option<String>,
    /// Task description computed via the 3-level cascade: handoff headline -> parent description -> first user prompt.
    task: Option<String>,
    subagents: Vec<SubagentSummary>,
    trace: Option<Vec<Fact>>,
    token_breakdown: Option<TokenBreakdown>,
    file_count: Option<FileCount>,
    command_count: Option<usize>,
    error_count: Option<usize>,
    git_op_count: Option<usize>,
}

/// Execute the `gaal inspect` command.
pub fn run(args: InspectArgs) -> Result<(), GaalError> {
    if args.id.is_none() && args.ids.is_none() && args.tag.is_none() {
        return Err(GaalError::ParseError(
            "inspect requires a session id, `latest`, --ids, or --tag".to_string(),
        ));
    }

    let conn = open_db_readonly()?;
    let session_rows = resolve_sessions(&conn, &args)?;

    let mut out = Vec::with_capacity(session_rows.len());
    for row in session_rows {
        out.push(build_inspect_data(&conn, &row, &args)?);
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

    let payload = to_json_value(data, &args)?;
    print_json(&payload).map_err(GaalError::from)?;
    Ok(())
}

fn resolve_sessions(conn: &Connection, args: &InspectArgs) -> Result<Vec<SessionRow>, GaalError> {
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

pub(crate) fn resolve_one(conn: &Connection, raw_id: &str) -> Result<SessionRow, GaalError> {
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

pub(crate) fn find_latest_session_id(conn: &Connection) -> Result<String, GaalError> {
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

pub(crate) fn find_session_ids_by_prefix(
    conn: &Connection,
    prefix: &str,
) -> Result<Vec<String>, GaalError> {
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

pub(crate) fn load_session_by_exact_id(
    conn: &Connection,
    id: &str,
) -> Result<SessionRow, GaalError> {
    get_session(conn, id)?.ok_or_else(|| GaalError::NotFound(id.to_string()))
}

fn build_inspect_data(
    conn: &Connection,
    row: &SessionRow,
    args: &InspectArgs,
) -> Result<InspectData, GaalError> {
    let any_fact_filter = args.files.is_some() || args.commands || args.errors || args.git;
    // Summary mode by default - compact card output unless --full or specific fact filters
    let summary_mode = !args.full && !any_fact_filter;
    let include_all_facts = !any_fact_filter && args.full;
    let include_files = args.files.is_some() || include_all_facts;
    let include_commands = args.commands || include_all_facts;
    let include_errors = args.errors || include_all_facts;
    let include_git = args.git || include_all_facts;
    let include_trace = args.trace;

    let facts = get_facts(conn, &row.id, None)?;

    let handoff = get_handoff(conn, &row.id)?;
    let tags = get_tags(conn, &row.id)?;
    let subagents = if row.session_type == "coordinator" {
        get_child_sessions(conn, &row.id)?
            .into_iter()
            .filter(|child| args.include_empty || subagent_has_meaningful_content(child))
            .map(|child| {
                let child_facts = get_facts(conn, &child.id, None)?;
                Ok(SubagentSummary {
                    id: child.id.clone(),
                    model: child.model.clone().unwrap_or_else(|| "unknown".to_string()),
                    total_tokens: child.total_input_tokens
                        + child.total_output_tokens
                        + child.cache_read_tokens
                        + child.cache_creation_tokens,
                    duration: format_duration(duration_secs(&child) as i64),
                    description: first_user_prompt(&child_facts)
                        .unwrap_or_else(|| "subagent task".to_string()),
                })
            })
            .collect::<Result<Vec<_>, GaalError>>()?
    } else {
        Vec::new()
    };

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

    let record = SessionRecord {
        id: row.id.clone(),
        engine: row.engine.clone(),
        model: row.model.clone().unwrap_or_else(|| "unknown".to_string()),
        cwd: row.cwd.clone().unwrap_or_default(),
        started_at: row.started_at.clone(),
        ended_at: row.ended_at.clone(),
        status: "".to_string(), // AF2: Status field exists but should be removed from JSON output
        duration_secs: duration_secs(row),
        tokens: TokenUsage {
            input: row.total_input_tokens.max(0) as u64,
            output: row.total_output_tokens.max(0) as u64,
        },
        peak_context: row.peak_context.max(0) as u64,
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
    let token_breakdown = if args.tokens {
        // Prefer DB-stored cache tokens; fall back to re-parsing JSONL if DB has zeros
        // (handles sessions indexed before the schema migration).
        let (cache_read, cache_creation) =
            if row.cache_read_tokens > 0 || row.cache_creation_tokens > 0 {
                (row.cache_read_tokens, row.cache_creation_tokens)
            } else {
                extract_cache_tokens(row)
            };
        Some(TokenBreakdown {
            input_total: record.tokens.input,
            input_total_note: "non-cached input tokens summed across the whole session",
            output_total: record.tokens.output,
            cache_read_input_tokens: cache_read,
            cache_creation_input_tokens: cache_creation,
            peak_context: record.peak_context,
            peak_context_note:
                "max single-turn input = non-cached input + cache read + cache creation",
            reasoning_tokens: row.reasoning_tokens,
            estimated_cost_usd: crate::db::queries::estimate_session_cost(row),
            turns: record.turns,
            avg_input_per_turn: avg_tokens(record.tokens.input, record.turns),
            avg_output_per_turn: avg_tokens(record.tokens.output, record.turns),
        })
    } else {
        None
    };

    // Compute task via 3-level cascade: handoff headline -> parent description -> first user prompt.
    let task = record
        .headline
        .clone()
        .or_else(|| {
            if row.session_type == "subagent" {
                parent_subagent_description_for_inspect(conn, row)
                    .ok()
                    .flatten()
            } else {
                None
            }
        })
        .or_else(|| first_user_prompt_for_inspect(&facts));

    Ok(InspectData {
        record,
        parent_id: row.parent_id.clone(),
        session_type: row.session_type.clone(),
        subagent_type: row.subagent_type.clone(),
        task,
        subagents,
        trace,
        token_breakdown,
        file_count,
        command_count,
        error_count,
        git_op_count,
    })
}

fn to_json_value(data: InspectData, args: &InspectArgs) -> Result<Value, GaalError> {
    let InspectData {
        record,
        parent_id,
        session_type,
        subagent_type,
        task,
        subagents,
        trace,
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

    // AF2: Always remove status field from JSON output
    map.remove("status");
    // P1: Add `task` field via 3-level cascade (handoff headline -> parent description -> first user prompt).
    if let Some(task_value) = task {
        map.insert("task".to_string(), json!(task_value));
    }
    map.insert("session_type".to_string(), json!(session_type));
    if let Some(ref subagent_type) = subagent_type {
        map.insert("subagent_type".to_string(), json!(subagent_type));
    }
    if let Some(parent_id) = parent_id {
        map.insert("parent_id".to_string(), json!(parent_id));
    }
    if session_type == "coordinator" {
        map.insert(
            "subagents".to_string(),
            serde_json::to_value(subagents)
                .map_err(|e| GaalError::Internal(format!("failed to serialize subagents: {e}")))?,
        );
    }

    let any_fact_filter = args.files.is_some() || args.errors || args.commands || args.git;
    let summary_mode = !args.full && !any_fact_filter;
    if summary_mode {
        // AF2: Compact card format - remove full arrays, add counts
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

        // AF2: Remove fields for compact card
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
    let mut seen: std::collections::HashMap<(String, i32), usize> =
        std::collections::HashMap::new();

    for fact in facts {
        if matches!(fact.fact_type, FactType::Error) {
            let dedup_cmd = error_dedup_cmd(fact, true);
            let entry = ErrorEntry {
                tool: fact.subject.clone().unwrap_or_else(|| "tool".to_string()),
                cmd: fact
                    .subject
                    .clone()
                    .or_else(|| fact.detail.clone())
                    .unwrap_or_else(|| "".to_string()),
                exit_code: fact.exit_code.unwrap_or(1),
                snippet: truncate(&fact.detail.clone().unwrap_or_else(|| "".to_string()), 280),
                ts: fact.ts.clone(),
            };
            let key = (dedup_cmd, entry.exit_code);
            if let Some(idx) = seen.get(&key).copied() {
                out[idx] = entry;
            } else {
                seen.insert(key, out.len());
                out.push(entry);
            }
            continue;
        }

        if matches!(fact.fact_type, FactType::Command) && fact.exit_code.unwrap_or(0) != 0 {
            let dedup_cmd = error_dedup_cmd(fact, false);
            let cmd = fact
                .detail
                .clone()
                .or_else(|| fact.subject.clone())
                .unwrap_or_else(|| "".to_string());
            let entry = ErrorEntry {
                tool: "Bash".to_string(),
                cmd: cmd.clone(),
                exit_code: fact.exit_code.unwrap_or(1),
                snippet: truncate(&cmd, 280),
                ts: fact.ts.clone(),
            };
            let key = (dedup_cmd, entry.exit_code);
            if let std::collections::hash_map::Entry::Vacant(slot) = seen.entry(key) {
                slot.insert(out.len());
                out.push(entry);
            }
        }
    }

    out
}

fn error_dedup_cmd(fact: &Fact, prefer_wrapped_command: bool) -> String {
    if let Some(subject) = fact.subject.clone() {
        let trimmed = subject.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }

    if prefer_wrapped_command {
        if let Some(detail) = fact.detail.as_deref() {
            if let Some(cmd) = extract_wrapped_shell_command(detail) {
                return cmd;
            }
        }
    }

    fact.detail.clone().unwrap_or_default()
}

fn extract_wrapped_shell_command(detail: &str) -> Option<String> {
    let first_line = detail.lines().next()?.trim();
    let raw = first_line.strip_prefix("Command: /bin/bash -lc ")?;
    Some(unquote_shell_wrapper(raw))
}

fn unquote_shell_wrapper(raw: &str) -> String {
    let trimmed = raw.trim();
    if let Some(stripped) = trimmed
        .strip_prefix('\'')
        .and_then(|value| value.strip_suffix('\''))
    {
        return stripped.to_string();
    }
    if let Some(stripped) = trimmed
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
    {
        return stripped.replace("\\\"", "\"");
    }
    trimmed.to_string()
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

fn get_child_sessions(conn: &Connection, parent_id: &str) -> Result<Vec<SessionRow>, GaalError> {
    let mut stmt = conn
        .prepare(
            "SELECT id, engine, model, cwd, started_at, ended_at, exit_signal, last_event_at,
         parent_id, session_type, jsonl_path, total_input_tokens, total_output_tokens,
         cache_read_tokens, cache_creation_tokens, reasoning_tokens,
         total_tools, total_turns, peak_context, last_indexed_offset, subagent_type
         FROM sessions WHERE parent_id = :parent_id ORDER BY started_at ASC",
        )
        .map_err(GaalError::from)?;
    let rows = stmt
        .query_map(named_params! { ":parent_id": parent_id }, |row| {
            Ok(SessionRow {
                id: row.get("id")?,
                engine: row.get("engine")?,
                model: row.get("model")?,
                cwd: row.get("cwd")?,
                started_at: row.get("started_at")?,
                ended_at: row.get("ended_at")?,
                exit_signal: row.get("exit_signal")?,
                last_event_at: row.get("last_event_at")?,
                parent_id: row.get("parent_id")?,
                session_type: row
                    .get::<_, Option<String>>("session_type")?
                    .unwrap_or_else(|| "standalone".to_string()),
                jsonl_path: row.get("jsonl_path")?,
                total_input_tokens: row.get("total_input_tokens")?,
                total_output_tokens: row.get("total_output_tokens")?,
                cache_read_tokens: row.get::<_, Option<i64>>("cache_read_tokens")?.unwrap_or(0),
                cache_creation_tokens: row
                    .get::<_, Option<i64>>("cache_creation_tokens")?
                    .unwrap_or(0),
                reasoning_tokens: row.get::<_, Option<i64>>("reasoning_tokens")?.unwrap_or(0),
                total_tools: row.get("total_tools")?,
                total_turns: row.get("total_turns")?,
                peak_context: row.get::<_, Option<i64>>("peak_context")?.unwrap_or(0),
                last_indexed_offset: row.get("last_indexed_offset")?,
                subagent_type: row
                    .get::<_, Option<String>>("subagent_type")?
                    .filter(|s| !s.is_empty()),
            })
        })
        .map_err(GaalError::from)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(GaalError::from)
}

fn first_user_prompt(facts: &[Fact]) -> Option<String> {
    facts.iter().find_map(|fact| {
        if matches!(fact.fact_type, FactType::UserPrompt) {
            fact.detail
                .as_deref()
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map(|text| truncate(text, 80))
        } else {
            None
        }
    })
}

/// Extract the first user prompt from already-loaded facts for the task field.
fn first_user_prompt_for_inspect(facts: &[Fact]) -> Option<String> {
    first_user_prompt(facts).map(|text| truncate(&text, 60))
}

/// Look up the parent's toolUseResult description for a subagent session.
fn parent_subagent_description_for_inspect(
    conn: &Connection,
    row: &SessionRow,
) -> Result<Option<String>, GaalError> {
    let Some(parent_id) = row.parent_id.as_deref() else {
        return Ok(None);
    };
    let Some(parent) = get_session(conn, parent_id)? else {
        return Ok(None);
    };
    let parent_jsonl = Path::new(&parent.jsonl_path);
    let Some(parent_dir) = parent_jsonl.parent() else {
        return Ok(None);
    };
    let child_path = Path::new(&row.jsonl_path);
    let summaries =
        crate::subagent::get_subagent_summaries(parent_jsonl, parent_dir).unwrap_or_default();
    for summary in summaries {
        if let Some(path) = summary.jsonl_path.as_deref() {
            if path == child_path {
                let desc = summary.meta.description.trim().to_string();
                if !desc.is_empty() {
                    return Ok(Some(truncate(&desc, 60)));
                }
            }
        }
    }
    Ok(None)
}

fn subagent_has_meaningful_content(row: &SessionRow) -> bool {
    if row.total_turns <= 0 {
        return false;
    }

    let total_tokens = row
        .total_input_tokens
        .saturating_add(row.total_output_tokens);
    total_tokens > 0 || row.total_tools > 0
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

/// Extract cache token breakdown by re-parsing the JSONL.
/// Returns (cache_read_input_tokens, cache_creation_input_tokens).
fn extract_cache_tokens(row: &SessionRow) -> (i64, i64) {
    let path = Path::new(&row.jsonl_path);
    if !path.exists() {
        return (0, 0);
    }

    let engine = match row.engine.as_str() {
        "claude" => crate::parser::Engine::Claude,
        "codex" => crate::parser::Engine::Codex,
        _ => return (0, 0),
    };

    let events = match engine {
        crate::parser::Engine::Claude => crate::parser::claude::parse_events(path),
        crate::parser::Engine::Codex => crate::parser::codex::parse_events(path),
        crate::parser::Engine::Gemini => return (0, 0),
    };

    let events = match events {
        Ok(events) => events,
        Err(_) => return (0, 0),
    };

    let mut seen_keys: HashSet<String> = HashSet::new();
    let mut cache_read = 0_i64;
    let mut cache_creation = 0_i64;

    for event in &events {
        if let EventKind::Usage {
            cache_read_input_tokens,
            cache_creation_input_tokens,
            dedup_key,
            ..
        } = &event.kind
        {
            if let Some(key) = dedup_key {
                if !seen_keys.insert(key.clone()) {
                    continue;
                }
            }
            cache_read += cache_read_input_tokens;
            cache_creation += cache_creation_input_tokens;
        }
    }

    (cache_read, cache_creation)
}

fn print_human(records: &[InspectData], args: &InspectArgs) {
    let any_fact_filter = args.files.is_some() || args.commands || args.errors || args.git;
    // Summary mode by default — full detail only with --full
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
        println!("Started: {}", record.started_at);
        println!("Duration: {}s", record.duration_secs);
        println!("CWD: {}", format_cwd(&record.cwd, 80));
        if data.session_type == "subagent" {
            if let Some(parent_id) = &data.parent_id {
                println!("Parent: {}", parent_id);
            }
            if let Some(ref subagent_type) = data.subagent_type {
                println!("Subagent Type: {}", subagent_type);
            }
        }
        if record.peak_context > 0 {
            println!(
                "Peak Context: {} (max single-turn input incl. cache)",
                format_peak_context(record.peak_context)
            );
        }

        if let Some(headline) = &record.headline {
            println!("Headline: {}", headline);
        }
        if data.session_type == "coordinator" {
            println!("Subagents ({}):", data.subagents.len());
            if !data.subagents.is_empty() {
                let rows = data
                    .subagents
                    .iter()
                    .map(|subagent| {
                        vec![
                            truncate(&subagent.id, 8),
                            subagent.model.clone(),
                            format_tokens(subagent.total_tokens),
                            subagent.duration.clone(),
                        ]
                    })
                    .collect::<Vec<_>>();
                print_table_with_kinds(
                    &["ID", "Model", "Tokens", "Duration"],
                    &rows,
                    &[
                        ColumnKind::Fixed,
                        ColumnKind::Variable,
                        ColumnKind::Fixed,
                        ColumnKind::Fixed,
                    ],
                );
            }
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
            println!("Ops: commands={} errors={} git={}", cmds, errs, git_ops);
            if record.peak_context > 0 {
                println!(
                    "Tokens: in(non-cache)={} out={}  Peak(max turn incl. cache): {}  Turns: {}  Tools: {}",
                    record.tokens.input,
                    record.tokens.output,
                    format_peak_context(record.peak_context),
                    record.turns,
                    record.tools_used
                );
            } else {
                println!(
                    "Tokens: in(non-cache)={} out={}  Turns: {}  Tools: {}",
                    record.tokens.input, record.tokens.output, record.turns, record.tools_used
                );
            }
        } else {
            // Full detail mode (--full or explicit fact filter).
            println!(
                "Tokens: in(non-cache total)={} out={}",
                record.tokens.input, record.tokens.output
            );
            println!("Turns: {}", record.turns);
            println!("Tools: {}", record.tools_used);

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
                    "Token breakdown: turns={} avg_in/turn={} avg_out/turn={} cost=${:.2}",
                    tokens.turns,
                    tokens.avg_input_per_turn,
                    tokens.avg_output_per_turn,
                    tokens.estimated_cost_usd,
                );
                println!(
                    "  Input total: {} ({})",
                    tokens.input_total, tokens.input_total_note
                );
                if tokens.peak_context > 0 {
                    println!(
                        "  Peak context: {} ({})",
                        format_peak_context(tokens.peak_context),
                        tokens.peak_context_note,
                    );
                }
                if tokens.cache_read_input_tokens > 0 || tokens.cache_creation_input_tokens > 0 {
                    println!(
                        "  Cache: read={} creation={}",
                        format_tokens(tokens.cache_read_input_tokens),
                        format_tokens(tokens.cache_creation_input_tokens),
                    );
                }
                if tokens.reasoning_tokens > 0 {
                    println!("  Reasoning: {}", format_tokens(tokens.reasoning_tokens),);
                }
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

fn truncate(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_string();
    }
    value.chars().take(max).collect::<String>()
}

/// Format peak context for human display.
fn format_peak_context(peak: u64) -> String {
    format!("{} peak", format_tokens(peak as i64))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fact(
        fact_type: FactType,
        subject: Option<&str>,
        detail: Option<&str>,
        exit_code: Option<i32>,
    ) -> Fact {
        Fact {
            id: None,
            session_id: "sess".to_string(),
            ts: "2026-03-07T10:00:00Z".to_string(),
            turn_number: Some(1),
            fact_type,
            subject: subject.map(str::to_string),
            detail: detail.map(str::to_string),
            exit_code,
            success: Some(exit_code.unwrap_or(0) == 0),
        }
    }

    #[test]
    fn collect_errors_deduplicates_matching_command_and_error_facts() {
        let long_cmd = "sqlite3 ~/.gaal/index.db \"select id, jsonl_path from sessions where engine='claude' and total_tools=0 order by started_at desc limit 30;\"";
        let truncated = truncate(long_cmd, 100);
        let facts = vec![
            fact(FactType::Command, Some(&truncated), Some(long_cmd), Some(5)),
            fact(
                FactType::Error,
                Some(&truncated),
                Some(&format!(
                    "Command: /bin/bash -lc \"{}\"\nChunk ID: x\nProcess exited with code 5\nOutput:\nError",
                    long_cmd.replace('"', "\\\"")
                )),
                Some(5),
            ),
        ];

        let errors = collect_errors(&facts);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].exit_code, 5);
    }

    #[test]
    fn subagent_meaningful_content_requires_turns_and_activity() {
        let empty = SessionRow {
            id: "child".to_string(),
            engine: "claude".to_string(),
            model: None,
            cwd: None,
            started_at: "2026-03-07T10:00:00Z".to_string(),
            ended_at: None,
            exit_signal: None,
            last_event_at: None,
            parent_id: Some("parent".to_string()),
            session_type: "subagent".to_string(),
            jsonl_path: "/tmp/child.jsonl".to_string(),
            total_input_tokens: 0,
            total_output_tokens: 0,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
            reasoning_tokens: 0,
            total_tools: 0,
            total_turns: 1,
            peak_context: 0,
            last_indexed_offset: 0,
            subagent_type: None,
        };

        assert!(!subagent_has_meaningful_content(&empty));

        let mut with_tokens = empty.clone();
        with_tokens.total_output_tokens = 12;
        assert!(subagent_has_meaningful_content(&with_tokens));

        let mut with_tools = empty.clone();
        with_tools.total_tools = 1;
        assert!(subagent_has_meaningful_content(&with_tools));

        let mut zero_turns = with_tools.clone();
        zero_turns.total_turns = 0;
        assert!(!subagent_has_meaningful_content(&zero_turns));
    }
}
