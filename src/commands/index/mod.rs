//! `gaal index` — build, manage, and inspect the session index.

mod recover_orphans;

pub use recover_orphans::{run_recover_orphans, RecoverOrphansArgs};

use std::collections::HashSet;
use std::fs;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{named_params, Connection};
use serde::Serialize;
use serde_json::{json, Value};

use crate::commands::search;
use crate::config::{gaal_home, load_config};
use crate::db::open_db;
use crate::db::queries::{
    delete_session, get_grok_diagnostic_summary, get_index_status, get_meta, get_session,
    insert_facts_batch, replace_grok_source_state, resolve_session_ids, set_meta, upsert_session,
    SessionRow,
};
use crate::discovery::codex::truncate_codex_id;
use crate::discovery::{discover_sessions_with_cutoff, DiscoveredSession};
use crate::error::GaalError;
use crate::model::Fact;
use crate::output::human::print_table;
use crate::output::json::print_json;
use crate::parser::types::Engine;
use crate::parser::{
    parse_discovered_session, parse_session, parse_session_incremental, ParsedSession,
};
use crate::subagent::engine::get_subagent_summaries;

/// Safety margin when computing the mtime cutoff for incremental discovery.
///
/// Actively-appending files have an mtime equal (within filesystem granularity)
/// to the last run's wall-clock time.  Subtracting a small window guarantees
/// those files are NOT gated out on the next pass.
const BACKFILL_CURSOR_SAFETY_MARGIN: Duration = Duration::from_secs(10);

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

#[derive(Debug)]
pub(crate) enum IndexOutcome {
    Indexed,
    Skipped,
}

/// Run `gaal index backfill`.
///
/// Strategy:
///   - One pass per engine (Claude, Codex, Gemini).
///   - Each engine keeps its own mtime cursor in the `meta` table
///     (`backfill:<engine>`).  On the next run, discovery skips files whose
///     on-disk mtime is older than the cursor minus a 10s safety margin.
///   - First run (no cursor) and DB wipes fall through to a full scan — the
///     cursor is absent, so `newer_than` is `None`.
///   - A per-engine failure leaves that engine's cursor untouched so the next
///     run retries the missed window; the other engines still advance.
///   - `--engine` filter continues to work — only the requested engine runs.
///   - `--since` filter is honored on top of the mtime gate (additive).
pub fn run_backfill(args: BackfillArgs) -> Result<(), GaalError> {
    // Resolve output_dir: CLI arg > config default > None.
    let output_dir = args
        .output_dir
        .or_else(|| load_config().markdown_output_dir);

    // --output-dir (or config default) implies --with-markdown.
    let with_markdown = args.with_markdown || output_dir.is_some();

    let mut conn = open_db()?;
    let engine_filter = parse_engine_filter(args.engine.as_deref())?;

    let mut summary = BackfillSummary {
        indexed: 0,
        skipped: 0,
        errors: 0,
        markdown_written: if output_dir.is_some() { Some(0) } else { None },
        markdown_skipped: if output_dir.is_some() { Some(0) } else { None },
    };

    // Batch-load all session IDs with invalid codex error rows once, instead of
    // querying per-session (which hit the wrong index and took ~60s over 2982 Codex sessions).
    let invalid_codex_error_sessions = load_codex_invalid_error_sessions(&conn)?;

    let run_start = SystemTime::now();
    let engines = engine_filter
        .map(|engine| vec![engine])
        .unwrap_or_else(backfill_engines);
    let mut any_engine_indexed = false;

    for engine in engines {
        let cursor_key = backfill_cursor_key(engine);
        let cursor = get_meta(&conn, cursor_key)
            .map_err(GaalError::from)?
            .and_then(|raw| parse_unix_seconds(&raw));
        let newer_than = cursor.and_then(|c| c.checked_sub(BACKFILL_CURSOR_SAFETY_MARGIN));

        match run_engine_pass(
            &mut conn,
            engine,
            newer_than,
            args.since.as_deref(),
            args.force,
            with_markdown,
            output_dir.as_deref(),
            &invalid_codex_error_sessions,
            &mut summary,
        ) {
            Ok(indexed_any) => {
                any_engine_indexed |= indexed_any;
                // Advance cursor only on successful pass completion.
                let secs = unix_seconds(run_start);
                if let Err(err) = set_meta(&conn, cursor_key, &secs.to_string()) {
                    eprintln!(
                        "warning: failed to persist backfill cursor for {:?}: {}",
                        engine, err
                    );
                }
            }
            Err(err) => {
                eprintln!("engine {:?} stalled: {}", engine, err);
                summary.errors += 1;
                // Cursor stays put — next run retries the missed window.
            }
        }
    }

    promote_codex_coordinators(&mut conn)?;
    if any_engine_indexed {
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

fn backfill_engines() -> Vec<Engine> {
    vec![
        Engine::Claude,
        Engine::Codex,
        Engine::Gemini,
        Engine::Agy,
        Engine::Hermes,
        Engine::Grok,
    ]
}

fn backfill_cursor_key(engine: Engine) -> &'static str {
    match engine {
        Engine::Claude => "backfill:claude",
        Engine::Codex => "backfill:codex",
        Engine::Gemini => "backfill:gemini",
        Engine::Agy => "backfill:agy",
        Engine::Hermes => "backfill:hermes",
        Engine::Grok => "backfill:grok",
    }
}

fn unix_seconds(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn parse_unix_seconds(raw: &str) -> Option<SystemTime> {
    let secs: u64 = raw.trim().parse().ok()?;
    UNIX_EPOCH.checked_add(Duration::from_secs(secs))
}

/// Run one engine's pass: discover (with mtime cutoff), iterate, index.
///
/// Returns `Ok(true)` when at least one session was indexed this pass.
#[allow(clippy::too_many_arguments)]
fn run_engine_pass(
    conn: &mut Connection,
    engine: Engine,
    newer_than: Option<SystemTime>,
    since: Option<&str>,
    force: bool,
    with_markdown: bool,
    output_dir: Option<&Path>,
    invalid_codex_error_sessions: &HashSet<String>,
    summary: &mut BackfillSummary,
) -> Result<bool, GaalError> {
    let mut sessions =
        discover_sessions_with_cutoff(Some(engine), newer_than).map_err(GaalError::from)?;
    if let Some(since) = since {
        sessions.retain(|session| session_on_or_after(session, since));
    }

    let total = sessions.len();
    let mut indexed_any = false;

    for (idx, session) in sessions.into_iter().enumerate() {
        match index_discovered_session(conn, &session, force, invalid_codex_error_sessions) {
            Ok(IndexOutcome::Indexed) => {
                summary.indexed += 1;
                indexed_any = true;
                eprintln!(
                    "[{}/{}] indexed {} ({})",
                    idx + 1,
                    total,
                    session.id,
                    session.path.display()
                );
                if with_markdown {
                    if let Some(output_dir) = output_dir {
                        match write_session_markdown_to_dir(conn, &session, output_dir, true) {
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
                        match generate_session_markdown(conn, &session) {
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
                if let Some(output_dir) = output_dir {
                    match write_session_markdown_to_dir(conn, &session, output_dir, false) {
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
                    match default_session_markdown_path(conn, &session) {
                        Ok(md_path) if !md_path.exists() => {
                            match generate_session_markdown(conn, &session) {
                                Ok(md_path) => {
                                    eprintln!("  -> markdown: {}", md_path.display());
                                }
                                Err(err) => {
                                    eprintln!("  -> markdown error: {err}");
                                }
                            }
                        }
                        Ok(_) => {}
                        Err(err) => {
                            eprintln!("  -> markdown error: {err}");
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

    Ok(indexed_any)
}

/// Run `gaal index status`.
pub fn run_status(human: bool) -> Result<(), GaalError> {
    let conn = open_db()?;
    let status = get_index_status(&conn)?;
    let grok = get_grok_diagnostic_summary(&conn)?;
    if human {
        print_status_human(&status, &grok);
        return Ok(());
    }

    let payload = json!({
        "db_path": crate::db::db_path().to_string_lossy().to_string(),
        "db_size_bytes": status.db_size_bytes,
        "sessions_total": status.sessions_total,
        "sessions_by_engine": status.sessions_by_engine,
        "facts_total": status.facts_total,
        "handoffs_total": status.handoffs_total,
        "last_indexed_at": status.last_indexed_at,
        "oldest_session": status.oldest_session,
        "newest_session": status.newest_session,
        "grok": grok
    });
    print_json(&payload).map_err(GaalError::from)
}

fn print_status_human(
    status: &crate::db::queries::IndexStatus,
    grok: &crate::db::queries::GrokDiagnosticSummary,
) {
    println!("Index status");
    println!("Sessions: {}", status.sessions_total);
    println!("Facts: {}", status.facts_total);
    println!("Handoffs: {}", status.handoffs_total);
    println!(
        "Oldest session: {}",
        status.oldest_session.as_deref().unwrap_or("-")
    );
    println!(
        "Newest session: {}",
        status.newest_session.as_deref().unwrap_or("-")
    );
    println!(
        "Last indexed event: {}",
        status.last_indexed_at.as_deref().unwrap_or("-")
    );

    if !status.sessions_by_engine.is_empty() {
        let mut rows: Vec<Vec<String>> = status
            .sessions_by_engine
            .iter()
            .map(|(engine, count)| vec![engine.clone(), count.to_string()])
            .collect();
        rows.sort_by(|a, b| a[0].cmp(&b[0]));
        println!();
        print_table(&["Engine", "Sessions"], &rows);
    }
    if grok.source_artifacts > 0 || grok.parser_observations > 0 {
        println!();
        println!("Grok diagnostics");
        println!("Sessions with artifacts: {}", grok.sessions_with_artifacts);
        println!("Source artifacts: {}", grok.source_artifacts);
        println!("Parser observations: {}", grok.parser_observations);
        println!("Unknown records: {}", grok.unknown_records);
        println!("Malformed records: {}", grok.malformed_records);
        println!("Private records: {}", grok.private_records);
        println!("Redacted records: {}", grok.redacted_records);
    }
}

/// Run `gaal index reindex`.
pub fn run_reindex(args: ReindexArgs) -> Result<(), GaalError> {
    let mut conn = open_db()?;
    let matching_ids = resolve_session_ids(&conn, &args.id, None)?;
    let resolved_id = match matching_ids.as_slice() {
        [] => return Err(GaalError::NotFound(args.id)),
        [id] => id.clone(),
        _ => return Err(GaalError::AmbiguousId(args.id)),
    };
    let existing = get_session(&conn, &resolved_id)?
        .ok_or_else(|| GaalError::NotFound(resolved_id.clone()))?;
    let path = PathBuf::from(&existing.jsonl_path);
    if !path.exists() {
        return Err(GaalError::NotFound(existing.jsonl_path));
    }

    let (parsed, offset) = if existing.engine == "hermes" {
        (
            crate::parser::hermes::parse_session(&path, &existing.id).map_err(GaalError::from)?,
            file_len_i64(&path)?,
        )
    } else if existing.engine == "grok" {
        (
            crate::parser::grok::parse_session(&path, &existing.id).map_err(GaalError::from)?,
            u64_to_i64(crate::discovery::grok::session_index_key(&path))?,
        )
    } else {
        (
            parse_session(&path).map_err(GaalError::from)?,
            file_len_i64(&path)?,
        )
    };
    let mut row = build_full_session_row(&parsed, &path, offset);
    row.id = existing.id.clone();
    row.session_type = existing.session_type.clone();
    row.parent_id = existing.parent_id.clone();
    row.subagent_type = existing.subagent_type.clone();
    let facts = normalize_facts(parsed.facts, &existing.id);
    let source_state =
        (existing.engine == "grok").then(|| crate::parser::grok::source_state(&path, &existing.id));

    let tx = conn
        .savepoint_with_name("reindex_session")
        .map_err(GaalError::from)?;
    tx.execute(
        "DELETE FROM facts WHERE session_id = :session_id",
        named_params! { ":session_id": &existing.id },
    )
    .map_err(GaalError::from)?;

    upsert_session(&tx, &row)?;
    if !facts.is_empty() {
        insert_facts_batch(&tx, &facts)?;
    }
    if let Some(state) = &source_state {
        replace_grok_source_state(&tx, &existing.id, state)?;
    }
    tx.commit().map_err(GaalError::from)?;
    search::build_search_index(&conn)?;

    let payload = ReindexSummary {
        session_id: existing.id,
        facts: facts.len(),
    };
    print_json(&payload).map_err(GaalError::from)
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
    let collision_policy = existing
        .as_ref()
        .map(|row| guard_index_collision(discovered, row))
        .transpose()?
        .unwrap_or(IndexCollisionPolicy::Safe);
    let file_size_i64 = u64_to_i64(discovered.file_size)?;
    let existing_peak_context_suspicious = existing
        .as_ref()
        .map(|row| row.peak_context > SUSPICIOUS_PEAK_CONTEXT_THRESHOLD)
        .unwrap_or(false);
    let existing_needs_full_reparse = existing
        .as_ref()
        .map(|row| session_needs_full_reparse(discovered, row, invalid_codex_error_sessions))
        .transpose()?
        .unwrap_or(false)
        || collision_policy == IndexCollisionPolicy::ForceFullReparse;

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
                && discovered.engine != Engine::Agy
                && discovered.engine != Engine::Hermes
                && discovered.engine != Engine::Grok
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

    let parsed = parse_discovered_session(discovered).map_err(GaalError::from)?;

    // Skip noise-only sessions (0 conversation turns, e.g. file-history-snapshot only).
    // Hermes cron/session-meta rows can be useful even without a user turn.
    let hermes_has_summary_or_tools = discovered.engine == Engine::Hermes
        && (parsed.session_summary.is_some() || parsed.total_tools > 0);
    if parsed.total_turns == 0 && !hermes_has_summary_or_tools {
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
    apply_hermes_lineage(&mut session_row, discovered);
    let facts = normalize_facts(parsed.facts, target_id);
    let source_state = (discovered.engine == Engine::Grok)
        .then(|| crate::parser::grok::source_state(&discovered.path, target_id));

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
    if let Some(state) = &source_state {
        replace_grok_source_state(&tx, target_id, state)?;
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
            gemini_summary: parsed.session_summary.clone(),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IndexCollisionPolicy {
    Safe,
    ForceFullReparse,
}

fn guard_index_collision(
    discovered: &DiscoveredSession,
    existing: &SessionRow,
) -> Result<IndexCollisionPolicy, GaalError> {
    resolve_index_collision(
        &discovered.id,
        &existing.engine,
        &existing.jsonl_path,
        &discovered.engine.to_string(),
        &discovered.path,
    )
}

fn resolve_index_collision(
    session_id: &str,
    existing_engine: &str,
    existing_path: &str,
    discovered_engine: &str,
    discovered_path: &Path,
) -> Result<IndexCollisionPolicy, GaalError> {
    if existing_engine != discovered_engine {
        return Err(GaalError::Internal(format!(
            "index collision for {session_id}: existing engine {existing_engine} at {existing_path} conflicts with discovered engine {discovered_engine} at {}",
            discovered_path.display()
        )));
    }

    if Path::new(existing_path) == discovered_path {
        return Ok(IndexCollisionPolicy::Safe);
    }

    if discovered_engine == "agy" {
        let existing_uuid = extract_agy_full_uuid_from_path(Path::new(existing_path), session_id);
        let discovered_uuid = extract_agy_full_uuid_from_path(discovered_path, session_id);

        return match (existing_uuid, discovered_uuid) {
            (Some(existing_uuid), Some(discovered_uuid)) if existing_uuid == discovered_uuid => {
                Ok(IndexCollisionPolicy::ForceFullReparse)
            }
            (Some(existing_uuid), Some(discovered_uuid)) => Err(GaalError::Internal(format!(
                "index collision for agy session {session_id}: existing brain UUID {existing_uuid} at {existing_path} conflicts with discovered brain UUID {discovered_uuid} at {}",
                discovered_path.display()
            ))),
            _ => Err(GaalError::Internal(format!(
                "index collision for agy session {session_id}: transcript path changed from {existing_path} to {}, but full brain UUID could not be established",
                discovered_path.display()
            ))),
        };
    }

    Ok(IndexCollisionPolicy::ForceFullReparse)
}

fn extract_agy_full_uuid_from_path(path: &Path, short_id: &str) -> Option<String> {
    let normalized_short_id = short_id
        .chars()
        .filter(|ch| ch.is_ascii_hexdigit())
        .map(|ch| ch.to_ascii_lowercase())
        .collect::<String>();
    if normalized_short_id.is_empty() {
        return None;
    }

    let path_text = path.to_string_lossy();
    uuid_candidates(&path_text)
        .into_iter()
        .find(|candidate| candidate.starts_with(&normalized_short_id))
}

fn uuid_candidates(text: &str) -> Vec<String> {
    let chars = text.chars().collect::<Vec<_>>();
    let mut out = Vec::new();

    for start in 0..chars.len() {
        if !chars[start].is_ascii_hexdigit() {
            continue;
        }

        let mut candidate = String::new();
        for ch in chars.iter().skip(start) {
            if ch.is_ascii_hexdigit() || *ch == '-' {
                candidate.push(*ch);
            } else {
                break;
            }
        }

        let normalized = candidate
            .chars()
            .filter(|ch| *ch != '-')
            .map(|ch| ch.to_ascii_lowercase())
            .collect::<String>();
        if normalized.len() == 32 && normalized.chars().all(|ch| ch.is_ascii_hexdigit()) {
            out.push(normalized);
        }
    }

    out
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
        gemini_summary: parsed.session_summary.clone(),
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
        gemini_summary: parsed_delta
            .session_summary
            .clone()
            .or_else(|| existing.gemini_summary.clone()),
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

fn apply_hermes_lineage(session_row: &mut SessionRow, discovered: &DiscoveredSession) {
    if discovered.engine != Engine::Hermes {
        return;
    }
    if let Some(parent_id) = discovered.forked_from_id.as_ref() {
        if !parent_id.is_empty() {
            session_row.parent_id = Some(parent_id.clone());
        }
    }
    session_row.session_type = "standalone".to_string();
    session_row.subagent_type = None;
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
fn default_session_markdown_path(
    conn: &Connection,
    discovered: &DiscoveredSession,
) -> Result<PathBuf, GaalError> {
    let engine = discovered.engine.to_string();
    let started_at = discovered
        .started_at
        .as_deref()
        .unwrap_or("1970-01-01T00:00:00Z");
    let (year, month, day) = extract_date_parts(started_at);
    let artifact_id = crate::db::queries::display_id_for_session(conn, &engine, &discovered.id)?;

    Ok(gaal_home()
        .join("data")
        .join(engine)
        .join("sessions")
        .join(year)
        .join(month)
        .join(day)
        .join(format!("{artifact_id}.md")))
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

    let markdown = render_discovered_markdown(conn, discovered)?;

    let engine = discovered.engine.to_string();
    let (year, month, day) = extract_date_parts(started_at);
    let artifact_id = crate::db::queries::display_id_for_session(conn, &engine, &discovered.id)?;

    let md_path = gaal_home()
        .join("data")
        .join(engine)
        .join("sessions")
        .join(year)
        .join(month)
        .join(day)
        .join(format!("{artifact_id}.md"));

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
    let engine = discovered.engine.to_string();
    let artifact_id = crate::db::queries::display_id_for_session(conn, &engine, &discovered.id)?;

    let md_path = output_dir
        .join(&year)
        .join(&month)
        .join(&day)
        .join(format!("{artifact_id}.md"));

    // Idempotent: skip if already written (unless overwrite requested).
    if !overwrite && md_path.exists() {
        return Ok(WriteOutcome::Skipped);
    }

    let markdown = render_discovered_markdown(conn, discovered)?;

    if let Some(parent) = md_path.parent() {
        fs::create_dir_all(parent).map_err(GaalError::from)?;
    }
    crate::util::atomic_write(&md_path, &markdown).map_err(GaalError::from)?;
    Ok(WriteOutcome::Written(md_path))
}

fn render_discovered_markdown(
    conn: &Connection,
    discovered: &DiscoveredSession,
) -> Result<String, GaalError> {
    if discovered.engine == Engine::Hermes {
        crate::render::session_md::render_hermes_session_markdown(
            &discovered.path,
            conn,
            &discovered.id,
        )
    } else if discovered.engine == Engine::Grok {
        crate::render::session_md::render_grok_session_markdown(&discovered.path, &discovered.id)
    } else {
        crate::render::session_md::render_session_markdown_with_db(
            &discovered.path,
            conn,
            Some(&discovered.id),
        )
    }
    .map_err(|e| GaalError::Internal(format!("render session markdown: {e}")))
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

    #[test]
    fn extracts_agy_full_uuid_matching_short_id_from_path() {
        let path = Path::new(
            "/Users/test/.agy/brains/abcdef12-3456-7890-abcd-ef1234567890/transcript.jsonl",
        );

        assert_eq!(
            extract_agy_full_uuid_from_path(path, "abcdef12").as_deref(),
            Some("abcdef1234567890abcdef1234567890")
        );
    }

    #[test]
    fn extracts_agy_full_uuid_matching_short_id_from_compact_path() {
        let path = Path::new("/Users/test/.agy/brains/abcdef1234567890abcdef1234567890.jsonl");

        assert_eq!(
            extract_agy_full_uuid_from_path(path, "abcdef12").as_deref(),
            Some("abcdef1234567890abcdef1234567890")
        );
    }

    #[test]
    fn ignores_agy_uuid_candidates_that_do_not_match_short_id() {
        let path = Path::new(
            "/Users/test/.agy/brains/99999999-3456-7890-abcd-ef1234567890/transcript.jsonl",
        );

        assert_eq!(extract_agy_full_uuid_from_path(path, "abcdef12"), None);
    }

    #[test]
    fn allows_agy_path_change_for_same_full_uuid_and_forces_reparse() {
        let existing_path = "/Users/test/.agy/fallback/abcdef12-3456-7890-abcd-ef1234567890.jsonl";
        let discovered_path =
            Path::new("/Users/test/.agy/preferred/abcdef1234567890abcdef1234567890.jsonl");

        let decision =
            resolve_index_collision("abcdef12", "agy", existing_path, "agy", discovered_path)
                .unwrap();

        assert_eq!(decision, IndexCollisionPolicy::ForceFullReparse);
    }

    #[test]
    fn rejects_same_short_id_for_different_engines() {
        let err = resolve_index_collision(
            "abcdef12",
            "codex",
            "/tmp/codex/abcdef12.jsonl",
            "agy",
            Path::new("/tmp/agy/abcdef12-3456-7890-abcd-ef1234567890.jsonl"),
        )
        .unwrap_err()
        .to_string();

        assert!(err.contains("existing engine codex"));
        assert!(err.contains("discovered engine agy"));
    }

    #[test]
    fn allows_same_engine_non_agy_path_change_and_forces_reparse() {
        let decision = resolve_index_collision(
            "abcdef12",
            "gemini",
            "/tmp/gemini/old/session-abcdef12.json",
            "gemini",
            Path::new("/tmp/gemini/new/session-abcdef12.json"),
        )
        .unwrap();

        assert_eq!(decision, IndexCollisionPolicy::ForceFullReparse);
    }

    #[test]
    fn rejects_agy_path_change_for_different_full_uuid() {
        let err = resolve_index_collision(
            "abcdef12",
            "agy",
            "/tmp/agy/abcdef12-3456-7890-abcd-ef1234567890.jsonl",
            "agy",
            Path::new("/tmp/agy/abcdef12-9999-7890-abcd-ef1234567890.jsonl"),
        )
        .unwrap_err()
        .to_string();

        assert!(err.contains("existing brain UUID abcdef1234567890abcdef1234567890"));
        assert!(err.contains("discovered brain UUID abcdef1299997890abcdef1234567890"));
    }

    #[test]
    fn rejects_agy_path_change_without_full_uuid() {
        let err = resolve_index_collision(
            "abcdef12",
            "agy",
            "/tmp/agy/fallback/transcript.jsonl",
            "agy",
            Path::new("/tmp/agy/preferred/transcript.jsonl"),
        )
        .unwrap_err()
        .to_string();

        assert!(err.contains("full brain UUID could not be established"));
    }
}
