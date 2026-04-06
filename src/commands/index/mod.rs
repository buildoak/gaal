//! `gaal index` — build, manage, and inspect the session index.

mod recover_orphans;

pub use recover_orphans::{run_recover_orphans, RecoverOrphansArgs};

use std::collections::HashSet;
use std::fs;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use rusqlite::{named_params, Connection};
use serde::Serialize;
use serde_json::{json, Value};

use crate::commands::search;
use crate::config::{gaal_home, load_config};
use crate::db::open_db;
use crate::db::queries::{
    delete_session, get_handoff, get_index_status, get_session, insert_facts_batch, upsert_handoff,
    upsert_session, SessionRow,
};
use crate::discovery::codex::truncate_codex_id;
use crate::discovery::{discover_sessions, DiscoveredSession};
use crate::error::GaalError;
use crate::model::{Fact, HandoffRecord};
use crate::output::json::print_json;
use crate::parser::types::Engine;
use crate::parser::{parse_session, parse_session_incremental, ParsedSession};
use crate::subagent::engine::get_subagent_summaries;

pub(super) const EPOCH_RFC3339: &str = "1970-01-01T00:00:00Z";
const SUSPICIOUS_PEAK_CONTEXT_THRESHOLD: i64 = 10_000_000;

/// Arguments for `gaal index backfill`.
#[derive(Debug, Clone)]
pub struct BackfillArgs {
    /// Optional engine filter (`claude` or `codex`).
    pub engine: Option<String>,
    /// Optional lower date/timestamp bound.
    pub since: Option<String>,
    /// Re-index even when the file size has not changed.
    pub force: bool,
    /// Also generate session markdown files during backfill.
    pub with_markdown: bool,
    /// Write session markdown files to this directory instead of the default gaal data dir.
    /// Layout: `<output_dir>/YYYY/MM/DD/<short-id>.md`.
    pub output_dir: Option<PathBuf>,
}

/// Arguments for `gaal index reindex`.
#[derive(Debug, Clone)]
pub struct ReindexArgs {
    /// Session id.
    pub id: String,
}

/// Arguments for `gaal index import-eywa`.
#[derive(Debug, Clone)]
pub struct ImportEywaArgs {
    /// Optional path to `handoff-index.json`.
    pub path: Option<String>,
}

/// Arguments for `gaal index prune`.
#[derive(Debug, Clone)]
pub struct PruneArgs {
    /// Delete facts older than this timestamp.
    pub before: String,
}

#[derive(Debug, Serialize)]
struct BackfillSummary {
    indexed: usize,
    skipped: usize,
    errors: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    markdown_written: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    markdown_skipped: Option<usize>,
}

#[derive(Debug, Serialize)]
struct ReindexSummary {
    session_id: String,
    facts: usize,
}

#[derive(Debug, Serialize)]
struct ImportEywaSummary {
    imported: usize,
    skipped: usize,
    errors: usize,
}

#[derive(Debug)]
pub(crate) enum IndexOutcome {
    Indexed,
    Skipped,
}

#[derive(Debug, Clone)]
struct EywaEntry {
    session_id: String,
    engine: Option<String>,
    model: Option<String>,
    cwd: Option<String>,
    started_at: Option<String>,
    headline: Option<String>,
    projects: Vec<String>,
    keywords: Vec<String>,
    substance: i32,
    duration_minutes: i32,
    generated_at: Option<String>,
    generated_by: Option<String>,
    content_path: Option<String>,
}

/// Run `gaal index backfill`.
pub fn run_backfill(args: BackfillArgs) -> Result<(), GaalError> {
    // Resolve output_dir: CLI arg > config default > None.
    let output_dir = args
        .output_dir
        .or_else(|| load_config().markdown_output_dir);

    // --output-dir (or config default) implies --with-markdown.
    let with_markdown = args.with_markdown || output_dir.is_some();

    let mut conn = open_db()?;
    let engine_filter = parse_engine_filter(args.engine.as_deref())?;
    let mut sessions = discover_sessions(engine_filter).map_err(GaalError::from)?;
    if let Some(since) = args.since.as_deref() {
        sessions.retain(|session| session_on_or_after(session, since));
    }

    let mut summary = BackfillSummary {
        indexed: 0,
        skipped: 0,
        errors: 0,
        markdown_written: if output_dir.is_some() { Some(0) } else { None },
        markdown_skipped: if output_dir.is_some() { Some(0) } else { None },
    };
    let total = sessions.len();

    // Batch-load all session IDs with invalid codex error rows once, instead of
    // querying per-session (which hit the wrong index and took ~60s over 2982 Codex sessions).
    let invalid_codex_error_sessions = load_codex_invalid_error_sessions(&conn)?;

    for (idx, session) in sessions.into_iter().enumerate() {
        match index_discovered_session(
            &mut conn,
            &session,
            args.force,
            &invalid_codex_error_sessions,
        ) {
            Ok(IndexOutcome::Indexed) => {
                summary.indexed += 1;
                eprintln!(
                    "[{}/{}] indexed {} ({})",
                    idx + 1,
                    total,
                    session.id,
                    session.path.display()
                );
                if with_markdown {
                    if let Some(output_dir) = &output_dir {
                        match write_session_markdown_to_dir(&conn, &session, output_dir, true) {
                            Ok(WriteOutcome::Written(md_path)) => {
                                *summary.markdown_written.as_mut().unwrap() += 1;
                                eprintln!("  -> markdown: {}", md_path.display());
                            }
                            Ok(WriteOutcome::Skipped) => {
                                *summary.markdown_skipped.as_mut().unwrap() += 1;
                            }
                            Err(err) => {
                                eprintln!("  -> markdown error: {err}");
                            }
                        }
                    } else {
                        match generate_session_markdown(&conn, &session) {
                            Ok(md_path) => {
                                eprintln!("  -> markdown: {}", md_path.display());
                            }
                            Err(err) => {
                                eprintln!("  -> markdown error: {err}");
                            }
                        }
                    }
                }
            }
            Ok(IndexOutcome::Skipped) => {
                summary.skipped += 1;
                eprintln!("[{}/{}] skipped {}", idx + 1, total, session.id);
                // Even for index-skipped sessions, write markdown if output-dir
                // is set and the file doesn't exist yet.
                if let Some(output_dir) = &output_dir {
                    match write_session_markdown_to_dir(&conn, &session, output_dir, false) {
                        Ok(WriteOutcome::Written(md_path)) => {
                            *summary.markdown_written.as_mut().unwrap() += 1;
                            eprintln!("  -> markdown: {}", md_path.display());
                        }
                        Ok(WriteOutcome::Skipped) => {
                            *summary.markdown_skipped.as_mut().unwrap() += 1;
                        }
                        Err(err) => {
                            eprintln!("  -> markdown error: {err}");
                        }
                    }
                } else if with_markdown {
                    // No output-dir: generate markdown to default gaal data dir
                    // if the file doesn't already exist.
                    let md_path = default_session_markdown_path(&session);
                    if !md_path.exists() {
                        match generate_session_markdown(&conn, &session) {
                            Ok(md_path) => {
                                eprintln!("  -> markdown: {}", md_path.display());
                            }
                            Err(err) => {
                                eprintln!("  -> markdown error: {err}");
                            }
                        }
                    }
                }
            }
            Err(err) => {
                summary.errors += 1;
                eprintln!(
                    "[{}/{}] error {} ({}): {}",
                    idx + 1,
                    total,
                    session.id,
                    session.path.display(),
                    err
                );
            }
        }
    }

    promote_codex_coordinators(&mut conn)?;
    if summary.indexed > 0 {
        search::build_search_index(&conn)?;
    }

    if let Some(output_dir) = &output_dir {
        let written = summary.markdown_written.unwrap_or(0);
        if written > 0 {
            eprintln!(
                "Wrote {} new session markdowns to {}",
                written,
                output_dir.display()
            );
        } else {
            eprintln!("No new sessions to process");
        }
    }

    print_json(&summary).map_err(GaalError::from)
}

/// Run `gaal index status`.
pub fn run_status() -> Result<(), GaalError> {
    let conn = open_db()?;
    let status = get_index_status(&conn)?;
    let payload = json!({
        "db_path": crate::db::db_path().to_string_lossy().to_string(),
        "db_size_bytes": status.db_size_bytes,
        "sessions_total": status.sessions_total,
        "sessions_by_engine": status.sessions_by_engine,
        "facts_total": status.facts_total,
        "handoffs_total": status.handoffs_total,
        "last_indexed_at": status.last_indexed_at,
        "oldest_session": status.oldest_session,
        "newest_session": status.newest_session
    });
    print_json(&payload).map_err(GaalError::from)
}

/// Run `gaal index reindex`.
pub fn run_reindex(args: ReindexArgs) -> Result<(), GaalError> {
    let conn = open_db()?;
    let existing = get_session(&conn, &args.id)?.ok_or_else(|| GaalError::NotFound(args.id))?;
    let path = PathBuf::from(&existing.jsonl_path);
    if !path.exists() {
        return Err(GaalError::NotFound(existing.jsonl_path));
    }

    let parsed = parse_session(&path).map_err(GaalError::from)?;
    let offset = file_len_i64(&path)?;
    let mut row = build_full_session_row(&parsed, &path, offset);
    row.id = existing.id.clone();
    row.session_type = existing.session_type.clone();
    let facts = normalize_facts(parsed.facts, &existing.id);

    conn.execute(
        "DELETE FROM facts WHERE session_id = :session_id",
        named_params! { ":session_id": &existing.id },
    )
    .map_err(GaalError::from)?;

    upsert_session(&conn, &row)?;
    if !facts.is_empty() {
        insert_facts_batch(&conn, &facts)?;
    }
    search::build_search_index(&conn)?;

    let payload = ReindexSummary {
        session_id: existing.id,
        facts: facts.len(),
    };
    print_json(&payload).map_err(GaalError::from)
}

/// Run `gaal index import-eywa`.
pub fn run_import_eywa(args: ImportEywaArgs) -> Result<(), GaalError> {
    let conn = open_db()?;
    let path = args
        .path
        .map(PathBuf::from)
        .unwrap_or_else(default_eywa_index_path);
    let raw = fs::read_to_string(&path).map_err(GaalError::from)?;
    let root: Value = serde_json::from_str(&raw)
        .map_err(|e| GaalError::ParseError(format!("invalid eywa index JSON: {e}")))?;
    let entries = parse_eywa_entries(&root)?;

    let mut summary = ImportEywaSummary {
        imported: 0,
        skipped: 0,
        errors: 0,
    };

    for entry in entries {
        match import_eywa_entry(&conn, &entry) {
            Ok(true) => summary.imported += 1,
            Ok(false) => summary.skipped += 1,
            Err(err) => {
                summary.errors += 1;
                eprintln!("failed importing eywa record {}: {}", entry.session_id, err);
            }
        }
    }

    print_json(&summary).map_err(GaalError::from)
}

/// Run `gaal index prune`.
pub fn run_prune(args: PruneArgs) -> Result<(), GaalError> {
    let conn = open_db()?;
    let deleted = conn
        .execute(
            "DELETE FROM facts WHERE ts < :before",
            named_params! { ":before": &args.before },
        )
        .map_err(GaalError::from)?;
    search::build_search_index(&conn)?;

    let payload = json!({
        "before": args.before,
        "deleted": deleted
    });
    print_json(&payload).map_err(GaalError::from)
}

pub(crate) fn index_discovered_session(
    conn: &mut rusqlite::Connection,
    discovered: &DiscoveredSession,
    force: bool,
    invalid_codex_error_sessions: &HashSet<String>,
) -> Result<IndexOutcome, GaalError> {
    let existing = get_session(conn, &discovered.id)?;
    let file_size_i64 = u64_to_i64(discovered.file_size)?;
    let existing_peak_context_suspicious = existing
        .as_ref()
        .map(|row| row.peak_context > SUSPICIOUS_PEAK_CONTEXT_THRESHOLD)
        .unwrap_or(false);
    let existing_needs_full_reparse = existing
        .as_ref()
        .map(|row| session_needs_full_reparse(discovered, row, invalid_codex_error_sessions))
        .transpose()?
        .unwrap_or(false);

    if let Some(row) = existing.as_ref() {
        if !force
            && !existing_peak_context_suspicious
            && !existing_needs_full_reparse
            && row.last_indexed_offset == file_size_i64
        {
            return Ok(IndexOutcome::Skipped);
        }
    }

    let should_incremental = existing
        .as_ref()
        .map(|row| {
            !force
                && !existing_needs_full_reparse
                && row.peak_context <= SUSPICIOUS_PEAK_CONTEXT_THRESHOLD
                && row.last_indexed_offset >= 0
                && (row.last_indexed_offset as u64) < discovered.file_size
                && discovered.engine != Engine::Gemini
        })
        .unwrap_or(false);

    if should_incremental {
        let existing_row = existing.ok_or_else(|| {
            GaalError::Internal("missing existing row for incremental parse".to_string())
        })?;
        let offset = u64::try_from(existing_row.last_indexed_offset).map_err(|_| {
            GaalError::Internal("negative last_indexed_offset for incremental parse".to_string())
        })?;
        let (parsed_delta, new_offset) =
            parse_session_incremental(&discovered.path, offset).map_err(GaalError::from)?;
        let merged_row = build_incremental_session_row(
            &existing_row,
            &parsed_delta,
            &discovered.path,
            new_offset,
        )?;
        let mut merged_row = merged_row;
        apply_codex_subagent_link(
            &mut merged_row,
            discovered,
            parsed_delta
                .meta
                .agent_role
                .clone()
                .or_else(|| existing_row.subagent_type.clone()),
        );
        let normalized_facts = normalize_facts(parsed_delta.facts, &existing_row.id);

        // Wrap upsert + facts + links in a single savepoint to reduce lock
        // acquisition cycles under parallel load.  Savepoints nest safely,
        // unlike unchecked_transaction() which crashes with "nested transaction"
        // when init_db leaves a phantom transaction open (I16/I17).
        let tx = conn
            .savepoint_with_name("index_session")
            .map_err(GaalError::from)?;
        upsert_session(&tx, &merged_row)?;
        if !normalized_facts.is_empty() {
            insert_facts_batch(&tx, &normalized_facts)?;
        }
        tx.commit().map_err(GaalError::from)?;
        if discovered.engine == Engine::Claude {
            let parent_id = merged_row.id.clone();
            match index_subagents(conn, &discovered.path, &parent_id) {
                Ok(count) => {
                    if count > 0 {
                        eprintln!("  -> indexed {} subagents", count);
                    }
                }
                Err(err) => {
                    eprintln!("  -> subagent indexing warning: {}", err);
                }
            }
        }
        return Ok(IndexOutcome::Indexed);
    }

    let parsed = parse_session(&discovered.path).map_err(GaalError::from)?;

    // Skip noise-only sessions (0 conversation turns, e.g. file-history-snapshot only).
    if parsed.total_turns == 0 {
        if let Some(row) = existing.as_ref() {
            // Prune stale zero-turn sessions from the DB on re-index.
            if row.total_turns == 0 {
                delete_session(conn, &row.id)?;
            }
        }
        return Ok(IndexOutcome::Skipped);
    }

    let target_id = existing
        .as_ref()
        .map(|row| row.id.as_str())
        .unwrap_or(&discovered.id);
    let mut session_row = build_full_session_row(&parsed, &discovered.path, file_size_i64);
    session_row.id = target_id.to_string();
    if let Some(row) = existing.as_ref() {
        session_row.session_type = row.session_type.clone();
    }
    apply_codex_subagent_link(&mut session_row, discovered, parsed.meta.agent_role.clone());
    let facts = normalize_facts(parsed.facts, target_id);

    // Wrap delete-old-facts + upsert + insert-facts + links in a single
    // savepoint to reduce lock acquisition cycles under parallel load.
    // Savepoints nest safely, unlike unchecked_transaction() which crashes
    // with "nested transaction" when init_db leaves a phantom transaction
    // open (I16/I17).
    let tx = conn
        .savepoint_with_name("index_full")
        .map_err(GaalError::from)?;
    if let Some(row) = existing.as_ref() {
        tx.execute(
            "DELETE FROM facts WHERE session_id = :session_id",
            named_params! { ":session_id": &row.id },
        )
        .map_err(GaalError::from)?;
    }

    upsert_session(&tx, &session_row)?;
    if !facts.is_empty() {
        insert_facts_batch(&tx, &facts)?;
    }
    tx.commit().map_err(GaalError::from)?;
    if discovered.engine == Engine::Claude {
        let parent_id = session_row.id.clone();
        match index_subagents(conn, &discovered.path, &parent_id) {
            Ok(count) => {
                if count > 0 {
                    eprintln!("  -> indexed {} subagents", count);
                }
            }
            Err(err) => {
                eprintln!("  -> subagent indexing warning: {}", err);
            }
        }
    }
    Ok(IndexOutcome::Indexed)
}

fn index_subagents(
    conn: &mut rusqlite::Connection,
    parent_jsonl_path: &Path,
    parent_session_id: &str,
) -> Result<usize, GaalError> {
    let parent_row = get_session(conn, parent_session_id)?.ok_or_else(|| {
        GaalError::Internal(format!(
            "parent session missing during subagent indexing: {parent_session_id}"
        ))
    })?;
    let session_dir = parent_jsonl_path.with_extension("");
    let summaries = get_subagent_summaries(parent_jsonl_path, &session_dir)
        .map_err(|e| GaalError::Internal(format!("discover subagents: {e}")))?;
    if summaries.is_empty() {
        return Ok(0);
    }

    let mut indexed = 0usize;

    for summary in summaries {
        if !summary.has_jsonl {
            continue;
        }

        let Some(jsonl_path) = summary.jsonl_path.as_ref() else {
            continue;
        };

        let child_id =
            match resolve_subagent_session_id(conn, &summary.meta.agent_id, parent_session_id)? {
                Some(id) => id,
                None => {
                    eprintln!(
                        "  -> subagent indexing warning: id collision for agent {} under parent {}",
                        summary.meta.agent_id, parent_session_id
                    );
                    continue;
                }
            };

        let parsed = match parse_session(jsonl_path) {
            Ok(parsed) => parsed,
            Err(err) => {
                eprintln!(
                    "  -> subagent indexing warning: failed to parse {}: {}",
                    jsonl_path.display(),
                    err
                );
                continue;
            }
        };
        let last_indexed_offset = match file_len_i64(jsonl_path) {
            Ok(len) => len,
            Err(err) => {
                eprintln!(
                    "  -> subagent indexing warning: failed to stat {}: {}",
                    jsonl_path.display(),
                    err
                );
                continue;
            }
        };
        let started_at = if parsed.meta.started_at == EPOCH_RFC3339 {
            parent_row.started_at.clone()
        } else {
            parsed.meta.started_at.clone()
        };

        let child_facts = normalize_facts(parsed.facts, &child_id);
        let child_row = SessionRow {
            id: child_id.clone(),
            engine: "claude".to_string(),
            model: parsed.meta.model.clone(),
            cwd: parsed.meta.cwd.clone().or_else(|| parent_row.cwd.clone()),
            started_at,
            ended_at: parsed.ended_at.clone(),
            exit_signal: parsed.exit_signal.clone(),
            last_event_at: parsed.last_event_at.clone(),
            parent_id: Some(parent_session_id.to_string()),
            session_type: "subagent".to_string(),
            jsonl_path: jsonl_path.to_string_lossy().to_string(),
            total_input_tokens: parsed.total_input_tokens,
            total_output_tokens: parsed.total_output_tokens,
            cache_read_tokens: parsed.cache_read_tokens,
            cache_creation_tokens: parsed.cache_creation_tokens,
            reasoning_tokens: parsed.reasoning_tokens,
            total_tools: i64::from(parsed.total_tools),
            total_turns: i64::from(parsed.total_turns),
            peak_context: parsed.peak_context,
            last_indexed_offset,
            subagent_type: summary.meta.subagent_type.clone(),
        };

        let tx = match conn.savepoint_with_name("index_subagent") {
            Ok(tx) => tx,
            Err(err) => {
                eprintln!(
                    "  -> subagent indexing warning: savepoint failed for {}: {}",
                    child_id, err
                );
                continue;
            }
        };

        let subagent_type_for_tag = summary.meta.subagent_type.clone();
        let save_result: Result<(), GaalError> = (|| {
            tx.execute(
                "DELETE FROM facts WHERE session_id = :session_id",
                named_params! { ":session_id": &child_id },
            )
            .map_err(GaalError::from)?;
            upsert_session(&tx, &child_row)?;
            if !child_facts.is_empty() {
                insert_facts_batch(&tx, &child_facts)?;
            }
            // P2: Auto-tag subagent_type so `gaal ls --tag gsd-heavy` works.
            if let Some(ref st) = subagent_type_for_tag {
                if !st.is_empty() {
                    crate::db::queries::add_tag(&tx, &child_id, st)?;
                }
            }
            tx.commit().map_err(GaalError::from)?;
            Ok(())
        })();

        match save_result {
            Ok(()) => indexed += 1,
            Err(err) => {
                eprintln!(
                    "  -> subagent indexing warning: failed to save {}: {}",
                    child_id, err
                );
            }
        }
    }

    if indexed > 0 {
        conn.execute(
            "UPDATE sessions SET session_type = 'coordinator' WHERE id = :id",
            named_params! { ":id": parent_session_id },
        )
        .map_err(GaalError::from)?;
    }

    Ok(indexed)
}

pub(super) fn resolve_subagent_session_id(
    conn: &rusqlite::Connection,
    agent_id: &str,
    parent_session_id: &str,
) -> Result<Option<String>, GaalError> {
    for prefix_len in [8usize, 12usize] {
        let candidate: String = agent_id.chars().take(prefix_len).collect();
        if candidate.is_empty() {
            return Ok(None);
        }

        match get_session(conn, &candidate)? {
            None => return Ok(Some(candidate)),
            Some(existing) if existing.parent_id.as_deref() == Some(parent_session_id) => {
                return Ok(Some(candidate));
            }
            // Orphaned subagent row (parent_id = NULL) — claim it for this parent
            // instead of falling through to create a 12-char duplicate.
            // The subsequent upsert_session will set the correct parent_id.
            Some(existing)
                if existing.parent_id.is_none()
                    && existing.session_type == "subagent"
                    && agent_id.starts_with(&candidate) =>
            {
                return Ok(Some(candidate));
            }
            Some(_) => continue,
        }
    }

    Ok(None)
}

fn session_needs_full_reparse(
    discovered: &DiscoveredSession,
    row: &SessionRow,
    invalid_codex_error_sessions: &HashSet<String>,
) -> Result<bool, GaalError> {
    if discovered.engine == Engine::Claude
        && row.total_tools == 0
        && claude_jsonl_contains_inline_tool_use(&discovered.path)?
    {
        return Ok(true);
    }

    let has_invalid_codex_error_rows =
        discovered.engine == Engine::Codex && invalid_codex_error_sessions.contains(&row.id);

    Ok(has_invalid_codex_error_rows)
}

fn claude_jsonl_contains_inline_tool_use(path: &Path) -> Result<bool, GaalError> {
    let file = File::open(path).map_err(GaalError::Io)?;
    let reader = BufReader::new(file);

    for line_result in reader.lines() {
        let line = line_result.map_err(GaalError::Io)?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let Ok(record) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };
        if record.get("type").and_then(Value::as_str) != Some("assistant") {
            continue;
        }

        let Some(items) = record.pointer("/message/content").and_then(Value::as_array) else {
            continue;
        };
        if items
            .iter()
            .any(|item| item.get("type").and_then(Value::as_str) == Some("tool_use"))
        {
            return Ok(true);
        }
    }

    Ok(false)
}

/// Batch-load ALL session IDs that have invalid codex error rows (fact_type='error', exit_code=0).
/// Called once before the backfill loop to avoid per-session queries that hit the wrong index.
fn load_codex_invalid_error_sessions(
    conn: &rusqlite::Connection,
) -> Result<HashSet<String>, GaalError> {
    let mut stmt = conn
        .prepare(
            "SELECT DISTINCT session_id FROM facts WHERE fact_type = 'error' AND exit_code = 0",
        )
        .map_err(GaalError::from)?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(GaalError::from)?;
    let mut set = HashSet::new();
    for row in rows {
        set.insert(row.map_err(GaalError::from)?);
    }
    Ok(set)
}

fn build_full_session_row(
    parsed: &ParsedSession,
    path: &Path,
    last_indexed_offset: i64,
) -> SessionRow {
    SessionRow {
        id: parsed.meta.id.clone(),
        engine: parsed.meta.engine.to_string(),
        model: parsed.meta.model.clone(),
        cwd: parsed.meta.cwd.clone(),
        started_at: parsed.meta.started_at.clone(),
        ended_at: parsed.ended_at.clone(),
        exit_signal: parsed.exit_signal.clone(),
        last_event_at: parsed.last_event_at.clone(),
        parent_id: None,
        session_type: "standalone".to_string(),
        jsonl_path: path.to_string_lossy().to_string(),
        total_input_tokens: parsed.total_input_tokens,
        total_output_tokens: parsed.total_output_tokens,
        cache_read_tokens: parsed.cache_read_tokens,
        cache_creation_tokens: parsed.cache_creation_tokens,
        reasoning_tokens: parsed.reasoning_tokens,
        total_tools: i64::from(parsed.total_tools),
        total_turns: i64::from(parsed.total_turns),
        peak_context: parsed.peak_context,
        last_indexed_offset,
        subagent_type: None, // standalone sessions don't have a subagent_type
    }
}

fn build_incremental_session_row(
    existing: &SessionRow,
    parsed_delta: &ParsedSession,
    path: &Path,
    new_offset: u64,
) -> Result<SessionRow, GaalError> {
    Ok(SessionRow {
        id: existing.id.clone(),
        engine: existing.engine.clone(),
        model: parsed_delta
            .meta
            .model
            .clone()
            .or_else(|| existing.model.clone()),
        cwd: parsed_delta
            .meta
            .cwd
            .clone()
            .or_else(|| existing.cwd.clone()),
        started_at: existing.started_at.clone(),
        ended_at: parsed_delta
            .ended_at
            .clone()
            .or_else(|| existing.ended_at.clone()),
        exit_signal: parsed_delta
            .exit_signal
            .clone()
            .or_else(|| existing.exit_signal.clone()),
        last_event_at: parsed_delta
            .last_event_at
            .clone()
            .or_else(|| existing.last_event_at.clone()),
        parent_id: existing.parent_id.clone(),
        session_type: existing.session_type.clone(),
        jsonl_path: path.to_string_lossy().to_string(),
        total_input_tokens: existing.total_input_tokens + parsed_delta.total_input_tokens,
        total_output_tokens: existing.total_output_tokens + parsed_delta.total_output_tokens,
        cache_read_tokens: existing.cache_read_tokens + parsed_delta.cache_read_tokens,
        cache_creation_tokens: existing.cache_creation_tokens + parsed_delta.cache_creation_tokens,
        reasoning_tokens: existing.reasoning_tokens + parsed_delta.reasoning_tokens,
        total_tools: existing.total_tools + i64::from(parsed_delta.total_tools),
        total_turns: existing.total_turns + i64::from(parsed_delta.total_turns),
        peak_context: existing.peak_context.max(parsed_delta.peak_context),
        last_indexed_offset: u64_to_i64(new_offset)?,
        subagent_type: existing.subagent_type.clone(),
    })
}

fn apply_codex_subagent_link(
    session_row: &mut SessionRow,
    discovered: &DiscoveredSession,
    subagent_type: Option<String>,
) {
    let Some(forked_from_id) = discovered.forked_from_id.as_deref() else {
        return;
    };

    session_row.session_type = "subagent".to_string();
    session_row.parent_id = Some(truncate_codex_id(forked_from_id));
    session_row.subagent_type = subagent_type;
}

fn promote_codex_coordinators(conn: &mut Connection) -> Result<(), GaalError> {
    conn.execute(
        r#"
        UPDATE sessions SET session_type = 'coordinator'
        WHERE engine = 'codex'
        AND session_type = 'standalone'
        AND id IN (
            SELECT DISTINCT parent_id FROM sessions
            WHERE engine = 'codex' AND session_type = 'subagent' AND parent_id IS NOT NULL
        )
        "#,
        [],
    )
    .map_err(GaalError::from)?;
    Ok(())
}

/// Compute the default markdown path for a session without writing anything.
///
/// Returns `~/.gaal/data/{engine}/sessions/YYYY/MM/DD/{id}.md`.
fn default_session_markdown_path(discovered: &DiscoveredSession) -> PathBuf {
    let engine = discovered.engine.to_string();
    let started_at = discovered
        .started_at
        .as_deref()
        .unwrap_or("1970-01-01T00:00:00Z");
    let (year, month, day) = extract_date_parts(started_at);

    gaal_home()
        .join("data")
        .join(engine)
        .join("sessions")
        .join(year)
        .join(month)
        .join(day)
        .join(format!(
            "{}.md",
            crate::util::sanitize_filename(&discovered.id)
                .chars()
                .take(8)
                .collect::<String>()
        ))
}

/// Generate a session markdown file from raw JSONL during backfill.
///
/// Writes the rendered markdown to `~/.gaal/data/{engine}/sessions/YYYY/MM/DD/{id}.md`.
fn generate_session_markdown(
    conn: &Connection,
    discovered: &DiscoveredSession,
) -> Result<PathBuf, GaalError> {
    let started_at = discovered.started_at.as_deref().unwrap_or(EPOCH_RFC3339);

    // Don't create markdown for sessions with no valid timestamp (epoch fallback).
    if started_at == EPOCH_RFC3339 {
        return Err(GaalError::Internal(
            "skipping markdown for epoch-timestamp session".to_string(),
        ));
    }

    let markdown = crate::render::session_md::render_session_markdown_with_db(
        &discovered.path,
        conn,
        Some(&discovered.id),
    )
    .map_err(|e| GaalError::Internal(format!("render session markdown: {e}")))?;

    let engine = discovered.engine.to_string();
    let (year, month, day) = extract_date_parts(started_at);

    let md_path = gaal_home()
        .join("data")
        .join(engine)
        .join("sessions")
        .join(year)
        .join(month)
        .join(day)
        .join(format!(
            "{}.md",
            crate::util::sanitize_filename(&discovered.id)
                .chars()
                .take(8)
                .collect::<String>()
        ));

    if let Some(parent) = md_path.parent() {
        fs::create_dir_all(parent).map_err(GaalError::from)?;
    }
    crate::util::atomic_write(&md_path, &markdown).map_err(GaalError::from)?;
    Ok(md_path)
}

#[derive(Debug)]
enum WriteOutcome {
    Written(PathBuf),
    Skipped,
}

/// Write a session's markdown to `<output_dir>/YYYY/MM/DD/<short-id>.md`.
///
/// When `overwrite` is false, skips if the target file already exists (idempotent).
/// When `overwrite` is true, always re-renders (used when the session was re-indexed
/// because new data arrived, e.g. an active session).
/// Uses atomic writes to avoid partial files.
fn write_session_markdown_to_dir(
    conn: &Connection,
    discovered: &DiscoveredSession,
    output_dir: &Path,
    overwrite: bool,
) -> Result<WriteOutcome, GaalError> {
    let started_at = discovered.started_at.as_deref().unwrap_or(EPOCH_RFC3339);

    // Don't create markdown for sessions with no valid timestamp (epoch fallback).
    if started_at == EPOCH_RFC3339 {
        return Ok(WriteOutcome::Skipped);
    }
    let (year, month, day) = extract_date_parts(started_at);
    let short_id = &crate::util::sanitize_filename(&discovered.id)
        .chars()
        .take(8)
        .collect::<String>();

    let md_path = output_dir
        .join(&year)
        .join(&month)
        .join(&day)
        .join(format!("{short_id}.md"));

    // Idempotent: skip if already written (unless overwrite requested).
    if !overwrite && md_path.exists() {
        return Ok(WriteOutcome::Skipped);
    }

    let markdown = crate::render::session_md::render_session_markdown_with_db(
        &discovered.path,
        conn,
        Some(&discovered.id),
    )
    .map_err(|e| GaalError::Internal(format!("render session markdown: {e}")))?;

    if let Some(parent) = md_path.parent() {
        fs::create_dir_all(parent).map_err(GaalError::from)?;
    }
    crate::util::atomic_write(&md_path, &markdown).map_err(GaalError::from)?;
    Ok(WriteOutcome::Written(md_path))
}

/// Extract (year, month, day) from an RFC3339 timestamp prefix.
fn extract_date_parts(started_at: &str) -> (String, String, String) {
    let prefix = started_at.get(0..10).unwrap_or("1970-01-01");
    let mut parts = prefix.split('-');
    let year = parts.next().unwrap_or("1970").to_string();
    let month = parts.next().unwrap_or("01").to_string();
    let day = parts.next().unwrap_or("01").to_string();
    (year, month, day)
}

pub(super) fn normalize_facts(mut facts: Vec<Fact>, session_id: &str) -> Vec<Fact> {
    for fact in &mut facts {
        fact.session_id = session_id.to_string();
    }
    facts
}

fn parse_engine_filter(engine: Option<&str>) -> Result<Option<Engine>, GaalError> {
    let Some(raw) = engine else {
        return Ok(None);
    };
    let normalized = raw.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return Ok(None);
    }
    Engine::from_str(&normalized).map(Some)
}

fn session_on_or_after(session: &DiscoveredSession, since: &str) -> bool {
    match session.started_at.as_deref() {
        Some(started_at) => started_at >= since,
        None => true,
    }
}

pub(super) fn file_len_i64(path: &Path) -> Result<i64, GaalError> {
    let len = fs::metadata(path).map_err(GaalError::from)?.len();
    u64_to_i64(len)
}

fn u64_to_i64(value: u64) -> Result<i64, GaalError> {
    i64::try_from(value)
        .map_err(|_| GaalError::Internal("value exceeds i64 range for SQLite integer".to_string()))
}

fn default_eywa_index_path() -> PathBuf {
    gaal_home()
        .join("data")
        .join("eywa")
        .join("handoff-index.json")
}

fn import_eywa_entry(conn: &rusqlite::Connection, entry: &EywaEntry) -> Result<bool, GaalError> {
    if entry.session_id.trim().is_empty() {
        return Ok(false);
    }

    let existing = get_session(conn, &entry.session_id)?;
    if existing.is_none() {
        let stub = build_eywa_session_stub(entry);
        upsert_session(conn, &stub)?;
    }

    let had_handoff = get_handoff(conn, &entry.session_id)?.is_some();
    let content_path = resolve_eywa_content_path(entry);
    let handoff = HandoffRecord {
        session_id: entry.session_id.clone(),
        headline: entry.headline.clone(),
        projects: entry.projects.clone(),
        keywords: entry.keywords.clone(),
        substance: entry.substance,
        duration_minutes: entry.duration_minutes,
        generated_at: entry.generated_at.clone(),
        generated_by: entry.generated_by.clone(),
        content_path,
    };
    upsert_handoff(conn, &handoff)?;
    Ok(!had_handoff)
}

/// Resolve the content_path for an eywa import entry.
///
/// Resolution order:
/// 1. Use entry content_path when it points to an existing file.
/// 2. Expand and check `~/...` paths.
/// 3. Derive from date + session id under known handoff directories.
fn resolve_eywa_content_path(entry: &EywaEntry) -> Option<String> {
    if let Some(path_str) = entry.content_path.as_deref() {
        let trimmed = path_str.trim();
        if !trimmed.is_empty() && !trimmed.starts_with("eywa://") {
            let path = Path::new(trimmed);
            if path.exists() {
                return Some(trimmed.to_string());
            }

            if let Some(rest) = trimmed.strip_prefix("~/") {
                if let Some(home) = dirs::home_dir() {
                    let expanded = home.join(rest);
                    if expanded.exists() {
                        return Some(expanded.to_string_lossy().to_string());
                    }
                }
            }
        }
    }

    let date_str = entry
        .started_at
        .as_deref()
        .or(entry.generated_at.as_deref())?;
    let (year, month, day) = extract_date_parts(date_str);
    let filename = format!("{}.md", entry.session_id);
    let candidate_roots = [
        gaal_home().join("data").join("eywa").join("handoffs"),
        gaal_home().join("data").join("claude").join("handoffs"),
    ];

    for root in candidate_roots {
        let candidate = root.join(&year).join(&month).join(&day).join(&filename);
        if candidate.exists() {
            return Some(candidate.to_string_lossy().to_string());
        }
    }

    None
}

fn build_eywa_session_stub(entry: &EywaEntry) -> SessionRow {
    let started_at = entry
        .started_at
        .clone()
        .or_else(|| entry.generated_at.clone())
        .unwrap_or_else(|| EPOCH_RFC3339.to_string());

    SessionRow {
        id: entry.session_id.clone(),
        engine: normalize_engine_string(entry.engine.as_deref()),
        model: entry.model.clone(),
        cwd: entry.cwd.clone(),
        started_at: started_at.clone(),
        ended_at: Some(started_at.clone()),
        exit_signal: None,
        last_event_at: Some(started_at),
        parent_id: None,
        session_type: "standalone".to_string(),
        jsonl_path: entry
            .content_path
            .clone()
            .unwrap_or_else(|| format!("eywa://{}", entry.session_id)),
        total_input_tokens: 0,
        total_output_tokens: 0,
        cache_read_tokens: 0,
        cache_creation_tokens: 0,
        reasoning_tokens: 0,
        total_tools: 0,
        total_turns: 0,
        peak_context: 0,
        last_indexed_offset: 0,
        subagent_type: None,
    }
}

fn normalize_engine_string(value: Option<&str>) -> String {
    let candidate = value
        .map(str::trim)
        .unwrap_or_default()
        .to_ascii_lowercase();
    match candidate.as_str() {
        "claude" | "codex" => candidate,
        _ => "claude".to_string(),
    }
}

fn parse_eywa_entries(root: &Value) -> Result<Vec<EywaEntry>, GaalError> {
    if let Some(entries) = root.as_array() {
        return entries
            .iter()
            .map(value_to_eywa_entry)
            .collect::<Result<Vec<_>, _>>();
    }

    if let Some(entries) = root.get("entries").and_then(Value::as_array) {
        return entries
            .iter()
            .map(value_to_eywa_entry)
            .collect::<Result<Vec<_>, _>>();
    }

    if let Some(map) = root.get("handoffs").and_then(Value::as_object) {
        let mut out = Vec::with_capacity(map.len());
        for (session_id, payload) in map {
            let mut entry = value_to_eywa_entry(payload)?;
            if entry.session_id.is_empty() {
                entry.session_id = session_id.to_string();
            }
            out.push(entry);
        }
        return Ok(out);
    }

    if let Some(map) = root.as_object() {
        let mut out = Vec::with_capacity(map.len());
        for (session_id, payload) in map {
            let mut entry = value_to_eywa_entry(payload)?;
            if entry.session_id.is_empty() {
                entry.session_id = session_id.to_string();
            }
            out.push(entry);
        }
        return Ok(out);
    }

    Err(GaalError::ParseError(
        "eywa index must be a JSON array or object".to_string(),
    ))
}

fn value_to_eywa_entry(value: &Value) -> Result<EywaEntry, GaalError> {
    let obj = value
        .as_object()
        .ok_or_else(|| GaalError::ParseError("eywa entry must be an object".to_string()))?;

    let session_id = first_string(obj, &["session_id", "id", "session"]).unwrap_or_default();
    let projects = first_string_vec(obj, &["projects"]);
    let keywords = first_string_vec(obj, &["keywords", "tags"]);

    Ok(EywaEntry {
        session_id,
        engine: first_string(obj, &["engine"]),
        model: first_string(obj, &["model"]),
        cwd: first_string(obj, &["cwd"]),
        started_at: first_string(obj, &["started_at", "date", "started"]),
        headline: first_string(obj, &["headline", "summary", "title"]),
        projects,
        keywords,
        substance: first_i32(obj, &["substance"]).unwrap_or(0),
        duration_minutes: first_i32(obj, &["duration_minutes", "duration"]).unwrap_or(0),
        generated_at: first_string(obj, &["generated_at", "updated_at"]),
        generated_by: first_string(obj, &["generated_by"]),
        content_path: first_string(obj, &["content_path", "path", "handoff_path"]),
    })
}

fn first_string(obj: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<String> {
    for key in keys {
        let value = obj.get(*key);
        let Some(value) = value else {
            continue;
        };
        if let Some(text) = value.as_str() {
            return Some(text.to_string());
        }
        if value.is_number() || value.is_boolean() {
            return Some(value.to_string());
        }
    }
    None
}

fn first_i32(obj: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<i32> {
    for key in keys {
        let value = obj.get(*key);
        let Some(value) = value else {
            continue;
        };
        if let Some(v) = value.as_i64() {
            if let Ok(out) = i32::try_from(v) {
                return Some(out);
            }
        }
        if let Some(text) = value.as_str() {
            if let Ok(parsed) = text.parse::<i32>() {
                return Some(parsed);
            }
        }
    }
    None
}

fn first_string_vec(obj: &serde_json::Map<String, Value>, keys: &[&str]) -> Vec<String> {
    for key in keys {
        let Some(value) = obj.get(*key) else {
            continue;
        };
        if let Some(arr) = value.as_array() {
            let mut out = Vec::new();
            for item in arr {
                if let Some(text) = item.as_str() {
                    out.push(text.to_string());
                } else if item.is_number() || item.is_boolean() {
                    out.push(item.to_string());
                }
            }
            return out;
        }
        if let Some(text) = value.as_str() {
            let out = text
                .split(',')
                .map(str::trim)
                .filter(|part| !part.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>();
            if !out.is_empty() {
                return out;
            }
        }
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_path(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        std::env::temp_dir().join(format!("gaal-index-{unique}-{name}.jsonl"))
    }

    #[test]
    fn detects_inline_claude_tool_use_in_assistant_messages() {
        let path = temp_path("claude-tools");
        fs::write(
            &path,
            concat!(
                "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"Read\",\"input\":{\"file_path\":\"/tmp/a\"}}]}}\n",
                "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"done\"}]}}\n"
            ),
        )
        .unwrap();

        let contains_tools = claude_jsonl_contains_inline_tool_use(&path).unwrap();
        let _ = fs::remove_file(&path);

        assert!(contains_tools);
    }

    #[test]
    fn ignores_claude_assistant_messages_without_tool_use_blocks() {
        let path = temp_path("claude-no-tools");
        fs::write(
            &path,
            "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"hello\"}]}}\n",
        )
        .unwrap();

        let contains_tools = claude_jsonl_contains_inline_tool_use(&path).unwrap();
        let _ = fs::remove_file(&path);

        assert!(!contains_tools);
    }
}
