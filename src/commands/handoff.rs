#![allow(clippy::items_after_test_module)]

use std::collections::HashSet;
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
use crate::db::queries::{get_facts, get_session, upsert_handoff, SessionRow};
use crate::db::{open_db, open_db_readonly};
use crate::discovery::DiscoveredSession;
use crate::error::GaalError;
use crate::model::{Fact, FactType, HandoffRecord};
use crate::output::json::print_json;
use crate::parser::{detect_engine, types::Engine};

/// Built-in fallback extraction prompt used when no prompt file is available.
const DEFAULT_HANDOFF_PROMPT: &str = r#"You are analyzing an agent session trace. Extract:
## Headline (one-line summary)
## What Happened (structured bullet summary of key actions)
## Key Decisions (decisions made)
## Open Threads (unfinished work)
## Key Files (files created/modified with descriptions)
Also extract: projects (list), keywords (list), substance score (0-3)."#;

const SINGLE_CONTEXT_LIMIT_CHARS: u64 = 80_000;
const MAX_CHUNKS: usize = 8;
const MAX_LLM_CALLS_PER_SESSION: usize = 9;
const ESTIMATED_CHARS_PER_TOKEN: u64 = 4;

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
    /// Print a compact human-readable dry-run plan.
    pub human: bool,
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

#[derive(Debug, Serialize)]
struct HandoffDryRunResult {
    session_id: String,
    status: String,
    engine: String,
    indexed: bool,
    jsonl_path: String,
    handoff_path: String,
    strategy: String,
    estimated_transcript_chars: u64,
    estimated_transcript_tokens: u64,
    jsonl_lines: usize,
    compaction_lines: Vec<usize>,
    chunk_count: usize,
    estimated_llm_calls: usize,
    single_context_limit_chars: u64,
    max_chunks: usize,
    max_llm_calls_per_session: usize,
    provider: String,
    provider_supported: bool,
    model: String,
    effort: String,
    format: String,
    session_model: Option<String>,
    side_effects: DryRunSideEffects,
    warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
struct DryRunSideEffects {
    spawn_provider_worker: bool,
    spend_tokens: bool,
    write_handoff_markdown: bool,
    upsert_db_rows: bool,
    index_jsonl: bool,
}

impl DryRunSideEffects {
    fn none() -> Self {
        Self {
            spawn_provider_worker: false,
            spend_tokens: false,
            write_handoff_markdown: false,
            upsert_db_rows: false,
            index_jsonl: false,
        }
    }
}

#[derive(Debug, Clone)]
struct DryRunSession {
    id: String,
    engine: String,
    model: Option<String>,
    started_at: String,
    jsonl_path: PathBuf,
    indexed: bool,
}

#[derive(Debug, Clone)]
struct JsonlPlanStats {
    chars: u64,
    lines: usize,
    compaction_lines: Vec<usize>,
    first_timestamp: Option<String>,
}

#[derive(Debug, Clone)]
struct ExecutionPlan {
    strategy: String,
    chunks: Vec<ChunkPlan>,
    jsonl_lines: usize,
    compaction_lines: Vec<usize>,
}

impl ExecutionPlan {
    fn is_chunked(&self) -> bool {
        self.strategy != "single"
    }
}

#[derive(Debug, Clone)]
struct ChunkPlan {
    index: usize,
    total: usize,
    source_start_line: usize,
    source_end_line: usize,
}

#[derive(Debug, Clone)]
struct ChunkContext {
    plan: ChunkPlan,
    body: String,
    source_kind: &'static str,
    rendered_start_line: Option<usize>,
    rendered_end_line: Option<usize>,
}

#[derive(Debug, Clone)]
struct ChunkMapResult {
    plan: ChunkPlan,
    status: String,
    response: String,
    source_kind: &'static str,
    rendered_start_line: Option<usize>,
    rendered_end_line: Option<usize>,
    context_chars: usize,
}

#[derive(Clone, Copy)]
struct DryRunRequest<'a> {
    model: &'a str,
    provider: &'a str,
    format: &'a str,
    effort: &'a str,
}

#[derive(Clone, Copy)]
struct HandoffRequest<'a> {
    engine: &'a str,
    model: &'a str,
    prompt: &'a str,
    provider: &'a str,
    format: &'a str,
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
        let current = config
            .agent_mux
            .timeout_secs
            .unwrap_or(config.llm.timeout_secs);
        if current < min_timeout {
            config.agent_mux.timeout_secs = Some(min_timeout);
        }
    }

    let mut conn = open_db()?;
    if args.batch {
        return run_batch(&conn, &config, &args);
    }

    let (id_or_today, detected) = if let Some(ref jsonl_path) = args.jsonl {
        let engine_name = infer_source_engine_from_jsonl(jsonl_path);
        let session_id = extract_session_id_from_jsonl(jsonl_path, &engine_name)
            .or_else(|| {
                jsonl_path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .map(str::to_string)
            })
            .ok_or_else(|| GaalError::ParseError("invalid --jsonl path".into()))?;
        let detected = DetectedSession {
            engine: engine_name,
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

    if args.dry_run {
        let plans = plan_single_session_dry_run(
            &conn,
            &args,
            &id_or_today,
            detected.as_ref(),
            DryRunRequest {
                model: &model,
                provider: &provider,
                format: &format,
                effort: effective_handoff_effort(&config),
            },
        )?;
        if args.human {
            print_handoff_dry_run_human(&plans);
            return Ok(());
        }
        return print_json(&plans).map_err(GaalError::from);
    }

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
    let handoff_request = HandoffRequest {
        engine: &engine,
        model: &model,
        prompt: &prompt,
        provider: &provider,
        format: &format,
    };
    for session in sessions {
        let processed = process_session_handoff(&conn, &config, &session, handoff_request)?;

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

fn plan_single_session_dry_run(
    conn: &Connection,
    args: &HandoffArgs,
    id_or_today: &str,
    detected: Option<&DetectedSession>,
    request: DryRunRequest<'_>,
) -> Result<Vec<HandoffDryRunResult>, GaalError> {
    if let Some(jsonl_path) = args.jsonl.as_deref() {
        let session = dry_run_session_from_jsonl(conn, jsonl_path, None)?;
        return Ok(vec![build_dry_run_plan(conn, &session, request)?]);
    }

    match resolve_sessions(conn, id_or_today) {
        Ok(sessions) => sessions
            .iter()
            .map(|session| build_dry_run_plan(conn, &DryRunSession::from(session), request))
            .collect(),
        Err(GaalError::NotFound(_)) if detected.is_some() => {
            let detected = detected.expect("checked is_some");
            let session = dry_run_session_from_detected(conn, detected)?;
            Ok(vec![build_dry_run_plan(conn, &session, request)?])
        }
        Err(err) => Err(err),
    }
}

impl From<&SessionRow> for DryRunSession {
    fn from(session: &SessionRow) -> Self {
        Self {
            id: session.id.clone(),
            engine: session.engine.clone(),
            model: session.model.clone(),
            started_at: session.started_at.clone(),
            jsonl_path: PathBuf::from(&session.jsonl_path),
            indexed: true,
        }
    }
}

fn dry_run_session_from_detected(
    conn: &Connection,
    detected: &DetectedSession,
) -> Result<DryRunSession, GaalError> {
    if let Some(indexed) = find_indexed_session_for_jsonl(
        conn,
        &detected.jsonl_path,
        &detected.session_id,
        &detected.engine,
    )? {
        return Ok(DryRunSession::from(&indexed));
    }

    let stats = scan_jsonl_for_plan(&detected.jsonl_path)?;
    let engine = Engine::from_str(&detected.engine)?;
    Ok(DryRunSession {
        id: truncate_session_id(&detected.session_id, &engine),
        engine: detected.engine.clone(),
        model: None,
        started_at: stats
            .first_timestamp
            .unwrap_or_else(|| Utc::now().to_rfc3339()),
        jsonl_path: detected.jsonl_path.clone(),
        indexed: false,
    })
}

fn dry_run_session_from_jsonl(
    conn: &Connection,
    jsonl_path: &Path,
    engine_override: Option<&str>,
) -> Result<DryRunSession, GaalError> {
    let engine = engine_override
        .map(str::to_string)
        .unwrap_or_else(|| infer_source_engine_from_jsonl(jsonl_path));
    let raw_id = extract_session_id_from_jsonl(jsonl_path, &engine)
        .or_else(|| {
            jsonl_path
                .file_stem()
                .and_then(|s| s.to_str())
                .map(str::to_string)
        })
        .ok_or_else(|| GaalError::ParseError("invalid --jsonl path".into()))?;

    if let Some(indexed) = find_indexed_session_for_jsonl(conn, jsonl_path, &raw_id, &engine)? {
        return Ok(DryRunSession::from(&indexed));
    }

    let stats = scan_jsonl_for_plan(jsonl_path)?;
    let engine_type = Engine::from_str(&engine)?;
    Ok(DryRunSession {
        id: truncate_session_id(&raw_id, &engine_type),
        engine,
        model: None,
        started_at: stats
            .first_timestamp
            .unwrap_or_else(|| Utc::now().to_rfc3339()),
        jsonl_path: jsonl_path.to_path_buf(),
        indexed: false,
    })
}

fn find_indexed_session_for_jsonl(
    conn: &Connection,
    jsonl_path: &Path,
    raw_id: &str,
    engine: &str,
) -> Result<Option<SessionRow>, GaalError> {
    let engine_type = Engine::from_str(engine)?;
    let short_id = truncate_session_id(raw_id, &engine_type);
    if let Some(session) = get_session(conn, &short_id)? {
        return Ok(Some(session));
    }

    let path = jsonl_path.to_string_lossy();
    let mut stmt = conn
        .prepare(
            r#"
            SELECT id
            FROM sessions
            WHERE jsonl_path = :jsonl_path
            ORDER BY started_at DESC
            LIMIT 1
            "#,
        )
        .map_err(GaalError::from)?;
    let mut rows = stmt
        .query(named_params! { ":jsonl_path": path.as_ref() })
        .map_err(GaalError::from)?;
    if let Some(row) = rows.next().map_err(GaalError::from)? {
        let id = row.get::<_, String>(0).map_err(GaalError::from)?;
        return get_session(conn, &id);
    }

    Ok(None)
}

fn build_dry_run_plan(
    conn: &Connection,
    session: &DryRunSession,
    request: DryRunRequest<'_>,
) -> Result<HandoffDryRunResult, GaalError> {
    let stats = scan_dry_run_session_for_handoff_plan(session)?;
    let mut warnings = Vec::new();
    let execution_plan = plan_execution_chunks(&stats, &mut warnings);
    let estimated_llm_calls = match execution_plan.strategy.as_str() {
        "single" => 1,
        _ => execution_plan
            .chunks
            .len()
            .saturating_add(1)
            .min(MAX_LLM_CALLS_PER_SESSION),
    };
    let provider_supported = request.provider == "agent-mux";
    if !provider_supported {
        warnings.push(format!(
            "provider `{}` is not implemented for real execution; non-dry-run will fail unless --provider agent-mux is used",
            request.provider
        ));
    }
    if !session.indexed {
        warnings.push(
            "session is not indexed; dry-run did not index JSONL or write DB rows".to_string(),
        );
    }
    let handoff_path =
        planned_handoff_path(conn, &session.id, &session.engine, &session.started_at)?;
    if handoff_path.exists() {
        warnings.push(
            "handoff file already exists; real execution would overwrite/update it".to_string(),
        );
    }

    Ok(HandoffDryRunResult {
        session_id: session.id.clone(),
        status: "dry_run".to_string(),
        engine: session.engine.clone(),
        indexed: session.indexed,
        jsonl_path: session.jsonl_path.to_string_lossy().to_string(),
        handoff_path: handoff_path.to_string_lossy().to_string(),
        strategy: execution_plan.strategy,
        estimated_transcript_chars: stats.chars,
        estimated_transcript_tokens: estimate_tokens(stats.chars),
        jsonl_lines: stats.lines,
        compaction_lines: stats.compaction_lines,
        chunk_count: execution_plan.chunks.len(),
        estimated_llm_calls,
        single_context_limit_chars: SINGLE_CONTEXT_LIMIT_CHARS,
        max_chunks: MAX_CHUNKS,
        max_llm_calls_per_session: MAX_LLM_CALLS_PER_SESSION,
        provider: request.provider.to_string(),
        provider_supported,
        model: request.model.to_string(),
        effort: request.effort.to_string(),
        format: request.format.to_string(),
        session_model: session.model.clone(),
        side_effects: DryRunSideEffects::none(),
        warnings,
    })
}

fn plan_chunking(stats: &JsonlPlanStats, warnings: &mut Vec<String>) -> (String, usize) {
    if stats.chars <= SINGLE_CONTEXT_LIMIT_CHARS {
        return ("single".to_string(), 1);
    }

    if !stats.compaction_lines.is_empty() {
        let natural_chunks = stats.compaction_lines.len().saturating_add(1);
        if natural_chunks > MAX_CHUNKS {
            warnings.push(format!(
                "compaction boundaries imply {natural_chunks} chunks; capped to max_chunks={MAX_CHUNKS}"
            ));
        }
        return (
            "chunked_compaction".to_string(),
            natural_chunks.clamp(2, MAX_CHUNKS),
        );
    }

    let natural_chunks = stats
        .chars
        .div_ceil(SINGLE_CONTEXT_LIMIT_CHARS)
        .try_into()
        .unwrap_or(usize::MAX);
    if natural_chunks > MAX_CHUNKS {
        warnings.push(format!(
            "size implies {natural_chunks} chunks; capped to max_chunks={MAX_CHUNKS}"
        ));
    }
    (
        "chunked_turn_split".to_string(),
        natural_chunks.clamp(2, MAX_CHUNKS),
    )
}

fn plan_execution_chunks(stats: &JsonlPlanStats, warnings: &mut Vec<String>) -> ExecutionPlan {
    let (strategy, chunk_count) = plan_chunking(stats, warnings);
    let total_lines = stats.lines.max(1);
    let chunks = if strategy == "chunked_compaction" {
        let mut chunks = Vec::new();
        let mut start = 1usize;
        for end in stats
            .compaction_lines
            .iter()
            .copied()
            .take(chunk_count.saturating_sub(1))
        {
            let end = end.clamp(start, total_lines);
            chunks.push(ChunkPlan {
                index: chunks.len() + 1,
                total: chunk_count,
                source_start_line: start,
                source_end_line: end,
            });
            start = end.saturating_add(1);
        }
        if chunks.len() < chunk_count && start <= total_lines {
            chunks.push(ChunkPlan {
                index: chunks.len() + 1,
                total: chunk_count,
                source_start_line: start,
                source_end_line: total_lines,
            });
        }
        normalize_chunk_totals(chunks)
    } else if strategy == "chunked_turn_split" {
        let chunk_size = total_lines.div_ceil(chunk_count);
        let mut chunks = Vec::new();
        for idx in 0..chunk_count {
            let start = idx.saturating_mul(chunk_size).saturating_add(1);
            if start > total_lines {
                break;
            }
            let end = ((idx + 1).saturating_mul(chunk_size)).min(total_lines);
            chunks.push(ChunkPlan {
                index: chunks.len() + 1,
                total: chunk_count,
                source_start_line: start,
                source_end_line: end,
            });
        }
        normalize_chunk_totals(chunks)
    } else {
        vec![ChunkPlan {
            index: 1,
            total: 1,
            source_start_line: 1,
            source_end_line: total_lines,
        }]
    };

    ExecutionPlan {
        strategy,
        chunks,
        jsonl_lines: stats.lines,
        compaction_lines: stats.compaction_lines.clone(),
    }
}

fn normalize_chunk_totals(mut chunks: Vec<ChunkPlan>) -> Vec<ChunkPlan> {
    let total = chunks.len().max(1);
    for (idx, chunk) in chunks.iter_mut().enumerate() {
        chunk.index = idx + 1;
        chunk.total = total;
    }
    chunks
}

fn scan_jsonl_for_plan(path: &Path) -> Result<JsonlPlanStats, GaalError> {
    let meta = fs::metadata(path).map_err(GaalError::from)?;
    let file = File::open(path).map_err(GaalError::from)?;
    let reader = BufReader::new(file);
    let mut lines = 0usize;
    let mut compaction_lines = Vec::new();
    let mut first_timestamp = None;

    for line in reader.lines() {
        let line = line.map_err(GaalError::from)?;
        lines += 1;
        if first_timestamp.is_none() {
            first_timestamp = extract_json_string_field(&line, "timestamp")
                .or_else(|| extract_json_string_field(&line, "created_at"));
        }
        if line_has_top_level_compacted_type(&line) {
            compaction_lines.push(lines);
        }
    }

    Ok(JsonlPlanStats {
        chars: meta.len(),
        lines,
        compaction_lines,
        first_timestamp,
    })
}

fn scan_dry_run_session_for_plan(session: &DryRunSession) -> Result<JsonlPlanStats, GaalError> {
    if session.engine == "hermes" {
        return scan_hermes_source_for_plan(&session.jsonl_path, &session.id, &session.started_at);
    }
    if session.engine == "grok" {
        return scan_grok_source_for_plan(&session.jsonl_path, &session.id, &session.started_at);
    }
    scan_jsonl_for_plan(&session.jsonl_path)
}

fn scan_indexed_session_for_plan(session: &SessionRow) -> Result<JsonlPlanStats, GaalError> {
    if session.engine == "hermes" {
        return scan_hermes_source_for_plan(
            Path::new(&session.jsonl_path),
            &session.id,
            &session.started_at,
        );
    }
    if session.engine == "grok" {
        return scan_grok_source_for_plan(
            Path::new(&session.jsonl_path),
            &session.id,
            &session.started_at,
        );
    }
    scan_jsonl_for_plan(Path::new(&session.jsonl_path))
}

fn scan_dry_run_session_for_handoff_plan(
    session: &DryRunSession,
) -> Result<JsonlPlanStats, GaalError> {
    if let Some(transcript) = resolve_dry_run_session_transcript(session) {
        return Ok(scan_transcript_text_for_plan(
            &transcript,
            Some(session.started_at.clone()),
        ));
    }
    scan_dry_run_session_for_plan(session)
}

fn scan_indexed_session_for_handoff_plan(
    session: &SessionRow,
    transcript: Option<&str>,
) -> Result<JsonlPlanStats, GaalError> {
    if let Some(transcript) = transcript {
        return Ok(scan_transcript_text_for_plan(
            transcript,
            Some(session.started_at.clone()),
        ));
    }
    scan_indexed_session_for_plan(session)
}

fn scan_transcript_text_for_plan(
    transcript: &str,
    first_timestamp: Option<String>,
) -> JsonlPlanStats {
    JsonlPlanStats {
        chars: transcript.len() as u64,
        lines: transcript.lines().count().max(1),
        compaction_lines: Vec::new(),
        first_timestamp,
    }
}

fn scan_hermes_source_for_plan(
    db_path: &Path,
    session_id: &str,
    started_at: &str,
) -> Result<JsonlPlanStats, GaalError> {
    if !db_path.exists() {
        return Err(GaalError::NotFound(db_path.display().to_string()));
    }
    let conn = open_db_readonly()?;
    let transcript =
        crate::render::session_md::render_hermes_session_markdown(db_path, &conn, session_id)
            .map_err(|e| GaalError::Internal(format!("render Hermes transcript: {e}")))?;
    Ok(JsonlPlanStats {
        chars: transcript.len() as u64,
        lines: transcript.lines().count().max(1),
        compaction_lines: Vec::new(),
        first_timestamp: Some(started_at.to_string()),
    })
}

fn scan_grok_source_for_plan(
    session_dir: &Path,
    session_id: &str,
    started_at: &str,
) -> Result<JsonlPlanStats, GaalError> {
    if !session_dir.exists() {
        return Err(GaalError::NotFound(session_dir.display().to_string()));
    }
    let transcript =
        crate::render::session_md::render_grok_session_markdown(session_dir, session_id)
            .map_err(|e| GaalError::Internal(format!("render Grok transcript: {e}")))?;
    Ok(JsonlPlanStats {
        chars: transcript.len() as u64,
        lines: transcript.lines().count().max(1),
        compaction_lines: Vec::new(),
        first_timestamp: Some(started_at.to_string()),
    })
}

fn line_has_top_level_compacted_type(line: &str) -> bool {
    let prefix = match line.find("\"payload\"") {
        Some(idx) => &line[..idx],
        None => line.get(..line.len().min(512)).unwrap_or(line),
    };
    prefix.contains("\"type\":\"compacted\"") || prefix.contains("\"type\": \"compacted\"")
}

fn extract_json_string_field(line: &str, field: &str) -> Option<String> {
    let compact = format!("\"{field}\":\"");
    let spaced = format!("\"{field}\": \"");
    let (idx, pattern_len) = line
        .find(&compact)
        .map(|idx| (idx, compact.len()))
        .or_else(|| line.find(&spaced).map(|idx| (idx, spaced.len())))?;
    let rest = &line[idx + pattern_len..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn planned_handoff_path(
    conn: &Connection,
    session_id: &str,
    engine: &str,
    started_at: &str,
) -> Result<PathBuf, GaalError> {
    let (year, month, day) = date_parts(started_at);
    let artifact_id = crate::db::queries::display_id_for_session(conn, engine, session_id)?;
    Ok(gaal_home()
        .join("data")
        .join(engine)
        .join("handoffs")
        .join(year)
        .join(month)
        .join(day)
        .join(format!("{artifact_id}.md")))
}

fn infer_engine_from_jsonl_path(path: &Path) -> &'static str {
    let path = path.to_string_lossy();
    if path.contains(".codex") {
        "codex"
    } else if path.contains(".gemini/antigravity-cli") {
        "agy"
    } else if path.contains(".gemini") {
        "gemini"
    } else {
        "claude"
    }
}

fn infer_source_engine_from_jsonl(path: &Path) -> String {
    detect_engine(path)
        .map(|engine| engine.to_string())
        .unwrap_or_else(|_| infer_engine_from_jsonl_path(path).to_string())
}

fn estimate_tokens(chars: u64) -> u64 {
    chars.div_ceil(ESTIMATED_CHARS_PER_TOKEN)
}

fn effective_handoff_effort(config: &GaalConfig) -> &str {
    config
        .agent_mux
        .effort
        .as_deref()
        .map(str::trim)
        .filter(|effort| !effort.is_empty())
        .unwrap_or("xhigh")
}

fn print_handoff_dry_run_human(plans: &[HandoffDryRunResult]) {
    for plan in plans {
        println!(
            "{}: {} ({} chunks, {} call(s), {} chars/~{} tokens)",
            plan.session_id,
            plan.strategy,
            plan.chunk_count,
            plan.estimated_llm_calls,
            plan.estimated_transcript_chars,
            plan.estimated_transcript_tokens
        );
        println!("  jsonl: {}", plan.jsonl_path);
        println!("  handoff: {}", plan.handoff_path);
        println!(
            "  provider/model/effort: {}/{}/{}",
            plan.provider, plan.model, plan.effort
        );
        println!("  indexed: {}", plan.indexed);
        println!("  provider_supported: {}", plan.provider_supported);
        println!("  compaction_lines: {:?}", plan.compaction_lines);
        println!(
            "  side_effects: spawn_provider_worker=false spend_tokens=false write_handoff_markdown=false upsert_db_rows=false index_jsonl=false"
        );
        if !plan.warnings.is_empty() {
            println!("  warnings:");
            for warning in &plan.warnings {
                println!("    - {warning}");
            }
        }
    }
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
    let handoff_request = HandoffRequest {
        engine: &engine,
        model: &model,
        prompt: &prompt,
        provider: &provider,
        format: &format,
    };

    if args.parallel <= 1 || total <= 1 {
        for (idx, session) in candidates.iter().enumerate() {
            eprintln!("Batch {}/{}: {}", idx + 1, total, session.id);
            let started = Instant::now();
            let outcome = process_single_batch_session(conn, config, session, handoff_request);
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
                        HandoffRequest {
                            engine: &engine,
                            model: &model,
                            prompt: &prompt,
                            provider: &provider,
                            format: &format,
                        },
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
    request: HandoffRequest<'_>,
) -> Result<String, GaalError> {
    let processed = process_session_handoff(conn, config, session, request)?;
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
    request: HandoffRequest<'_>,
) -> Result<ProcessedSessionHandoff, GaalError> {
    if request.provider != "agent-mux" {
        return Err(GaalError::Other(anyhow!(
            "handoff provider `{}` is not implemented for real execution; use --provider agent-mux",
            request.provider
        )));
    }

    let transcript = resolve_session_transcript(conn, session, config);
    let mut plan_warnings = Vec::new();
    let execution_plan = scan_indexed_session_for_handoff_plan(session, transcript.as_deref())
        .map(|stats| plan_execution_chunks(&stats, &mut plan_warnings))
        .ok();
    if let Some(plan) = execution_plan {
        for warning in plan_warnings {
            eprintln!("Handoff planner warning: {warning}");
        }
        if plan.is_chunked() {
            return process_chunked_session_handoff(
                conn, config, session, request, plan, transcript,
            );
        }
    }

    // Try session markdown transcript first (full narrative context),
    // fall back to DB facts (lossy structured context).
    let context = match transcript {
        Some(transcript) => {
            eprintln!(
                "Using session transcript ({} chars) for context",
                transcript.len()
            );
            build_context_from_transcript(session, &transcript, request.provider, request.format)
        }
        None => {
            eprintln!("No session transcript found, falling back to DB facts");
            let facts = get_facts(conn, &session.id, None)?;
            build_context(session, &facts, request.provider, request.format)
        }
    };

    let (response, extracted) =
        invoke_validated_handoff(config, session, request, &context, "single-pass")?;

    finish_handoff(conn, config, session, request, response, extracted)
}

fn process_chunked_session_handoff(
    conn: &Connection,
    config: &GaalConfig,
    session: &SessionRow,
    request: HandoffRequest<'_>,
    plan: ExecutionPlan,
    transcript: Option<String>,
) -> Result<ProcessedSessionHandoff, GaalError> {
    eprintln!(
        "Using chunked handoff execution: {} ({} mapper calls + 1 reducer call)",
        plan.strategy,
        plan.chunks.len()
    );

    let chunk_contexts = build_chunk_contexts(
        conn,
        session,
        config,
        &plan,
        request.provider,
        request.format,
        transcript.as_deref(),
    )?;
    let mapper_prompt = build_chunk_mapper_prompt(request.prompt);
    let timeout_secs = config
        .agent_mux
        .timeout_secs
        .unwrap_or(config.llm.timeout_secs);
    let mut mapped = Vec::with_capacity(chunk_contexts.len());

    for chunk in chunk_contexts {
        eprintln!(
            "Mapping chunk {}/{} from source JSONL lines {}-{}",
            chunk.plan.index,
            chunk.plan.total,
            chunk.plan.source_start_line,
            chunk.plan.source_end_line
        );
        let context_chars = chunk.body.len();
        let response = invoke_agent_mux(
            &config.agent_mux,
            request.engine,
            request.model,
            session.cwd.as_deref().unwrap_or("."),
            &mapper_prompt,
            &chunk.body,
            timeout_secs,
        )?;
        mapped.push(ChunkMapResult {
            plan: chunk.plan,
            status: "mapped".to_string(),
            response,
            source_kind: chunk.source_kind,
            rendered_start_line: chunk.rendered_start_line,
            rendered_end_line: chunk.rendered_end_line,
            context_chars,
        });
    }

    let coverage_manifest = build_coverage_manifest(session, &plan, &mapped);
    let reducer_prompt = build_chunk_reducer_prompt(request.prompt);
    let reducer_context = build_reducer_context(
        session,
        request.provider,
        request.format,
        &mapped,
        &coverage_manifest,
    );
    let (response, extracted) = invoke_validated_handoff(
        config,
        session,
        HandoffRequest {
            engine: request.engine,
            model: request.model,
            prompt: &reducer_prompt,
            provider: request.provider,
            format: request.format,
        },
        &reducer_context,
        "chunked reducer",
    )?;
    let response = format!(
        "{}\n\n{}",
        coverage_manifest.trim_end(),
        response.trim_start()
    );

    finish_handoff(conn, config, session, request, response, extracted)
}

fn invoke_validated_handoff(
    config: &GaalConfig,
    session: &SessionRow,
    request: HandoffRequest<'_>,
    context: &str,
    label: &str,
) -> Result<(String, ExtractedMetadata), GaalError> {
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
            request.engine,
            request.model,
            session.cwd.as_deref().unwrap_or("."),
            request.prompt,
            context,
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
                    "Handoff validation failed for {label} (attempt {}/{}): {}. Retrying...",
                    attempt, max_attempts, reason
                );
            }
            Err(reason) => {
                eprintln!(
                    "Handoff validation failed for {label} after {} attempts: {}. Accepting best-effort.",
                    max_attempts, reason
                );
            }
        }
    }

    Ok((response, extracted))
}

fn finish_handoff(
    conn: &Connection,
    config: &GaalConfig,
    session: &SessionRow,
    request: HandoffRequest<'_>,
    response: String,
    extracted: ExtractedMetadata,
) -> Result<ProcessedSessionHandoff, GaalError> {
    // Use the session's own engine/model for frontmatter (ground truth),
    // not the extraction LLM engine/model.
    let session_engine = &session.engine;
    let session_model = session.model.as_deref().unwrap_or("unknown");
    let frontmatter = build_handoff_frontmatter(session, &extracted, session_engine, session_model);
    let full_content = format!("{}{}", frontmatter, response);
    let handoff_path = write_handoff_markdown(conn, session, &full_content)?;
    let generated_by = build_generated_by_label(&config.agent_mux, request.engine, request.model);

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

fn build_chunk_contexts(
    conn: &Connection,
    session: &SessionRow,
    config: &GaalConfig,
    plan: &ExecutionPlan,
    provider: &str,
    format: &str,
    transcript: Option<&str>,
) -> Result<Vec<ChunkContext>, GaalError> {
    if let Some(transcript) = transcript {
        eprintln!(
            "Using session transcript ({} chars) split into {} chunk contexts",
            transcript.len(),
            plan.chunks.len()
        );
        return Ok(build_transcript_chunk_contexts(
            session, transcript, plan, provider, format,
        ));
    }

    if let Some(transcript) = resolve_session_transcript(conn, session, config) {
        eprintln!(
            "Using session transcript ({} chars) split into {} chunk contexts",
            transcript.len(),
            plan.chunks.len()
        );
        return Ok(build_transcript_chunk_contexts(
            session,
            &transcript,
            plan,
            provider,
            format,
        ));
    }

    eprintln!("No session transcript found, falling back to JSONL line chunks");
    build_jsonl_chunk_contexts(session, plan, provider, format)
}

fn build_transcript_chunk_contexts(
    session: &SessionRow,
    transcript: &str,
    plan: &ExecutionPlan,
    provider: &str,
    format: &str,
) -> Vec<ChunkContext> {
    let transcript_lines: Vec<&str> = transcript.lines().collect();
    let total_rendered_lines = transcript_lines.len().max(1);
    let total_source_lines = plan
        .chunks
        .iter()
        .map(|chunk| chunk.source_end_line)
        .max()
        .unwrap_or(1)
        .max(1);

    plan.chunks
        .iter()
        .enumerate()
        .map(|(idx, chunk)| {
            let previous_end = if idx == 0 {
                0
            } else {
                proportional_line(
                    plan.chunks[idx - 1].source_end_line,
                    total_source_lines,
                    total_rendered_lines,
                )
            };
            let rendered_start = previous_end
                .saturating_add(1)
                .clamp(1, total_rendered_lines);
            let mut rendered_end = if idx + 1 == plan.chunks.len() {
                total_rendered_lines
            } else {
                proportional_line(
                    chunk.source_end_line,
                    total_source_lines,
                    total_rendered_lines,
                )
            };
            if rendered_end < rendered_start {
                rendered_end = rendered_start;
            }
            let slice = transcript_lines
                .iter()
                .skip(rendered_start.saturating_sub(1))
                .take(
                    rendered_end
                        .saturating_sub(rendered_start)
                        .saturating_add(1),
                )
                .copied()
                .collect::<Vec<_>>()
                .join("\n");
            let body = build_chunk_context_body(
                session,
                provider,
                format,
                &plan.strategy,
                chunk,
                "rendered transcript",
                Some((rendered_start, rendered_end)),
                &slice,
            );
            ChunkContext {
                plan: chunk.clone(),
                body,
                source_kind: "rendered_transcript",
                rendered_start_line: Some(rendered_start),
                rendered_end_line: Some(rendered_end),
            }
        })
        .collect()
}

fn proportional_line(
    source_line: usize,
    total_source_lines: usize,
    total_rendered_lines: usize,
) -> usize {
    let source_zero = source_line.saturating_sub(1);
    let rendered_zero = source_zero
        .saturating_mul(total_rendered_lines)
        .checked_div(total_source_lines.max(1))
        .unwrap_or(0);
    rendered_zero
        .saturating_add(1)
        .clamp(1, total_rendered_lines.max(1))
}

fn build_jsonl_chunk_contexts(
    session: &SessionRow,
    plan: &ExecutionPlan,
    provider: &str,
    format: &str,
) -> Result<Vec<ChunkContext>, GaalError> {
    let file = File::open(&session.jsonl_path).map_err(GaalError::from)?;
    let reader = BufReader::new(file);
    let mut chunks = plan
        .chunks
        .iter()
        .map(|chunk| (chunk.clone(), Vec::<String>::new()))
        .collect::<Vec<_>>();

    for (line_number, line) in reader.lines().enumerate() {
        let line_number = line_number + 1;
        let line = line.map_err(GaalError::from)?;
        if let Some((_, lines)) = chunks.iter_mut().find(|(chunk, _)| {
            line_number >= chunk.source_start_line && line_number <= chunk.source_end_line
        }) {
            lines.push(line);
        }
    }

    Ok(chunks
        .into_iter()
        .map(|(chunk, lines)| {
            let slice = lines.join("\n");
            let body = build_chunk_context_body(
                session,
                provider,
                format,
                &plan.strategy,
                &chunk,
                "raw JSONL",
                None,
                &slice,
            );
            ChunkContext {
                plan: chunk,
                body,
                source_kind: "raw_jsonl",
                rendered_start_line: None,
                rendered_end_line: None,
            }
        })
        .collect())
}

#[allow(clippy::too_many_arguments)]
fn build_chunk_context_body(
    session: &SessionRow,
    provider: &str,
    format: &str,
    strategy: &str,
    chunk: &ChunkPlan,
    source_label: &str,
    rendered_lines: Option<(usize, usize)>,
    body: &str,
) -> String {
    let rendered_line_note = rendered_lines
        .map(|(start, end)| {
            format!(
                "- rendered_transcript_lines: {start}-{end} (approximate range derived from source JSONL line proportions)\n"
            )
        })
        .unwrap_or_default();
    format!(
        "Requested provider: {provider}\nRequested format: {format}\n\n\
GROUND TRUTH (do not override in your output):\n\
- engine: {engine}\n\
- model: {model}\n\
These values are determined from the session source. Do not infer or hallucinate different engine/model values.\n\n\
Chunk Coverage:\n\
- strategy: {strategy}\n\
- chunk: {chunk_index}/{chunk_total}\n\
- source_jsonl_path: {jsonl_path}\n\
- source_jsonl_lines: {source_start}-{source_end}\n\
{rendered_line_note}\
\nSession Summary:\n\
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
--- SESSION CHUNK ({source_label}) ---\n\n\
{body}\n",
        engine = session.engine,
        model = session.model.as_deref().unwrap_or("unknown"),
        chunk_index = chunk.index,
        chunk_total = chunk.total,
        jsonl_path = session.jsonl_path,
        source_start = chunk.source_start_line,
        source_end = chunk.source_end_line,
        id = session.id,
        cwd = session.cwd.as_deref().unwrap_or("."),
        started_at = session.started_at,
        ended_at = session.ended_at.as_deref().unwrap_or("in_progress"),
        input_tokens = session.total_input_tokens,
        output_tokens = session.total_output_tokens,
        tools = session.total_tools,
        turns = session.total_turns,
    )
}

fn build_chunk_mapper_prompt(base_prompt: &str) -> String {
    format!(
        "{base_prompt}\n\n\
You are running the mapper phase of chunked Gaal handoff generation. Analyze only the supplied chunk. \
Return concise markdown with concrete facts, decisions, open threads, files, commands, and risks visible in this chunk. \
Do not claim whole-session coverage and do not invent missing continuity.\n\n\
Path and state fidelity rules:\n\
- Preserve exact file paths as written in the chunk; never rewrite path roots or move files into a more familiar repo.\n\
- Because final handoffs live under ~/.gaal, repo-relative paths are unsafe for continuation. If the chunk provides an absolute path, copy the absolute path.\n\
- If the chunk establishes a worktree root and later mentions repo-relative paths from that worktree, resolve them to absolute paths in your mapper output.\n\
- If the chunk contains final git/worktree status, dirty files, uncommitted changes, or explicit not-committed/not-pushed state, preserve it.\n\
- Preserve the exact dirty-file list when present.\n\
- Preserve concrete verification facts, falsifiers, endpoint diagnostics, and residual risks even when they look too detailed.\n\
- If a fact is uncertain from this chunk, label it uncertain instead of smoothing it into a confident claim."
    )
}

fn build_chunk_reducer_prompt(base_prompt: &str) -> String {
    format!(
        "{base_prompt}\n\n\
You are running the reducer phase of chunked Gaal handoff generation. Synthesize the mapper outputs into one final handoff for the whole session. \
Preserve the normal handoff sections and metadata expectations from the base prompt. \
Use only mapper evidence and the coverage manifest supplied in context.\n\n\
Continuation-critical fidelity rules:\n\
- Preserve exact file paths from mapper evidence; do not rewrite path roots or infer shorter/cleaner destinations.\n\
- Final handoffs are stored under ~/.gaal, so repo-relative paths are ambiguous. Key Files and Open Threads must use absolute paths whenever a mapper supplied, or could resolve, an absolute path.\n\
- If a mapper still gives a relative path, state its worktree root explicitly before the relative path instead of leaving it bare.\n\
- Preserve final repo/worktree state, especially dirty files, uncommitted changes, not-pushed branches, and explicit next gates.\n\
- Preserve the exact dirty-file list when present.\n\
- Preserve concrete verification facts and falsifiers that would change whether the next agent trusts the result.\n\
- Prefer a slightly longer handoff over dropping late-session validation evidence.\n\
- If mapper evidence conflicts, say so; do not silently merge it into a cleaner story."
    )
}

fn build_reducer_context(
    session: &SessionRow,
    provider: &str,
    format: &str,
    mapped: &[ChunkMapResult],
    coverage_manifest: &str,
) -> String {
    let mut chunks = String::new();
    for result in mapped {
        chunks.push_str(&format!(
            "\n\n--- MAPPER OUTPUT {}/{} (source JSONL lines {}-{}, status {}) ---\n\n{}\n",
            result.plan.index,
            result.plan.total,
            result.plan.source_start_line,
            result.plan.source_end_line,
            result.status,
            result.response.trim()
        ));
    }

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
Path Root Rule:\n\
- This final handoff will be stored under ~/.gaal, not in the original worktree. Do not leave continuation-critical paths as repo-relative strings. Use absolute paths from mapper evidence, or explicitly name the worktree root before relative paths.\n\n\
{coverage_manifest}\n\
{chunks}\n",
        engine = session.engine,
        model = session.model.as_deref().unwrap_or("unknown"),
        id = session.id,
        cwd = session.cwd.as_deref().unwrap_or("."),
        started_at = session.started_at,
        ended_at = session.ended_at.as_deref().unwrap_or("in_progress"),
        input_tokens = session.total_input_tokens,
        output_tokens = session.total_output_tokens,
        tools = session.total_tools,
        turns = session.total_turns,
    )
}

fn build_coverage_manifest(
    session: &SessionRow,
    plan: &ExecutionPlan,
    mapped: &[ChunkMapResult],
) -> String {
    let mut manifest = format!(
        "## Coverage Manifest\n\n\
- strategy: {}\n\
- source_jsonl_path: {}\n\
- source_jsonl_lines: {}\n\
- compaction_lines: {:?}\n\
- chunks_completed: {}/{}\n\
- mapper_calls: {}\n\
- reducer_calls: 1\n\
- surfaced_part_files: false\n\
- excluded_content: compacted records were used as boundary markers only; encrypted replacement_history payloads were not interpreted\n\n\
| Chunk | Source JSONL Lines | Source Used | Rendered Transcript Lines | Status | Context Chars |\n\
|---|---:|---|---:|---|---:|\n",
        plan.strategy,
        session.jsonl_path,
        plan.jsonl_lines,
        plan.compaction_lines,
        mapped.len(),
        plan.chunks.len(),
        mapped.len()
    );
    for result in mapped {
        let rendered = match (result.rendered_start_line, result.rendered_end_line) {
            (Some(start), Some(end)) => format!("{start}-{end}"),
            _ => "-".to_string(),
        };
        manifest.push_str(&format!(
            "| {}/{} | {}-{} | {} | {} | {} | {} |\n",
            result.plan.index,
            result.plan.total,
            result.plan.source_start_line,
            result.plan.source_end_line,
            result.source_kind,
            rendered,
            result.status,
            result.context_chars
        ));
    }
    manifest
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
            gemini_summary: None,
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

    let ids = crate::db::queries::resolve_session_ids(conn, id_or_today, None)?;

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
            "agy" => {
                if let Some(id) = value
                    .get("sessionId")
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

    if engine == "agy" {
        return extract_agy_session_id_from_path(path);
    }

    None
}

fn extract_agy_session_id_from_path(path: &Path) -> Option<String> {
    let mut previous_was_brain = false;
    for component in path.components() {
        let value = component.as_os_str().to_string_lossy();
        if previous_was_brain {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
        previous_was_brain = value == "brain";
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
        forked_from_id: None,
        file_size: meta.len(),
    };

    // For single-session on-the-fly indexing, pass an empty set — force=true
    // already triggers full reparse so the codex error check is moot.
    let empty_set = HashSet::new();
    match index_discovered_session(conn, &discovered, true, &empty_set) {
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
        Engine::Claude | Engine::Gemini | Engine::Agy => raw.chars().take(8).collect(),
        Engine::Hermes | Engine::Grok => crate::util::sanitize_filename(raw),
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

/// Attempt to locate and read a session markdown transcript for handoff generation.
///
/// Checks three sources in priority order:
/// 1. On-the-fly render from the session's JSONL file
/// 2. Gaal's own rendered markdown (~/.gaal/data/{engine}/sessions/YYYY/MM/DD/{id}.md)
/// 3. External output directory configured for rendered session markdown
///
/// Returns `None` if all sources fail.
fn resolve_session_transcript(
    conn: &Connection,
    session: &SessionRow,
    config: &GaalConfig,
) -> Option<String> {
    let artifact_id = crate::db::queries::artifact_id_for_session(conn, session)
        .unwrap_or_else(|_| crate::util::session_artifact_id(&session.engine, &session.id));
    let (year, month, day) = date_parts(&session.started_at);

    // 1. Fresh render from JSONL. Handoff generation should not depend on
    // cached markdown because active/recent sessions may still be appending.
    let source_path = Path::new(&session.jsonl_path);
    if source_path.exists() {
        let render_result = if session.engine == "hermes" {
            match open_db_readonly() {
                Ok(conn) => crate::render::session_md::render_hermes_session_markdown(
                    source_path,
                    &conn,
                    &session.id,
                ),
                Err(err) => Err(anyhow!("open gaal DB for Hermes transcript: {err}")),
            }
        } else if session.engine == "grok" {
            crate::render::session_md::render_grok_session_markdown(source_path, &session.id)
        } else {
            crate::render::session_md::render_session_markdown(source_path)
        };
        match render_result {
            Ok(content) if !content.trim().is_empty() => {
                eprintln!(
                    "  -> transcript source: freshly rendered from {}",
                    source_path.display()
                );
                return Some(content);
            }
            Ok(_) => {}
            Err(e) => {
                eprintln!("  -> fresh render failed: {e}");
            }
        }
    }

    // 2. Gaal's own session markdown directory (kept fresh by cron backfill)
    let gaal_md_path = gaal_home()
        .join("data")
        .join(&session.engine)
        .join("sessions")
        .join(&year)
        .join(&month)
        .join(&day)
        .join(format!("{artifact_id}.md"));
    if let Ok(content) = fs::read_to_string(&gaal_md_path) {
        if !content.trim().is_empty() {
            eprintln!("  -> transcript source: {}", gaal_md_path.display());
            return Some(content);
        }
    }

    // 3. External output directory (config.markdown_output_dir) — fallback
    if let Some(ref output_dir) = config.markdown_output_dir {
        let external_path = output_dir
            .join(&year)
            .join(&month)
            .join(&day)
            .join(format!("{artifact_id}.md"));
        if let Ok(content) = fs::read_to_string(&external_path) {
            if !content.trim().is_empty() {
                eprintln!("  -> transcript source: {}", external_path.display());
                return Some(content);
            }
        }
    }

    None
}

fn resolve_dry_run_session_transcript(session: &DryRunSession) -> Option<String> {
    let artifact_id = if session.engine == "hermes" {
        open_db_readonly()
            .ok()
            .and_then(|conn| {
                crate::db::queries::display_id_for_session(&conn, &session.engine, &session.id).ok()
            })
            .unwrap_or_else(|| crate::util::session_artifact_id(&session.engine, &session.id))
    } else {
        crate::util::session_artifact_id(&session.engine, &session.id)
    };
    let (year, month, day) = date_parts(&session.started_at);

    let source_path = &session.jsonl_path;
    if source_path.exists() {
        let render_result = if session.engine == "hermes" {
            match open_db_readonly() {
                Ok(conn) => crate::render::session_md::render_hermes_session_markdown(
                    source_path,
                    &conn,
                    &session.id,
                ),
                Err(err) => Err(anyhow!("open gaal DB for Hermes transcript: {err}")),
            }
        } else if session.engine == "grok" {
            crate::render::session_md::render_grok_session_markdown(source_path, &session.id)
        } else {
            crate::render::session_md::render_session_markdown(source_path)
        };
        if let Ok(content) = render_result {
            if !content.trim().is_empty() {
                return Some(content);
            }
        }
    }

    let gaal_md_path = gaal_home()
        .join("data")
        .join(&session.engine)
        .join("sessions")
        .join(&year)
        .join(&month)
        .join(&day)
        .join(format!("{artifact_id}.md"));
    fs::read_to_string(&gaal_md_path)
        .ok()
        .filter(|content| !content.trim().is_empty())
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
    let profile = mux_config
        .profile
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty());
    let effort = mux_config
        .effort
        .as_deref()
        .map(str::trim)
        .filter(|effort| !effort.is_empty());

    let requested_cwd = mux_config
        .cwd
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .unwrap_or(cwd);
    let effective_cwd = if Path::new(requested_cwd).is_dir() {
        requested_cwd.to_string()
    } else {
        let fallback = std::env::current_dir()
            .map(|path| path.to_string_lossy().to_string())
            .unwrap_or_else(|_| ".".to_string());
        eprintln!("agent-mux cwd `{requested_cwd}` is not available; falling back to `{fallback}`");
        fallback
    };

    let mut command = Command::new(&mux_config.path);
    command
        .arg("--engine")
        .arg(engine)
        .arg("--model")
        .arg(model);
    if let Some(profile) = profile {
        command.arg("-P").arg(profile);
    }
    if let Some(effort) = effort {
        command.arg("--effort").arg(effort);
    }

    let prompt_file = TempPromptFile::new(&request).ok();
    command
        .arg("--timeout")
        .arg(mux_timeout_secs.to_string())
        .arg("--cwd")
        .arg(&effective_cwd);
    if let Some(ref prompt_file) = prompt_file {
        command.arg("--prompt-file").arg(prompt_file.path());
    } else {
        command.arg(request);
    }

    let child = command
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

struct TempPromptFile {
    path: PathBuf,
}

impl TempPromptFile {
    fn new(content: &str) -> std::io::Result<Self> {
        let timestamp = Utc::now().timestamp_nanos_opt().unwrap_or_default();
        let path = std::env::temp_dir().join(format!(
            "gaal-agent-mux-{}-{timestamp}.md",
            std::process::id()
        ));
        fs::write(&path, content)?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempPromptFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
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
    let profile = mux_config
        .profile
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty());
    let effort = mux_config
        .effort
        .as_deref()
        .map(str::trim)
        .filter(|effort| !effort.is_empty());

    let mut label = format!("agent-mux -E={engine} -m={model}");
    if let Some(profile) = profile {
        label.push_str(&format!(" -P={profile}"));
    }
    if let Some(effort) = effort {
        label.push_str(&format!(" -e={effort}"));
    }
    label
}

/// Build YAML frontmatter for handoff markdown files.
fn build_handoff_frontmatter(
    session: &SessionRow,
    extracted: &ExtractedMetadata,
    engine: &str,
    model: &str,
) -> String {
    let sid = crate::util::session_artifact_id(&session.engine, &session.id);

    // Date is user-local, matching rendered transcript frontmatter.
    let date_str = DateTime::parse_from_rfc3339(&session.started_at)
        .map(|dt| dt.with_timezone(&Local).format("%Y-%m-%d").to_string())
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

    let engine_str = if engine.contains("codex") {
        "codex"
    } else if engine.contains("agy") {
        "agy"
    } else if engine.contains("gemini") {
        "gemini"
    } else if engine.contains("hermes") {
        "hermes"
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

fn write_handoff_markdown(
    conn: &Connection,
    session: &SessionRow,
    content: &str,
) -> Result<PathBuf, GaalError> {
    let (year, month, day) = date_parts(&session.started_at);
    let artifact_id = crate::db::queries::artifact_id_for_session(conn, session)?;
    let path = gaal_home()
        .join("data")
        .join(&session.engine)
        .join("handoffs")
        .join(year)
        .join(month)
        .join(day)
        .join(format!("{artifact_id}.md"));

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
        .or_else(|| map.get("substance_score"))
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
    if !out.is_empty() {
        return out;
    }

    let target = format!("- {name}");
    let mut in_bullet_section = false;
    let mut section_indent = 0_usize;
    for line in text.lines() {
        let trimmed = line.trim();
        let lower = trimmed.to_ascii_lowercase();
        if !in_bullet_section {
            if lower == target || lower == format!("{target}:") {
                in_bullet_section = true;
                section_indent = line.len().saturating_sub(line.trim_start().len());
            }
            continue;
        }

        if trimmed.is_empty() {
            continue;
        }
        let indent = line.len().saturating_sub(line.trim_start().len());
        if indent <= section_indent {
            break;
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
    let mut next_value_line = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if next_value_line && !trimmed.is_empty() {
            if let Some(value) = first_substance_digit(trimmed) {
                return value;
            }
            next_value_line = false;
        }

        let lower = line.to_ascii_lowercase();
        if !lower.contains("substance") {
            continue;
        }
        if let Some(value) = first_substance_digit(&lower) {
            return value;
        }
        next_value_line = true;
    }
    0
}

fn first_substance_digit(value: &str) -> Option<i32> {
    value
        .chars()
        .find(|ch| ('0'..='3').contains(ch))
        .map(|ch| (ch as u8 - b'0') as i32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn detects_codex_compaction_type_without_rendering_payload() {
        let line = r#"{"timestamp":"2026-05-04T00:57:52.496Z","type":"compacted","payload":{"replacement_history":[{"type":"message"}]}}"#;
        assert!(line_has_top_level_compacted_type(line));

        let nested_only = r#"{"timestamp":"2026-05-04T00:57:52.496Z","type":"response_item","payload":{"replacement_history":[{"type":"compacted"}]}}"#;
        assert!(!line_has_top_level_compacted_type(nested_only));
    }

    #[test]
    fn extracts_agy_jsonl_session_id_from_brain_path_when_record_lacks_id() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "gaal-agy-handoff-id-{}-{unique}",
            std::process::id()
        ));
        let path = root.join(
            ".gemini/antigravity-cli/brain/12345678-90ab-cdef-1234-567890abcdef/.system_generated/logs/transcript_full.jsonl",
        );
        fs::create_dir_all(path.parent().expect("fixture parent")).expect("create fixture parent");
        fs::write(
            &path,
            "{\"type\":\"USER_INPUT\",\"created_at\":\"2026-06-20T10:00:00Z\",\"content\":\"no id here\"}\n",
        )
        .expect("write fixture transcript");

        let id = extract_session_id_from_jsonl(&path, "agy");
        fs::remove_dir_all(&root).ok();

        assert_eq!(id.as_deref(), Some("12345678-90ab-cdef-1234-567890abcdef"));
    }

    #[test]
    fn scan_jsonl_for_plan_uses_agy_created_at_when_timestamp_absent() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "gaal-agy-created-at-plan-{}-{unique}.jsonl",
            std::process::id()
        ));
        fs::write(
            &path,
            "{\"type\":\"USER_INPUT\",\"created_at\":\"2026-06-20T10:00:00Z\",\"content\":\"hello\"}\n",
        )
        .expect("write fixture transcript");

        let stats = scan_jsonl_for_plan(&path).expect("scan jsonl");
        fs::remove_file(&path).ok();

        assert_eq!(
            stats.first_timestamp.as_deref(),
            Some("2026-06-20T10:00:00Z")
        );
    }

    #[test]
    fn planned_handoff_path_uses_registered_hermes_alias() {
        let conn = rusqlite::Connection::open_in_memory().expect("open db");
        crate::db::init_db(&conn).expect("init schema");
        let session = SessionRow {
            id: "20260505_100000_abcdef123456".to_string(),
            engine: "hermes".to_string(),
            model: Some("hermes-test-model".to_string()),
            cwd: None,
            started_at: "2026-05-05T10:00:00Z".to_string(),
            ended_at: None,
            exit_signal: None,
            last_event_at: Some("2026-05-05T10:00:00Z".to_string()),
            parent_id: None,
            session_type: "standalone".to_string(),
            jsonl_path: "/tmp/hermes-state.db".to_string(),
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
            gemini_summary: None,
        };
        crate::db::queries::upsert_session(&conn, &session).expect("insert session");
        let alias = crate::db::queries::get_session_alias(&conn, "hermes", &session.id)
            .expect("query alias")
            .expect("alias exists");

        let path = planned_handoff_path(&conn, &session.id, &session.engine, &session.started_at)
            .expect("planned path");

        let suffix = format!("data/hermes/handoffs/2026/05/05/{alias}.md");
        assert!(path.to_string_lossy().ends_with(&suffix));
    }

    #[test]
    fn compaction_boundaries_plan_chunk_and_final_calls() {
        let stats = JsonlPlanStats {
            chars: 11_341_402,
            lines: 5_726,
            compaction_lines: vec![687, 1584, 2403, 3322, 5119],
            first_timestamp: None,
        };
        let mut warnings = Vec::new();
        let (strategy, chunks) = plan_chunking(&stats, &mut warnings);

        assert_eq!(strategy, "chunked_compaction");
        assert_eq!(chunks, 6);
        assert_eq!(chunks + 1, 7);
        assert!(warnings.is_empty());
    }

    #[test]
    fn compaction_boundaries_select_chunked_execution_ranges() {
        let stats = JsonlPlanStats {
            chars: 11_341_402,
            lines: 5_726,
            compaction_lines: vec![687, 1584, 2403, 3322, 5119],
            first_timestamp: None,
        };
        let mut warnings = Vec::new();
        let plan = plan_execution_chunks(&stats, &mut warnings);

        assert!(plan.is_chunked());
        assert_eq!(plan.strategy, "chunked_compaction");
        assert_eq!(plan.chunks.len(), 6);
        assert_eq!(plan.chunks[0].source_start_line, 1);
        assert_eq!(plan.chunks[0].source_end_line, 687);
        assert_eq!(plan.chunks[5].source_start_line, 5120);
        assert_eq!(plan.chunks[5].source_end_line, 5726);
        assert!(warnings.is_empty());
    }

    #[test]
    fn rendered_transcript_plan_uses_transcript_size_not_raw_jsonl_size() {
        let transcript = "short handoff transcript\nwith only a few lines\n";
        let stats = scan_transcript_text_for_plan(transcript, None);
        let mut warnings = Vec::new();
        let plan = plan_execution_chunks(&stats, &mut warnings);

        assert_eq!(stats.chars, transcript.len() as u64);
        assert_eq!(stats.lines, 2);
        assert_eq!(plan.strategy, "single");
        assert_eq!(plan.chunks.len(), 1);
        assert!(warnings.is_empty());
    }

    #[test]
    fn rendered_transcript_plan_chunks_large_transcript_without_compaction_bias() {
        let line = format!("{}\n", "x".repeat(1_000));
        let transcript = line.repeat((SINGLE_CONTEXT_LIMIT_CHARS as usize / line.len()) + 2);
        let stats = scan_transcript_text_for_plan(&transcript, None);
        let mut warnings = Vec::new();
        let plan = plan_execution_chunks(&stats, &mut warnings);

        assert_eq!(plan.strategy, "chunked_turn_split");
        assert_eq!(plan.chunks.len(), 2);
        assert!(warnings.is_empty());
    }

    #[test]
    fn transcript_chunk_ranges_cover_every_rendered_line_without_gaps() {
        let plan = ExecutionPlan {
            strategy: "chunked_turn_split".to_string(),
            chunks: vec![
                ChunkPlan {
                    index: 1,
                    total: 3,
                    source_start_line: 1,
                    source_end_line: 10,
                },
                ChunkPlan {
                    index: 2,
                    total: 3,
                    source_start_line: 11,
                    source_end_line: 20,
                },
                ChunkPlan {
                    index: 3,
                    total: 3,
                    source_start_line: 21,
                    source_end_line: 30,
                },
            ],
            jsonl_lines: 30,
            compaction_lines: Vec::new(),
        };
        let session = SessionRow {
            id: "test1234".to_string(),
            engine: "claude".to_string(),
            model: Some("claude-opus".to_string()),
            cwd: Some("/tmp".to_string()),
            started_at: "2026-05-04T00:00:00Z".to_string(),
            ended_at: Some("2026-05-04T01:00:00Z".to_string()),
            exit_signal: None,
            last_event_at: None,
            parent_id: None,
            session_type: "standalone".to_string(),
            jsonl_path: "/tmp/test.jsonl".to_string(),
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
        let transcript = (1..=10)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n");

        let chunks =
            build_transcript_chunk_contexts(&session, &transcript, &plan, "agent-mux", "markdown");

        assert_eq!(chunks[0].rendered_start_line, Some(1));
        assert_eq!(chunks[0].rendered_end_line, Some(4));
        assert_eq!(chunks[1].rendered_start_line, Some(5));
        assert_eq!(chunks[1].rendered_end_line, Some(7));
        assert_eq!(chunks[2].rendered_start_line, Some(8));
        assert_eq!(chunks[2].rendered_end_line, Some(10));
    }

    #[test]
    fn handoff_frontmatter_uses_local_date() {
        let session = SessionRow {
            id: "36a6276e".to_string(),
            engine: "codex".to_string(),
            model: Some("gpt-5.5".to_string()),
            cwd: None,
            started_at: "2026-05-03T23:43:35.159Z".to_string(),
            ended_at: Some("2026-05-04T17:18:23.770Z".to_string()),
            exit_signal: None,
            last_event_at: None,
            parent_id: None,
            session_type: "standalone".to_string(),
            jsonl_path: "/tmp/test.jsonl".to_string(),
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
        let extracted = ExtractedMetadata {
            headline: Some("headline".to_string()),
            projects: vec!["gaal".to_string()],
            keywords: vec!["handoff".to_string()],
            substance: 1,
        };

        let frontmatter = build_handoff_frontmatter(&session, &extracted, "codex", "gpt-5.5");

        assert!(frontmatter.contains("date: 2026-05-04"));
    }

    #[test]
    fn extract_text_metadata_reads_substance_score_heading_value() {
        let response = r#"
## Headline
Generated and converted a banana image.

## Keywords
- `generated-banana.png`
- `GenerateImage`

## Substance Score
2
"#;

        let extracted = extract_text_metadata(response);

        assert_eq!(
            extracted.headline.as_deref(),
            Some("Generated and converted a banana image.")
        );
        assert_eq!(extracted.substance, 2);
        assert_eq!(
            extracted.keywords,
            vec![
                "`generated-banana.png`".to_string(),
                "`GenerateImage`".to_string()
            ]
        );
    }

    #[test]
    fn extract_json_metadata_accepts_substance_score_alias() {
        let response = r#"{
  "headline": "Generated banana image",
  "projects": ["agent-mux"],
  "keywords": ["generated-banana.png"],
  "substance_score": 2
}"#;

        let extracted = extract_metadata(response);

        assert_eq!(extracted.substance, 2);
        assert_eq!(extracted.projects, vec!["agent-mux".to_string()]);
        assert_eq!(extracted.keywords, vec!["generated-banana.png".to_string()]);
    }

    #[test]
    fn extract_text_metadata_reads_lowercase_bullet_sections() {
        let response = r#"
## Headline
Identity smoke passed.

- projects
  - `agent-mux`

- keywords
  - `AGY_FINAL_OK`
  - `AGY_FINAL_RESUME_OK`

- substance_score
  - `1`
"#;

        let extracted = extract_text_metadata(response);

        assert_eq!(extracted.projects, vec!["`agent-mux`".to_string()]);
        assert_eq!(
            extracted.keywords,
            vec![
                "`AGY_FINAL_OK`".to_string(),
                "`AGY_FINAL_RESUME_OK`".to_string()
            ]
        );
        assert_eq!(extracted.substance, 1);
    }
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
