//! `gaal index recover-orphans` — recover subagent JSONL files orphaned by CC's 30-day cleanup.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;

use rusqlite::{named_params, OptionalExtension};
use serde_json::json;

use crate::commands::search;
use crate::db::open_db;
use crate::db::queries::{add_tag, get_session, insert_facts_batch, upsert_session, SessionRow};
use crate::error::GaalError;
use crate::output::json::print_json;
use crate::parser::parse_session;
use crate::subagent::collect_all_subagent_files;

use super::{file_len_i64, normalize_facts, resolve_subagent_session_id, EPOCH_RFC3339};

/// Arguments for `gaal index recover-orphans`.
#[derive(Debug, Clone)]
pub struct RecoverOrphansArgs {
    /// Preview mode — report findings without DB writes.
    pub dry_run: bool,
}

/// Run `gaal index recover-orphans`.
pub fn run_recover_orphans(args: RecoverOrphansArgs) -> Result<(), GaalError> {
    let claude_projects_root = dirs::home_dir()
        .ok_or_else(|| GaalError::Internal("home directory not found".to_string()))?
        .join(".claude/projects");
    let subagent_files = collect_all_subagent_files(&claude_projects_root);

    let mut seen_paths = HashSet::new();
    let mut deduped_files = Vec::new();
    for subagent_file in subagent_files {
        let canonical_path = match fs::canonicalize(&subagent_file.path) {
            Ok(path) => path,
            Err(err) => {
                eprintln!(
                    "recover-orphans warning: failed to canonicalize {}: {}",
                    subagent_file.path.display(),
                    err
                );
                continue;
            }
        };

        if !seen_paths.insert(canonical_path.clone()) {
            continue;
        }

        deduped_files.push((subagent_file, canonical_path));
    }

    let mut conn = open_db()?;
    let mut orphan_groups: HashMap<String, Vec<(crate::subagent::SubagentFile, PathBuf)>> =
        HashMap::new();

    {
        let mut stmt = conn
            .prepare("SELECT parent_id FROM sessions WHERE jsonl_path = :path LIMIT 1")
            .map_err(GaalError::from)?;

        for (subagent_file, canonical_path) in deduped_files {
            let Some(parent_uuid) = subagent_file
                .parent_session_dir
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned)
            else {
                eprintln!(
                    "recover-orphans warning: missing parent UUID for {}",
                    canonical_path.display()
                );
                continue;
            };

            let canonical_path_str = canonical_path.to_string_lossy().to_string();
            let existing_parent_id: Option<Option<String>> = stmt
                .query_row(named_params! { ":path": &canonical_path_str }, |row| {
                    row.get(0)
                })
                .optional()
                .map_err(GaalError::from)?;

            if matches!(existing_parent_id, Some(Some(_))) {
                continue;
            }

            orphan_groups
                .entry(parent_uuid)
                .or_default()
                .push((subagent_file, canonical_path));
        }
    }

    let orphan_files = orphan_groups.values().map(Vec::len).sum::<usize>();
    let parent_groups = orphan_groups.len();

    if args.dry_run {
        eprintln!(
            "recover-orphans dry-run: found {} orphan files across {} parent groups",
            orphan_files, parent_groups
        );
        let payload = json!({
            "orphan_files": orphan_files,
            "parent_groups": parent_groups,
            "dry_run": true
        });
        return print_json(&payload).map_err(GaalError::from);
    }

    let total = orphan_files;
    let mut processed = 0usize;
    let mut ghosts_created = 0usize;
    let mut subagents_indexed = 0usize;
    let mut errors = 0usize;
    let mut parent_ids = orphan_groups.keys().cloned().collect::<Vec<_>>();
    parent_ids.sort();

    for parent_uuid in parent_ids {
        let Some(group) = orphan_groups.get(&parent_uuid) else {
            continue;
        };
        let parent_short_id: String = parent_uuid.chars().take(8).collect();
        if parent_short_id.is_empty() {
            errors += group.len();
            eprintln!("recover-orphans warning: invalid parent UUID {parent_uuid}");
            continue;
        }

        let parent_exists = get_session(&conn, &parent_short_id)?;
        if parent_exists.is_none() {
            let mut ghost_cwd = None;
            let mut ghost_started_at: Option<String> = None;

            for (_, canonical_path) in group {
                match parse_session(canonical_path) {
                    Ok(parsed) => {
                        let candidate_started_at = if parsed.meta.started_at == EPOCH_RFC3339 {
                            None
                        } else {
                            Some(parsed.meta.started_at.clone())
                        };
                        let replace = match (&ghost_started_at, &candidate_started_at) {
                            (None, Some(_)) => true,
                            (None, None) => ghost_cwd.is_none(),
                            (Some(current), Some(candidate)) => candidate < current,
                            _ => false,
                        };
                        if replace {
                            ghost_cwd = parsed.meta.cwd.clone();
                            ghost_started_at =
                                Some(candidate_started_at.unwrap_or(parsed.meta.started_at));
                        }
                    }
                    Err(err) => {
                        eprintln!(
                            "recover-orphans warning: failed to parse {} while creating ghost {}: {}",
                            canonical_path.display(),
                            parent_short_id,
                            err
                        );
                    }
                }
            }

            let Some(started_at) = ghost_started_at else {
                errors += group.len();
                eprintln!(
                    "recover-orphans warning: no parseable subagents for ghost parent {}",
                    parent_short_id
                );
                continue;
            };

            let ghost = SessionRow {
                id: parent_short_id.clone(),
                engine: "claude".to_string(),
                model: None,
                cwd: ghost_cwd,
                started_at,
                ended_at: None,
                exit_signal: None,
                last_event_at: None,
                parent_id: None,
                session_type: "coordinator".to_string(),
                jsonl_path: "(recovered)".to_string(),
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
                gemini_summary: None,
            };
            upsert_session(&conn, &ghost)?;
            add_tag(&conn, &parent_short_id, "_recovered")?;
            ghosts_created += 1;
        } else if parent_exists
            .as_ref()
            .map(|row| row.session_type == "standalone")
            .unwrap_or(false)
        {
            conn.execute(
                "UPDATE sessions SET session_type = 'coordinator' WHERE id = :id",
                named_params! { ":id": &parent_short_id },
            )
            .map_err(GaalError::from)?;
        }

        for (subagent_file, canonical_path) in group {
            processed += 1;
            if processed.is_multiple_of(100) {
                eprintln!("[{}/{}] processing orphans...", processed, total);
            }

            let parsed = match parse_session(canonical_path) {
                Ok(parsed) => parsed,
                Err(err) => {
                    errors += 1;
                    eprintln!(
                        "recover-orphans warning: failed to parse {}: {}",
                        canonical_path.display(),
                        err
                    );
                    continue;
                }
            };

            let Some(child_id) =
                resolve_subagent_session_id(&conn, &subagent_file.agent_id, &parent_short_id)?
            else {
                errors += 1;
                eprintln!(
                    "recover-orphans warning: id collision for agent {} under parent {}",
                    subagent_file.agent_id, parent_short_id
                );
                continue;
            };

            let last_indexed_offset = match file_len_i64(canonical_path) {
                Ok(len) => len,
                Err(err) => {
                    errors += 1;
                    eprintln!(
                        "recover-orphans warning: failed to stat {}: {}",
                        canonical_path.display(),
                        err
                    );
                    continue;
                }
            };

            let child_facts = normalize_facts(parsed.facts, &child_id);
            let child_row = SessionRow {
                id: child_id.clone(),
                engine: "claude".to_string(),
                model: parsed.meta.model.clone(),
                cwd: parsed.meta.cwd.clone(),
                started_at: parsed.meta.started_at.clone(),
                ended_at: parsed.ended_at.clone(),
                exit_signal: parsed.exit_signal.clone(),
                last_event_at: parsed.last_event_at.clone(),
                parent_id: Some(parent_short_id.clone()),
                session_type: "subagent".to_string(),
                jsonl_path: canonical_path.to_string_lossy().to_string(),
                total_input_tokens: parsed.total_input_tokens,
                total_output_tokens: parsed.total_output_tokens,
                cache_read_tokens: parsed.cache_read_tokens,
                cache_creation_tokens: parsed.cache_creation_tokens,
                reasoning_tokens: parsed.reasoning_tokens,
                total_tools: i64::from(parsed.total_tools),
                total_turns: i64::from(parsed.total_turns),
                peak_context: parsed.peak_context,
                last_indexed_offset,
                subagent_type: None, // orphan recovery doesn't have parent context
                gemini_summary: parsed.session_summary.clone(),
            };

            let tx = match conn.savepoint_with_name("recover_orphan") {
                Ok(tx) => tx,
                Err(err) => {
                    errors += 1;
                    eprintln!(
                        "recover-orphans warning: savepoint failed for {}: {}",
                        canonical_path.display(),
                        err
                    );
                    continue;
                }
            };

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
                tx.commit().map_err(GaalError::from)?;
                Ok(())
            })();

            match save_result {
                Ok(()) => subagents_indexed += 1,
                Err(err) => {
                    errors += 1;
                    eprintln!(
                        "recover-orphans warning: failed to save {}: {}",
                        canonical_path.display(),
                        err
                    );
                }
            }
        }
    }

    search::build_search_index(&conn)?;

    let payload = json!({
        "ghosts_created": ghosts_created,
        "subagents_indexed": subagents_indexed,
        "errors": errors
    });
    print_json(&payload).map_err(GaalError::from)
}
