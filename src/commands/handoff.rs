use std::fs::{self, File};
use std::io::{BufRead, BufReader};
#[cfg(unix)]
use std::os::fd::AsFd;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::str::FromStr;
use std::sync::{mpsc, Arc, LazyLock, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::anyhow;
use chrono::{DateTime, Local, TimeDelta, Utc};
use regex::Regex;
use rusqlite::{named_params, Connection};
use serde::Serialize;
use serde_json::Value;

use crate::commands::index::{index_discovered_session, IndexOutcome};
use crate::config::{gaal_home, load_config, AgentMuxConfig, GaalConfig};
use crate::db::open_db;
use crate::db::queries::{get_facts, get_session, upsert_handoff, SessionRow};
use crate::discovery::DiscoveredSession;
use crate::error::GaalError;
use crate::model::{Fact, FactType, HandoffRecord};
use crate::output::json::print_json;
use crate::parser::types::Engine;

/// Built-in fallback extraction prompt used when no prompt file is available.
const DEFAULT_HANDOFF_PROMPT: &str = r#"You are analyzing an agent session trace. Extract:
## Headline (one-line summary)
## What Happened (structured bullet summary of key actions)
## Key Decisions (decisions made)
## Open Threads (unfinished work)
## Key Files (files created/modified with descriptions)
Also extract: projects (list), keywords (list), substance score (0-3)."#;

static FENCED_JSON_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)```json\s*\n(.*?)\n\s*```")
        .expect("fenced JSON regex for handoff metadata should compile")
});

/// Arguments for `gaal create-handoff`.
#[derive(Debug, Clone)]
pub struct HandoffArgs {
    /// Session id/prefix, or the keyword `today`.
    pub id: Option<String>,
    /// Direct JSONL path override.
    pub jsonl: Option<PathBuf>,
    /// Override extraction engine (defaults to config).
    pub engine: Option<String>,
    /// Override extraction model (defaults to config).
    pub model: Option<String>,
    /// Optional prompt path override.
    pub prompt: Option<PathBuf>,
    /// Optional provider label for metadata/routing context.
    pub provider: Option<String>,
    /// Optional output format label.
    pub format: Option<String>,
    /// Run batch mode.
    pub batch: bool,
    /// Time window (for example: `7d`).
    pub since: Option<String>,
    /// Max concurrent workers.
    pub parallel: usize,
    /// Minimum turns required to process a session.
    pub min_turns: usize,
    /// Force using the nearest detected session (this process lineage).
    pub force_this: bool,
    /// Preview candidates without processing.
    pub dry_run: bool,
    /// Effort level override (low, medium, high, xhigh).
    pub effort: Option<String>,
}

#[derive(Debug, Serialize)]
struct HandoffRunResult {
    session_id: String,
    handoff_path: String,
    headline: Option<String>,
    projects: Vec<String>,
    keywords: Vec<String>,
    substance: i32,
}

#[derive(Debug, Serialize, Clone)]
struct BatchResult {
    session_id: String,
    status: String,
    handoff_path: Option<String>,
    error: Option<String>,
    duration_secs: f64,
}

#[derive(Debug, Default, Clone)]
struct ExtractedMetadata {
    headline: Option<String>,
    projects: Vec<String>,
    keywords: Vec<String>,
    substance: i32,
}

#[derive(Debug, Clone)]
struct DetectedSession {
    engine: String,
    session_id: String,
    jsonl_path: PathBuf,
    pid: u32,
}

/// Runs the `gaal create-handoff` workflow.
pub fn run(args: HandoffArgs) -> Result<(), GaalError> {
    let mut config = load_config();
    if config.handoff.prompt.is_relative() {
        config.handoff.prompt = gaal_home().join(&config.handoff.prompt);
    }

    // CLI --effort overrides config effort
    if let Some(ref effort) = args.effort {
        config.agent_mux.effort = Some(effort.clone());
    }

    // When effort is set, ensure gaal's wrapper timeout is at least as long
    // as agent-mux's effort-mapped timeout bucket to avoid premature kills.
    if let Some(ref effort) = config.agent_mux.effort {
        let min_timeout = match effort.as_str() {
            "low" => 130,
            "medium" => 610,
            "high" => 1810,
            "xhigh" => 2710,
            _ => 0,
        };
        let current = config.agent_mux.timeout_secs.unwrap_or(config.llm.timeout_secs);
        if current < min_timeout {
            config.agent_mux.timeout_secs = Some(min_timeout);
        }
    }

    let mut conn = open_db()?;
    if args.batch {
        return run_batch(&conn, &config, &args);
    }

    let (id_or_today, detected) = if let Some(ref jsonl_path) = args.jsonl {
        let session_id = jsonl_path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| GaalError::ParseError("invalid --jsonl path".into()))?
            .to_string();
        let engine_name = if jsonl_path.to_string_lossy().contains(".codex") {
            "codex"
        } else {
            "claude"
        };
        let detected = DetectedSession {
            engine: engine_name.to_string(),
            session_id: session_id.clone(),
            jsonl_path: jsonl_path.clone(),
            pid: 0,
        };
        eprintln!("Using provided JSONL: {}", jsonl_path.display());
        (session_id, Some(detected))
    } else if let Some(id) = args.id.clone() {
        (id, None)
    } else {
        let detected = if args.force_this {
            detect_current_session()?
        } else {
            detect_preferred_session()?
        };
        eprintln!(
            "Auto-detected {} session {} (PID {}, JSONL: {})",
            detected.engine,
            detected.session_id,
            detected.pid,
            detected.jsonl_path.display()
        );
        let id = detected.session_id.clone();
        (id, Some(detected))
    };

    let engine = args
        .engine
        .clone()
        .unwrap_or_else(|| config.llm.default_engine.clone());
    let model = args
        .model
        .clone()
        .unwrap_or_else(|| config.llm.default_model.clone());
    let provider = args
        .provider
        .clone()
        .unwrap_or_else(|| "agent-mux".to_string());
    let format = args
        .format
        .clone()
        .unwrap_or_else(|| config.handoff.format.clone());
    let prompt_path = args
        .prompt
        .clone()
        .unwrap_or_else(|| config.handoff.prompt.clone());
    let prompt = load_prompt(&prompt_path)?;

    let sessions = match resolve_sessions(&conn, &id_or_today) {
        Ok(sessions) => sessions,
        Err(GaalError::NotFound(_)) if detected.is_some() => {
            // Session not in DB yet (active session, cron hasn't indexed it).
            // Index the JSONL on-the-fly and retry.
            let detected = detected.as_ref().unwrap();
            eprintln!(
                "Session not indexed yet — indexing {} on-the-fly...",
                detected.jsonl_path.display()
            );
            let short_id = index_single_jsonl(&mut conn, detected)?;
            // Retry with the truncated ID that the indexer stores in the DB.
            resolve_sessions(&conn, &short_id)?
        }
        Err(err) => return Err(err),
    };
    if sessions.is_empty() {
        return Err(GaalError::NoResults);
    }

    let mut results = Vec::new();
    for session in sessions {
        let processed = process_session_handoff(
            &conn, &config, &session, &engine, &model, &prompt, &provider, &format,
        )?;

        results.push(HandoffRunResult {
            session_id: session.id,
            handoff_path: processed.path.to_string_lossy().to_string(),
            headline: processed.extracted.headline,
            projects: processed.extracted.projects,
            keywords: processed.extracted.keywords,
            substance: processed.extracted.substance,
        });
    }

    print_json(&results).map_err(GaalError::from)
}

#[derive(Debug, Clone)]
struct ProcessedSessionHandoff {
    path: PathBuf,
    extracted: ExtractedMetadata,
}

fn run_batch(conn: &Connection, config: &GaalConfig, args: &HandoffArgs) -> Result<(), GaalError> {
    let engine = args
        .engine
        .clone()
        .unwrap_or_else(|| config.llm.default_engine.clone());
    let model = args
        .model
        .clone()
        .unwrap_or_else(|| config.llm.default_model.clone());
    let provider = args
        .provider
        .clone()
        .unwrap_or_else(|| "agent-mux".to_string());
    let format = args
        .format
        .clone()
        .unwrap_or_else(|| config.handoff.format.clone());
    let prompt_path = args
        .prompt
        .clone()
        .unwrap_or_else(|| config.handoff.prompt.clone());
    let prompt = load_prompt(&prompt_path)?;

    let since_date = parse_since_filter(args.since.as_deref().unwrap_or("7d"));
    let candidates = find_batch_candidates(conn, &since_date, args.min_turns)?;
    if candidates.is_empty() {
        eprintln!(
            "Batch complete: 0/0 succeeded, 0 failed (since {since_date}, min_turns={})",
            args.min_turns
        );
        return print_json(&Vec::<BatchResult>::new()).map_err(GaalError::from);
    }

    if args.dry_run {
        eprintln!(
            "Batch dry-run: {} candidate session(s) since {} with min_turns={}",
            candidates.len(),
            since_date,
            args.min_turns
        );
        for session in &candidates {
            eprintln!(
                "- {} (started_at={}, turns={})",
                session.id, session.started_at, session.total_turns
            );
        }
        let results: Vec<BatchResult> = candidates
            .iter()
            .map(|session| BatchResult {
                session_id: session.id.clone(),
                status: "pending".to_string(),
                handoff_path: None,
                error: None,
                duration_secs: 0.0,
            })
            .collect();
        return print_json(&results).map_err(GaalError::from);
    }

    let total = candidates.len();
    let mut results = Vec::with_capacity(total);

    if args.parallel <= 1 || total <= 1 {
        for (idx, session) in candidates.iter().enumerate() {
            eprintln!("Batch {}/{}: {}", idx + 1, total, session.id);
            let started = Instant::now();
            let outcome = process_single_batch_session(
                conn, config, session, &engine, &model, &prompt, &provider, &format,
            );
            let duration_secs = started.elapsed().as_secs_f64();
            match outcome {
                Ok(path) => results.push(BatchResult {
                    session_id: session.id.clone(),
                    status: "success".to_string(),
                    handoff_path: Some(path),
                    error: None,
                    duration_secs,
                }),
                Err(err) => results.push(BatchResult {
                    session_id: session.id.clone(),
                    status: "error".to_string(),
                    handoff_path: None,
                    error: Some(err.to_string()),
                    duration_secs,
                }),
            }
        }
    } else {
        let workers = args.parallel.clamp(1, 5);
        let chunk_size = candidates.len().div_ceil(workers);
        let shared: Arc<Mutex<Vec<BatchResult>>> = Arc::new(Mutex::new(Vec::with_capacity(total)));
        let mut handles = Vec::new();

        for chunk in candidates.chunks(chunk_size) {
            let sessions = chunk.to_vec();
            let shared_results = Arc::clone(&shared);
            let engine = engine.clone();
            let model = model.clone();
            let prompt = prompt.clone();
            let provider = provider.clone();
            let format = format.clone();
            let config = config.clone();

            handles.push(thread::spawn(move || {
                let thread_conn = match open_db() {
                    Ok(conn) => conn,
                    Err(err) => {
                        if let Ok(mut guard) = shared_results.lock() {
                            for session in sessions {
                                guard.push(BatchResult {
                                    session_id: session.id,
                                    status: "error".to_string(),
                                    handoff_path: None,
                                    error: Some(err.to_string()),
                                    duration_secs: 0.0,
                                });
                            }
                        }
                        return;
                    }
                };

                for session in sessions {
                    let started = Instant::now();
                    let outcome = process_single_batch_session(
                        &thread_conn,
                        &config,
                        &session,
                        &engine,
                        &model,
                        &prompt,
                        &provider,
                        &format,
                    );
                    let duration_secs = started.elapsed().as_secs_f64();
                    let result = match outcome {
                        Ok(path) => BatchResult {
                            session_id: session.id.clone(),
                            status: "success".to_string(),
                            handoff_path: Some(path),
                            error: None,
                            duration_secs,
                        },
                        Err(err) => BatchResult {
                            session_id: session.id.clone(),
                            status: "error".to_string(),
                            handoff_path: None,
                            error: Some(err.to_string()),
                            duration_secs,
                        },
                    };

                    if let Ok(mut guard) = shared_results.lock() {
                        guard.push(result);
                    }
                }
            }));
        }

        for handle in handles {
            let _ = handle.join();
        }

        let guard = shared
            .lock()
            .map_err(|_| GaalError::Internal("batch results lock poisoned".to_string()))?;
        results = guard.clone();
    }

    let succeeded = results.iter().filter(|r| r.status == "success").count();
    let failed = results.iter().filter(|r| r.status == "error").count();
    eprintln!("Batch complete: {succeeded}/{total} succeeded, {failed} failed");

    print_json(&results).map_err(GaalError::from)
}

fn process_single_batch_session(
    conn: &Connection,
    config: &GaalConfig,
    session: &SessionRow,
    engine: &str,
    model: &str,
    prompt: &str,
    provider: &str,
    format: &str,
) -> Result<String, GaalError> {
    let processed = process_session_handoff(
        conn, config, session, engine, model, prompt, provider, format,
    )?;
    Ok(processed.path.to_string_lossy().to_string())
}

fn validate_handoff_metadata(
    headline: Option<&str>,
    substance: i32,
    projects: &[String],
    keywords: &[String],
) -> Result<(), String> {
    let hl = headline.unwrap_or("");
    if hl.len() < 5 {
        return Err(format!("headline too short ({} chars, need ≥5)", hl.len()));
    }
    if !(0..=3).contains(&substance) {
        return Err(format!(
            "substance out of range: {} (expected 0-3)",
            substance
        ));
    }
    if substance >= 1 && projects.is_empty() {
        return Err("substantive session (substance ≥1) must have at least one project".into());
    }
    if substance >= 1 && keywords.is_empty() {
        return Err("substantive session (substance ≥1) must have at least one keyword".into());
    }
    Ok(())
}

fn process_session_handoff(
    conn: &Connection,
    config: &GaalConfig,
    session: &SessionRow,
    engine: &str,
    model: &str,
    prompt: &str,
    provider: &str,
    format: &str,
) -> Result<ProcessedSessionHandoff, GaalError> {
    // Try session markdown transcript first (full narrative context),
    // fall back to DB facts (lossy structured context).
    let context = match resolve_session_transcript(session, config) {
        Some(transcript) => {
            eprintln!(
                "Using session transcript ({} chars) for context",
                transcript.len()
            );
            build_context_from_transcript(session, &transcript, provider, format)
        }
        None => {
            eprintln!("No session transcript found, falling back to DB facts");
            let facts = get_facts(conn, &session.id, None)?;
            build_context(session, &facts, provider, format)
        }
    };

    let max_attempts = 2;
    let mut response = String::new();
    let mut extracted = ExtractedMetadata::default();
    let timeout_secs = config
        .agent_mux
        .timeout_secs
        .unwrap_or(config.llm.timeout_secs);
    for attempt in 1..=max_attempts {
        response = invoke_agent_mux(
            &config.agent_mux,
            engine,
            model,
            session.cwd.as_deref().unwrap_or("."),
            prompt,
            &context,
            timeout_secs,
        )?;
        extracted = extract_metadata(&response);
        match validate_handoff_metadata(
            extracted.headline.as_deref(),
            extracted.substance,
            &extracted.projects,
            &extracted.keywords,
        ) {
            Ok(()) => break,
            Err(reason) if attempt < max_attempts => {
                eprintln!(
                    "Handoff validation failed (attempt {}/{}): {}. Retrying...",
                    attempt, max_attempts, reason
                );
            }
            Err(reason) => {
                eprintln!(
                    "Handoff validation failed after {} attempts: {}. Accepting best-effort.",
                    max_attempts, reason
                );
            }
        }
    }

    // Use the session's own engine/model for frontmatter (ground truth),
    // not the extraction LLM engine/model.
    let session_engine = &session.engine;
    let session_model = session.model.as_deref().unwrap_or("unknown");
    let frontmatter = build_handoff_frontmatter(session, &extracted, session_engine, session_model);
    let full_content = format!("{}{}", frontmatter, response);
    let handoff_path = write_handoff_markdown(session, &full_content)?;
    let generated_by = build_generated_by_label(&config.agent_mux, engine, model);

    let record = HandoffRecord {
        session_id: session.id.clone(),
        headline: extracted.headline.clone(),
        projects: extracted.projects.clone(),
        keywords: extracted.keywords.clone(),
        substance: extracted.substance,
        duration_minutes: duration_minutes(session),
        generated_at: Some(Utc::now().to_rfc3339()),
        generated_by: Some(generated_by),
        content_path: Some(handoff_path.to_string_lossy().to_string()),
    };
    upsert_handoff(conn, &record)?;

    Ok(ProcessedSessionHandoff {
        path: handoff_path,
        extracted,
    })
}

fn find_batch_candidates(
    conn: &Connection,
    since_date: &str,
    min_turns: usize,
) -> Result<Vec<SessionRow>, GaalError> {
    let mut stmt = conn
        .prepare(
            r#"
            SELECT
                s.id, s.engine, s.model, s.cwd, s.started_at, s.ended_at, s.exit_signal, s.last_event_at,
                s.parent_id, s.session_type, s.jsonl_path, s.total_input_tokens, s.total_output_tokens,
                s.total_tools, s.total_turns, s.last_indexed_offset
            FROM sessions s
            WHERE s.id NOT IN (SELECT session_id FROM handoffs)
              AND s.total_turns >= :min_turns
              AND s.started_at >= :since
            ORDER BY s.started_at DESC
            "#,
        )
        .map_err(GaalError::from)?;
    let mut rows = stmt
        .query(named_params! {
            ":min_turns": min_turns as i64,
            ":since": since_date,
        })
        .map_err(GaalError::from)?;

    let mut out = Vec::new();
    while let Some(row) = rows.next().map_err(GaalError::from)? {
        out.push(SessionRow {
            id: row.get(0).map_err(GaalError::from)?,
            engine: row.get(1).map_err(GaalError::from)?,
            model: row.get(2).map_err(GaalError::from)?,
            cwd: row.get(3).map_err(GaalError::from)?,
            started_at: row.get(4).map_err(GaalError::from)?,
            ended_at: row.get(5).map_err(GaalError::from)?,
            exit_signal: row.get(6).map_err(GaalError::from)?,
            last_event_at: row.get(7).map_err(GaalError::from)?,
            parent_id: row.get(8).map_err(GaalError::from)?,
            session_type: row.get(9).map_err(GaalError::from)?,
            jsonl_path: row.get(10).map_err(GaalError::from)?,
            total_input_tokens: row.get(11).map_err(GaalError::from)?,
            total_output_tokens: row.get(12).map_err(GaalError::from)?,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
            reasoning_tokens: 0,
            total_tools: row.get(13).map_err(GaalError::from)?,
            total_turns: row.get(14).map_err(GaalError::from)?,
            peak_context: 0,
            last_indexed_offset: row.get(15).map_err(GaalError::from)?,
            subagent_type: None,
        });
    }
    Ok(out)
}

fn parse_since_filter(since: &str) -> String {
    let raw = since.trim();
    if raw.is_empty() {
        return (Local::now() - TimeDelta::days(7))
            .format("%Y-%m-%d")
            .to_string();
    }
    if raw.len() >= 10 && raw.contains('-') {
        return raw.to_string();
    }

    let normalized = raw.to_ascii_lowercase();
    let (count_raw, unit) = normalized.split_at(normalized.len().saturating_sub(1));
    let count = count_raw
        .parse::<i64>()
        .ok()
        .filter(|v| *v > 0)
        .unwrap_or(7);
    let days = match unit {
        "d" => count,
        "w" => count.saturating_mul(7),
        _ => 7,
    };

    let delta = TimeDelta::try_days(days).unwrap_or_else(|| TimeDelta::days(7));
    (Local::now() - delta).format("%Y-%m-%d").to_string()
}

fn resolve_sessions(conn: &Connection, id_or_today: &str) -> Result<Vec<SessionRow>, GaalError> {
    if id_or_today.eq_ignore_ascii_case("latest") {
        return Ok(vec![crate::commands::inspect::resolve_one(conn, "latest")?]);
    }

    if id_or_today.eq_ignore_ascii_case("today") {
        return resolve_today_sessions(conn);
    }

    if let Some(exact) = get_session(conn, id_or_today)? {
        return Ok(vec![exact]);
    }

    let mut stmt = conn
        .prepare(
            r#"
            SELECT id
            FROM sessions
            WHERE id LIKE :prefix
            ORDER BY started_at DESC
            "#,
        )
        .map_err(GaalError::from)?;
    let pattern = format!("{id_or_today}%");
    let mut rows = stmt
        .query(named_params! { ":prefix": pattern })
        .map_err(GaalError::from)?;

    let mut ids = Vec::new();
    while let Some(row) = rows.next().map_err(GaalError::from)? {
        ids.push(row.get::<_, String>(0).map_err(GaalError::from)?);
    }

    if ids.is_empty() {
        return Err(GaalError::NotFound(id_or_today.to_string()));
    }
    if ids.len() > 1 {
        let choices = ids.join(", ");
        return Err(GaalError::AmbiguousId(format!("{id_or_today} ({choices})")));
    }

    match get_session(conn, &ids[0])? {
        Some(session) => Ok(vec![session]),
        None => Err(GaalError::NotFound(id_or_today.to_string())),
    }
}

fn resolve_today_sessions(conn: &Connection) -> Result<Vec<SessionRow>, GaalError> {
    let today = Local::now().format("%Y-%m-%d").to_string();
    let pattern = format!("{today}%");
    let mut stmt = conn
        .prepare(
            r#"
            SELECT id
            FROM sessions
            WHERE started_at LIKE :today
            ORDER BY started_at ASC
            "#,
        )
        .map_err(GaalError::from)?;
    let mut rows = stmt
        .query(named_params! { ":today": pattern })
        .map_err(GaalError::from)?;

    let mut out = Vec::new();
    while let Some(row) = rows.next().map_err(GaalError::from)? {
        let id = row.get::<_, String>(0).map_err(GaalError::from)?;
        if let Some(session) = get_session(conn, &id)? {
            out.push(session);
        }
    }
    Ok(out)
}

/// Returns all session candidates along the current process ancestry.
/// First element is closest to gaal (likely child), last is furthest (likely parent).
fn detect_session_candidates() -> Result<Vec<DetectedSession>, GaalError> {
    let mut current = std::process::id();
    let mut candidates = Vec::new();

    for _ in 0..20 {
        let Some(name) = get_process_name(current) else {
            break;
        };
        let engine = name.to_ascii_lowercase();
        if engine == "claude" || engine == "codex" {
            let jsonl_path =
                resolve_jsonl_for_pid(current).or_else(|| resolve_jsonl_via_cwd(current, &engine));
            if let Some(jsonl_path) = jsonl_path {
                if let Some(session_id) = extract_session_id_from_jsonl(&jsonl_path, &engine) {
                    candidates.push(DetectedSession {
                        engine: engine.clone(),
                        session_id,
                        jsonl_path,
                        pid: current,
                    });
                }
            }
        }

        let Some(parent) = get_ppid(current) else {
            break;
        };
        if parent <= 1 || parent == current {
            break;
        }
        current = parent;
    }

    if candidates.is_empty() {
        Err(GaalError::Internal(
            "Could not detect current session. Provide a session ID, use 'today', or run from within a Claude Code session.".to_string(),
        ))
    } else {
        Ok(candidates)
    }
}

fn detect_current_session() -> Result<DetectedSession, GaalError> {
    detect_session_candidates()?
        .into_iter()
        .next()
        .ok_or_else(|| {
            GaalError::Internal(
                "Could not detect current session. Provide a session ID, use 'today', or run from within a Claude Code session.".to_string(),
            )
        })
}

/// Parent-child preference is permanently disabled.
fn detect_preferred_session() -> Result<DetectedSession, GaalError> {
    detect_current_session()
}

fn get_ppid(pid: u32) -> Option<u32> {
    let output = Command::new("ps")
        .args(["-o", "ppid=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<u32>()
        .ok()
}

fn get_process_name(pid: u32) -> Option<String> {
    let output = Command::new("ps")
        .args(["-o", "comm=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if raw.is_empty() {
        return None;
    }
    Some(raw.rsplit('/').next().map(str::to_string).unwrap_or(raw))
}

fn resolve_jsonl_for_pid(pid: u32) -> Option<PathBuf> {
    let output = Command::new("lsof")
        .args(["-p", &pid.to_string(), "-Ffn"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut candidate: Option<PathBuf> = None;
    for line in stdout.lines() {
        let Some(path) = line.strip_prefix('n') else {
            continue;
        };
        if !path.ends_with(".jsonl") {
            continue;
        }
        candidate = Some(PathBuf::from(path));
    }
    candidate
}

fn resolve_jsonl_via_cwd(pid: u32, engine: &str) -> Option<PathBuf> {
    let cwd = resolve_cwd_for_pid(pid)?;
    let home = dirs::home_dir()?;

    match engine {
        "claude" => {
            let projects_root = home.join(".claude").join("projects");
            let encoded = cwd.replace('/', "-");
            if let Some(path) = latest_jsonl_in_dir(&projects_root.join(&encoded)) {
                return Some(path);
            }
            if let Ok(real) = fs::canonicalize(&cwd) {
                if let Some(real_str) = real.to_str() {
                    let encoded_real = real_str.replace('/', "-");
                    if let Some(path) = latest_jsonl_in_dir(&projects_root.join(encoded_real)) {
                        return Some(path);
                    }
                }
            }
            None
        }
        "codex" => {
            let sessions_dir = home.join(".codex").join("sessions");
            latest_jsonl_in_dir(&sessions_dir)
        }
        _ => None,
    }
}

fn resolve_cwd_for_pid(pid: u32) -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let output = Command::new("lsof")
            .args(["-p", &pid.to_string(), "-Ffn"])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let lines: Vec<&str> = stdout.lines().collect();
        for (idx, line) in lines.iter().enumerate() {
            if *line == "fcwd" {
                if let Some(next) = lines.get(idx + 1) {
                    if let Some(path) = next.strip_prefix('n') {
                        return Some(path.to_string());
                    }
                }
            }
            if let Some(rest) = line.strip_prefix("fcwd") {
                if !rest.is_empty() {
                    return Some(rest.to_string());
                }
            }
        }
        None
    }
    #[cfg(target_os = "linux")]
    {
        let output = Command::new("readlink")
            .arg(format!("/proc/{pid}/cwd"))
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let cwd = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if cwd.is_empty() {
            None
        } else {
            Some(cwd)
        }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        None
    }
}

fn latest_jsonl_in_dir(dir: &Path) -> Option<PathBuf> {
    let entries = fs::read_dir(dir).ok()?;
    let mut newest: Option<(std::time::SystemTime, PathBuf)> = None;

    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(ft) = entry.file_type() else { continue };
        if !ft.is_file() {
            continue;
        }
        let is_jsonl = path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.eq_ignore_ascii_case("jsonl"))
            .unwrap_or(false);
        if !is_jsonl {
            continue;
        }
        let modified = fs::metadata(&path)
            .ok()
            .and_then(|m| m.modified().ok())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        match &newest {
            Some((best, _)) if modified <= *best => {}
            _ => newest = Some((modified, path)),
        }
    }
    newest.map(|(_, path)| path)
}

fn extract_session_id_from_jsonl(path: &Path, engine: &str) -> Option<String> {
    let file = File::open(path).ok()?;
    let reader = BufReader::new(file);

    for line in reader.lines().take(30).flatten() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            continue;
        };

        match engine {
            "claude" => {
                if let Some(id) = value
                    .get("sessionId")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
                {
                    return Some(id);
                }
            }
            "codex" => {
                if let Some(id) = value
                    .pointer("/payload/id")
                    .or_else(|| value.get("session_id"))
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
                {
                    return Some(id);
                }
            }
            _ => {}
        }
    }

    None
}

/// Index a single JSONL file on-the-fly so the current (active) session
/// becomes available in the DB for handoff. This bridges the timing gap
/// where the cron indexer hasn't picked up the session yet.
///
/// Returns the short ID stored in the DB (for retry lookup).
fn index_single_jsonl(
    conn: &mut Connection,
    detected: &DetectedSession,
) -> Result<String, GaalError> {
    let meta = fs::metadata(&detected.jsonl_path).map_err(GaalError::from)?;
    let engine = Engine::from_str(&detected.engine)?;
    let short_id = truncate_session_id(&detected.session_id, &engine);

    let discovered = DiscoveredSession {
        id: short_id.clone(),
        engine,
        path: detected.jsonl_path.clone(),
        model: None,
        cwd: None,
        started_at: None,
        file_size: meta.len(),
    };

    match index_discovered_session(conn, &discovered, true) {
        Ok(IndexOutcome::Indexed) => {
            eprintln!("On-the-fly index complete for session {}", discovered.id);
            Ok(short_id)
        }
        Ok(IndexOutcome::Skipped) => {
            eprintln!("Session {} already indexed (skipped)", discovered.id);
            Ok(short_id)
        }
        Err(err) => {
            eprintln!("On-the-fly indexing failed: {err}");
            Err(err)
        }
    }
}

/// Truncate a session ID to match the short-ID convention used by the indexer.
///
/// Claude (UUIDv4): first 8 characters.
/// Codex (UUIDv7): last 8 hex characters (dashes stripped).
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

fn load_prompt(path: &Path) -> Result<String, GaalError> {
    let resolved = expand_home(path);
    match fs::read_to_string(resolved) {
        Ok(content) => Ok(content),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            Ok(DEFAULT_HANDOFF_PROMPT.to_string())
        }
        Err(err) => Err(GaalError::Io(err)),
    }
}

/// Attempt to locate and read a session markdown transcript.
///
/// Checks three sources in priority order:
/// 1. External output directory (e.g. pratchett-os/data/claude-code-sessions/) via config
/// 2. Gaal's own rendered markdown (~/.gaal/data/{engine}/sessions/YYYY/MM/DD/{id}.md)
/// 3. On-the-fly render from the session's JSONL file
///
/// Returns `None` if all sources fail.
fn resolve_session_transcript(session: &SessionRow, config: &GaalConfig) -> Option<String> {
    let short_id: String = session.id.chars().take(8).collect();
    let (year, month, day) = date_parts(&session.started_at);

    // 1. Gaal's own session markdown directory (kept fresh by cron backfill)
    let gaal_md_path = gaal_home()
        .join("data")
        .join(&session.engine)
        .join("sessions")
        .join(&year)
        .join(&month)
        .join(&day)
        .join(format!("{short_id}.md"));
    if let Ok(content) = fs::read_to_string(&gaal_md_path) {
        if !content.trim().is_empty() {
            eprintln!("  -> transcript source: {}", gaal_md_path.display());
            return Some(content);
        }
    }

    // 2. External output directory (config.markdown_output_dir) — fallback
    if let Some(ref output_dir) = config.markdown_output_dir {
        let external_path = output_dir
            .join(&year)
            .join(&month)
            .join(&day)
            .join(format!("{short_id}.md"));
        if let Ok(content) = fs::read_to_string(&external_path) {
            if !content.trim().is_empty() {
                eprintln!("  -> transcript source: {}", external_path.display());
                return Some(content);
            }
        }
    }

    // 3. On-the-fly render from JSONL (if path exists and is readable)
    let jsonl_path = Path::new(&session.jsonl_path);
    if jsonl_path.exists() {
        match crate::render::session_md::render_session_markdown(jsonl_path) {
            Ok(content) if !content.trim().is_empty() => {
                eprintln!(
                    "  -> transcript source: rendered from {}",
                    jsonl_path.display()
                );
                return Some(content);
            }
            Ok(_) => {}
            Err(e) => {
                eprintln!("  -> on-the-fly render failed: {e}");
            }
        }
    }

    None
}

/// Build LLM context from a full session markdown transcript.
///
/// Wraps the transcript with the same session metadata header used by
/// `build_context()` so the extraction prompt sees engine/model ground truth.
fn build_context_from_transcript(
    session: &SessionRow,
    transcript: &str,
    provider: &str,
    format: &str,
) -> String {
    let engine = &session.engine;
    let model = session.model.as_deref().unwrap_or("unknown");

    format!(
        "Requested provider: {provider}\nRequested format: {format}\n\n\
GROUND TRUTH (do not override in your output):\n\
- engine: {engine}\n\
- model: {model}\n\
These values are determined from the session source. Do not infer or hallucinate different engine/model values.\n\n\
Session Summary:\n\
- id: {id}\n\
- engine: {engine}\n\
- model: {model}\n\
- cwd: {cwd}\n\
- started_at: {started_at}\n\
- ended_at: {ended_at}\n\
- total_input_tokens: {input_tokens}\n\
- total_output_tokens: {output_tokens}\n\
- total_tools: {tools}\n\
- total_turns: {turns}\n\n\
--- FULL SESSION TRANSCRIPT ---\n\n\
{transcript}\n",
        id = session.id,
        cwd = session.cwd.as_deref().unwrap_or("."),
        started_at = session.started_at,
        ended_at = session.ended_at.as_deref().unwrap_or("in_progress"),
        input_tokens = session.total_input_tokens,
        output_tokens = session.total_output_tokens,
        tools = session.total_tools,
        turns = session.total_turns
    )
}

fn build_context(session: &SessionRow, facts: &[Fact], provider: &str, format: &str) -> String {
    let mut commands = Vec::new();
    let mut errors = Vec::new();
    let mut files = Vec::new();
    let mut decisions = Vec::new();

    for fact in facts {
        let line = format_fact(fact);
        match &fact.fact_type {
            FactType::Command => commands.push(line),
            FactType::Error => errors.push(line),
            FactType::FileRead | FactType::FileWrite => files.push(line),
            FactType::AssistantReply | FactType::UserPrompt | FactType::GitOp => {
                if looks_like_decision(fact) {
                    decisions.push(line);
                }
            }
            FactType::TaskSpawn => {}
        }
    }

    let command_block = bullet_lines(&commands, 40);
    let error_block = bullet_lines(&errors, 20);
    let file_block = bullet_lines(&files, 40);
    let decision_block = bullet_lines(&decisions, 20);

    let engine = &session.engine;
    let model = session.model.as_deref().unwrap_or("unknown");

    format!(
        "Requested provider: {provider}\nRequested format: {format}\n\n\
GROUND TRUTH (do not override in your output):\n\
- engine: {engine}\n\
- model: {model}\n\
These values are determined from the session source. Do not infer or hallucinate different engine/model values.\n\n\
Session Summary:\n\
- id: {id}\n\
- engine: {engine}\n\
- model: {model}\n\
- cwd: {cwd}\n\
- started_at: {started_at}\n\
- ended_at: {ended_at}\n\
- total_input_tokens: {input_tokens}\n\
- total_output_tokens: {output_tokens}\n\
- total_tools: {tools}\n\
- total_turns: {turns}\n\n\
Commands:\n{command_block}\n\n\
Errors:\n{error_block}\n\n\
Files:\n{file_block}\n\n\
Key Decisions:\n{decision_block}\n",
        id = session.id,
        cwd = session.cwd.as_deref().unwrap_or("."),
        started_at = session.started_at,
        ended_at = session.ended_at.as_deref().unwrap_or("in_progress"),
        input_tokens = session.total_input_tokens,
        output_tokens = session.total_output_tokens,
        tools = session.total_tools,
        turns = session.total_turns
    )
}

fn format_fact(fact: &Fact) -> String {
    let ts = fact.ts.as_str();
    let subject = fact.subject.as_deref().unwrap_or("-");
    let detail = fact.detail.as_deref().unwrap_or("-");
    let snippet = truncate(detail, 400);
    format!("[{ts}] {subject} | {snippet}")
}

fn looks_like_decision(fact: &Fact) -> bool {
    let mut haystack = String::new();
    if let Some(subject) = &fact.subject {
        haystack.push_str(subject);
        haystack.push(' ');
    }
    if let Some(detail) = &fact.detail {
        haystack.push_str(detail);
    }
    let text = haystack.to_ascii_lowercase();
    [
        "decid", "choose", "selected", "plan", "will", "next", "use ", "switch", "migrate",
    ]
    .iter()
    .any(|needle| text.contains(needle))
}

fn bullet_lines(values: &[String], max: usize) -> String {
    if values.is_empty() {
        return "- (none)".to_string();
    }
    let len = values.len();
    if len <= max {
        return values
            .iter()
            .map(|line| format!("- {line}"))
            .collect::<Vec<_>>()
            .join("\n");
    }
    // Take head (1/4) + tail (3/4) so the execution phase is always preserved.
    let head_count = max / 4;
    let tail_count = max - head_count;
    let skipped = len - head_count - tail_count;
    let mut lines: Vec<String> = values
        .iter()
        .take(head_count)
        .map(|line| format!("- {line}"))
        .collect();
    lines.push(format!("[... {skipped} items skipped ...]"));
    lines.extend(
        values
            .iter()
            .skip(len - tail_count)
            .map(|line| format!("- {line}")),
    );
    lines.join("\n")
}

fn invoke_agent_mux(
    mux_config: &AgentMuxConfig,
    engine: &str,
    model: &str,
    cwd: &str,
    prompt: &str,
    context: &str,
    timeout_secs: u64,
) -> Result<String, GaalError> {
    let request = format!("{prompt}\n\n---\n\nSession context:\n{context}");
    let mux_timeout_secs = timeout_secs.saturating_sub(5).max(10);
    let role = mux_config
        .role
        .as_deref()
        .map(str::trim)
        .filter(|role| !role.is_empty());
    let variant = mux_config
        .variant
        .as_deref()
        .map(str::trim)
        .filter(|variant| !variant.is_empty());
    let effort = mux_config
        .effort
        .as_deref()
        .map(str::trim)
        .filter(|effort| !effort.is_empty());

    // When a role is used, agent-mux v2 resolves skills relative to --cwd.
    // The config cwd override ensures skills are found regardless of the session's cwd.
    // Falls back to the session's cwd if no override is configured.
    let effective_cwd = mux_config
        .cwd
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .unwrap_or(cwd);

    let mut command = Command::new(&mux_config.path);
    if let Some(role) = role {
        command.arg("-R").arg(role);
        if let Some(variant) = variant {
            command.arg("--variant").arg(variant);
        }
    } else {
        command
            .arg("--engine")
            .arg(engine)
            .arg("--model")
            .arg(model);
    }
    // Pass --effort in both role and non-role paths
    if let Some(effort) = effort {
        command.arg("--effort").arg(effort);
    }

    // Override response truncation — handoff documents with the JSON metadata
    // block typically need 4000-8000 chars, exceeding agent-mux's default
    // response_max_chars (4000). Without this override, the response gets
    // truncated and the JSON metadata block at the end is lost.
    // Note: -1 (unlimited) is not reliably honored by agent-mux v2's config
    // merge, so we use an explicit large value instead.
    let child = command
        .arg("--timeout")
        .arg(mux_timeout_secs.to_string())
        .arg("--cwd")
        .arg(effective_cwd)
        .arg("--response-max-chars")
        .arg("50000")
        .arg(request)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(GaalError::from)?;

    #[cfg(unix)]
    let _stdout_clone = child
        .stdout
        .as_ref()
        .ok_or_else(|| GaalError::Internal("failed to open stdout for agent-mux".to_string()))?
        .as_fd()
        .try_clone_to_owned()
        .map_err(GaalError::from)?;

    #[cfg(unix)]
    let _stderr_clone = child
        .stderr
        .as_ref()
        .ok_or_else(|| GaalError::Internal("failed to open stderr for agent-mux".to_string()))?
        .as_fd()
        .try_clone_to_owned()
        .map_err(GaalError::from)?;

    let child_pid = child.id();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });

    let output = match rx.recv_timeout(Duration::from_secs(timeout_secs)) {
        Ok(result) => result.map_err(GaalError::from)?,
        Err(mpsc::RecvTimeoutError::Timeout) => {
            let _ = terminate_process(child_pid);
            return Err(GaalError::Other(anyhow!(
                "agent-mux timed out after {timeout_secs}s"
            )));
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            return Err(GaalError::Other(anyhow!(
                "agent-mux worker thread disconnected unexpectedly"
            )));
        }
    };

    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if let Ok(value) = serde_json::from_str::<Value>(&stdout) {
            if let Some(stdout_error) = extract_agent_mux_error(&value) {
                let message = if stderr.is_empty() {
                    format!("agent-mux failed: {stdout_error}")
                } else {
                    format!("agent-mux failed: {stdout_error}; stderr: {stderr}")
                };
                return Err(GaalError::Other(anyhow!(message)));
            }
        }
        let message = if stderr.is_empty() {
            "agent-mux command failed".to_string()
        } else {
            format!("agent-mux failed: {stderr}")
        };
        return Err(GaalError::Other(anyhow!(message)));
    }

    let stdout = String::from_utf8(output.stdout)
        .map_err(|e| GaalError::ParseError(format!("agent-mux output was not valid UTF-8: {e}")))?;
    parse_agent_mux_response(&stdout)
}

fn terminate_process(pid: u32) -> Result<(), GaalError> {
    #[cfg(unix)]
    {
        let status = Command::new("kill")
            .arg("-9")
            .arg(pid.to_string())
            .status()
            .map_err(GaalError::from)?;
        if !status.success() {
            return Err(GaalError::Other(anyhow!(
                "failed to terminate agent-mux process {pid}"
            )));
        }
        Ok(())
    }

    #[cfg(windows)]
    {
        let status = Command::new("taskkill")
            .arg("/PID")
            .arg(pid.to_string())
            .arg("/F")
            .status()
            .map_err(GaalError::from)?;
        if !status.success() {
            return Err(GaalError::Other(anyhow!(
                "failed to terminate agent-mux process {pid}"
            )));
        }
        Ok(())
    }
}

fn parse_agent_mux_response(stdout: &str) -> Result<String, GaalError> {
    if let Ok(value) = serde_json::from_str::<Value>(stdout.trim()) {
        return value_to_response(value);
    }

    for line in stdout.lines().rev() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
            return value_to_response(value);
        }
    }

    Err(GaalError::ParseError(
        "agent-mux returned non-JSON output".to_string(),
    ))
}

fn value_to_response(value: Value) -> Result<String, GaalError> {
    if value.get("schema_version").is_some() {
        return match value.get("status").and_then(Value::as_str) {
            Some("completed") => extract_agent_mux_response(&value).ok_or_else(|| {
                GaalError::ParseError("agent-mux JSON output missing `response` field".to_string())
            }),
            Some("timed_out") => {
                let mut message = "agent-mux timed out".to_string();
                if let Some(partial) = value.get("partial").and_then(Value::as_bool) {
                    message.push_str(&format!("; partial={partial}"));
                }
                if let Some(recoverable) = value.get("recoverable").and_then(Value::as_bool) {
                    message.push_str(&format!("; recoverable={recoverable}"));
                }
                if let Some(response) = extract_agent_mux_response(&value) {
                    message.push_str(&format!("; partial response: {response}"));
                }
                Err(GaalError::Other(anyhow!(message)))
            }
            Some("failed") => {
                let error = extract_agent_mux_error(&value)
                    .unwrap_or_else(|| "agent-mux reported failure".to_string());
                Err(GaalError::Other(anyhow!(error)))
            }
            Some(status) => Err(GaalError::ParseError(format!(
                "agent-mux returned unknown status `{status}`"
            ))),
            None => Err(GaalError::ParseError(
                "agent-mux JSON output missing `status` field".to_string(),
            )),
        };
    }

    if value.get("timed_out").and_then(Value::as_bool) == Some(true) {
        let mut message = "agent-mux timed out".to_string();
        if let Some(response) = extract_agent_mux_response(&value) {
            message.push_str(&format!("; partial response: {response}"));
        }
        return Err(GaalError::Other(anyhow!(message)));
    }

    if value.get("completed").and_then(Value::as_bool) == Some(false) {
        let mut message = "agent-mux did not complete".to_string();
        if let Some(error) = extract_agent_mux_error(&value) {
            message.push_str(&format!("; {error}"));
        }
        return Err(GaalError::Other(anyhow!(message)));
    }

    if value.get("success").and_then(Value::as_bool) == Some(false) {
        let error = extract_agent_mux_error(&value)
            .unwrap_or_else(|| "agent-mux reported failure".to_string());
        return Err(GaalError::Other(anyhow!(error)));
    }

    extract_agent_mux_response(&value).ok_or_else(|| {
        GaalError::ParseError("agent-mux JSON output missing `response` field".to_string())
    })
}

fn extract_agent_mux_response(value: &Value) -> Option<String> {
    value
        .get("response")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            value
                .pointer("/data/response")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .or_else(|| {
            value
                .get("content")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
}

fn extract_agent_mux_error(value: &Value) -> Option<String> {
    if value.get("schema_version").is_some() {
        if value.get("status").and_then(Value::as_str) != Some("failed")
            && value.get("status").and_then(Value::as_str) != Some("timed_out")
        {
            return None;
        }

        let message = value
            .pointer("/error/message")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|message| !message.is_empty())
            .map(str::to_string);
        let suggestion = value
            .pointer("/error/suggestion")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|suggestion| !suggestion.is_empty())
            .map(str::to_string);

        return match (message, suggestion) {
            (Some(message), Some(suggestion)) => {
                Some(format!("{message} Suggestion: {suggestion}"))
            }
            (Some(message), None) => Some(message),
            (None, Some(suggestion)) => Some(format!("Suggestion: {suggestion}")),
            (None, None) => None,
        };
    }

    if value.get("success").and_then(Value::as_bool) != Some(false) {
        return None;
    }

    value
        .get("error")
        .or_else(|| value.pointer("/data/error"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|error| !error.is_empty())
        .map(str::to_string)
}

fn build_generated_by_label(mux_config: &AgentMuxConfig, engine: &str, model: &str) -> String {
    let role = mux_config
        .role
        .as_deref()
        .map(str::trim)
        .filter(|role| !role.is_empty());
    let variant = mux_config
        .variant
        .as_deref()
        .map(str::trim)
        .filter(|variant| !variant.is_empty());
    let effort = mux_config
        .effort
        .as_deref()
        .map(str::trim)
        .filter(|effort| !effort.is_empty());

    if let Some(role) = role {
        let mut label = format!("agent-mux-v2 -R={role}");
        if let Some(variant) = variant {
            label.push_str(&format!(" --variant={variant}"));
        }
        if let Some(effort) = effort {
            label.push_str(&format!(" --effort={effort}"));
        }

        let resolved = match effort {
            Some(effort) => format!("{engine}/{model}/{effort}"),
            None => format!("{engine}/{model}"),
        };
        label.push_str(&format!(" ({resolved})"));
        return label;
    }

    format!("{engine}/{model}")
}

/// Build YAML frontmatter for handoff markdown files.
fn build_handoff_frontmatter(
    session: &SessionRow,
    extracted: &ExtractedMetadata,
    engine: &str,
    model: &str,
) -> String {
    // Session ID: first 8 chars
    let sid: String = session.id.chars().take(8).collect();

    // Date from started_at (RFC3339)
    let date_str = DateTime::parse_from_rfc3339(&session.started_at)
        .map(|dt| dt.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|_| "unknown".to_string());

    // Duration in human format "2h 24m" / "45m" / "0m"
    let dur_mins = duration_minutes(session);
    let duration_str = if dur_mins >= 60 {
        format!("{}h {}m", dur_mins / 60, dur_mins % 60)
    } else {
        format!("{}m", dur_mins)
    };

    // Simplified model name using same logic as render/session_md.rs
    let model_simple = simplify_model_name(model);

    // Engine: "claude" or "codex"
    let engine_str = if engine.contains("codex") {
        "codex"
    } else {
        "claude"
    };

    // Headline - quote if it contains YAML special chars
    let headline = extracted.headline.as_deref().unwrap_or("Untitled session");
    let headline_formatted = if headline.contains(':')
        || headline.contains('#')
        || headline.contains('&')
        || headline.contains('*')
        || headline.contains('!')
        || headline.contains('|')
        || headline.contains('"')
        || headline.contains('\'')
        || headline.contains('%')
        || headline.contains('@')
        || headline.contains('<')
        || headline.contains('>')
        || headline.contains('{')
        || headline.contains('}')
        || headline.contains('[')
        || headline.contains(']')
    {
        // Use double quotes, escape any internal double quotes
        format!("\"{}\"", headline.replace('"', "\\\""))
    } else {
        headline.to_string()
    };

    // Projects as YAML inline list
    let projects_str = format!("[{}]", extracted.projects.join(", "));

    // Keywords as YAML inline list
    let keywords_str = format!("[{}]", extracted.keywords.join(", "));

    // Substance score
    let substance = extracted.substance;

    format!(
        "---\nsession_id: {sid}\ndate: {date_str}\nduration: {duration_str}\nmodel: {model_simple}\nengine: {engine_str}\nheadline: {headline_formatted}\nprojects: {projects_str}\nkeywords: {keywords_str}\nsubstance: {substance}\n---\n\n"
    )
}

/// Simplify model name to human-readable form.
fn simplify_model_name(model: &str) -> String {
    let lower = model.to_lowercase();
    if lower.contains("opus") {
        "Opus".to_string()
    } else if lower.contains("sonnet") {
        "Sonnet".to_string()
    } else if lower.contains("haiku") {
        "Haiku".to_string()
    } else if lower.contains("codex") {
        if let Some(pos) = lower.find("codex") {
            let prefix = model[..pos].trim_end_matches('-');
            if prefix.is_empty() {
                "Codex".to_string()
            } else {
                format!("{} Codex", prefix.to_uppercase())
            }
        } else {
            "Codex".to_string()
        }
    } else if lower.contains("o4-mini") {
        "o4-mini".to_string()
    } else {
        model.to_string()
    }
}

fn write_handoff_markdown(session: &SessionRow, content: &str) -> Result<PathBuf, GaalError> {
    let (year, month, day) = date_parts(&session.started_at);
    let path = gaal_home()
        .join("data")
        .join(&session.engine)
        .join("handoffs")
        .join(year)
        .join(month)
        .join(day)
        .join(format!(
            "{}.md",
            crate::util::sanitize_filename(&session.id)
                .chars()
                .take(8)
                .collect::<String>()
        ));

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(GaalError::from)?;
    }
    crate::util::atomic_write(&path, content).map_err(GaalError::from)?;
    Ok(path)
}

fn date_parts(started_at: &str) -> (String, String, String) {
    let fallback = || {
        let now = Local::now();
        (
            now.format("%Y").to_string(),
            now.format("%m").to_string(),
            now.format("%d").to_string(),
        )
    };

    let Some(prefix) = started_at.get(0..10) else {
        return fallback();
    };
    let mut parts = prefix.split('-');
    let year = parts.next().unwrap_or_default();
    let month = parts.next().unwrap_or_default();
    let day = parts.next().unwrap_or_default();

    if year.len() == 4 && month.len() == 2 && day.len() == 2 {
        (year.to_string(), month.to_string(), day.to_string())
    } else {
        fallback()
    }
}

fn duration_minutes(session: &SessionRow) -> i32 {
    let Some(ended_at) = session.ended_at.as_deref() else {
        return 0;
    };

    let started = DateTime::parse_from_rfc3339(&session.started_at);
    let ended = DateTime::parse_from_rfc3339(ended_at);
    match (started, ended) {
        (Ok(started_ts), Ok(ended_ts)) => {
            let mins = ended_ts.signed_duration_since(started_ts).num_minutes();
            if mins < 0 {
                0
            } else {
                mins as i32
            }
        }
        _ => 0,
    }
}

fn extract_metadata(response: &str) -> ExtractedMetadata {
    if let Ok(value) = serde_json::from_str::<Value>(response.trim()) {
        return extract_json_metadata(value).unwrap_or_else(|| extract_text_metadata(response));
    }

    if let Some(captures) = FENCED_JSON_RE.captures(response) {
        if let Some(raw_json) = captures.get(1).map(|capture| capture.as_str()) {
            if let Ok(value) = serde_json::from_str::<Value>(raw_json) {
                if let Some(metadata) = extract_json_metadata(value) {
                    return metadata;
                }
            }
        }
    }

    extract_text_metadata(response)
}

fn extract_json_metadata(value: Value) -> Option<ExtractedMetadata> {
    let Value::Object(map) = value else {
        return None;
    };

    let headline = map
        .get("headline")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    let projects = extract_string_array(map.get("projects"));
    let keywords = extract_string_array(map.get("keywords"));
    let substance = map
        .get("substance")
        .and_then(Value::as_i64)
        .map(|v| v.clamp(0, 3) as i32)
        .unwrap_or(0);

    Some(ExtractedMetadata {
        headline,
        projects,
        keywords,
        substance,
    })
}

fn extract_text_metadata(response: &str) -> ExtractedMetadata {
    let headline = extract_heading_value(response, "Headline")
        .or_else(|| first_nonempty_line(response).map(str::to_string));
    let projects = extract_named_list(response, "projects");
    let keywords = extract_named_list(response, "keywords");
    let substance = extract_substance(response);

    ExtractedMetadata {
        headline,
        projects,
        keywords,
        substance,
    }
}

fn extract_heading_value(text: &str, heading: &str) -> Option<String> {
    let mut in_section = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("##") {
            if in_section {
                break;
            }
            let name = trimmed.trim_start_matches('#').trim();
            in_section = name.eq_ignore_ascii_case(heading);
            continue;
        }
        if in_section && !trimmed.is_empty() {
            let cleaned = trimmed
                .trim_start_matches('-')
                .trim_start_matches('*')
                .trim();
            if !cleaned.is_empty() {
                return Some(cleaned.to_string());
            }
        }
    }
    None
}

fn extract_named_list(text: &str, name: &str) -> Vec<String> {
    for line in text.lines() {
        let trimmed = line.trim();
        let lower = trimmed.to_ascii_lowercase();
        let prefix = format!("{name}:");
        if lower.starts_with(&prefix) {
            let raw = trimmed[prefix.len()..].trim();
            let parsed = parse_list(raw);
            if !parsed.is_empty() {
                return parsed;
            }
        }
    }

    let target = format!("## {name}");
    let mut in_section = false;
    let mut out = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("##") {
            if in_section {
                break;
            }
            in_section = trimmed.to_ascii_lowercase() == target;
            continue;
        }
        if !in_section {
            continue;
        }
        if let Some(item) = trim_bullet(trimmed) {
            out.push(item);
        }
    }
    out
}

fn parse_list(raw: &str) -> Vec<String> {
    if raw.is_empty() {
        return Vec::new();
    }

    if raw.starts_with('[') && raw.ends_with(']') {
        if let Ok(value) = serde_json::from_str::<Value>(raw) {
            return extract_string_array(Some(&value));
        }
    }

    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

fn trim_bullet(line: &str) -> Option<String> {
    let trimmed = line.trim();
    let value = if trimmed.starts_with("- ") || trimmed.starts_with("* ") {
        trimmed[2..].trim()
    } else {
        return None;
    };
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn extract_substance(text: &str) -> i32 {
    for line in text.lines() {
        let lower = line.to_ascii_lowercase();
        if !lower.contains("substance") {
            continue;
        }
        for ch in lower.chars() {
            if ('0'..='3').contains(&ch) {
                return (ch as u8 - b'0') as i32;
            }
        }
    }
    0
}

fn first_nonempty_line(text: &str) -> Option<&str> {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.trim_start_matches('#').trim())
}

fn extract_string_array(value: Option<&Value>) -> Vec<String> {
    let Some(value) = value else {
        return Vec::new();
    };
    let Some(items) = value.as_array() else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect()
}

fn truncate(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    value.chars().take(max_chars).collect()
}

fn expand_home(path: &Path) -> PathBuf {
    let raw = path.to_string_lossy();
    if let Some(rest) = raw.strip_prefix("~/") {
        return gaal_home()
            .parent()
            .map(|home| home.join(rest))
            .unwrap_or_else(|| path.to_path_buf());
    }
    path.to_path_buf()
}
