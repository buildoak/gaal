use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use anyhow::{bail, Context, Result};
use serde_json::Value;

use crate::discovery::DiscoveredSession;

pub mod agy;
pub mod claude;
pub mod codex;
pub mod common;
pub mod event;
pub mod facts;
pub mod gemini;
pub mod hermes;
pub mod types;

pub use event::{ContentBlock, EventKind, SessionEvent, ToolUseEvent};
pub use types::{Engine, ParsedSession, SessionMeta};

/// Detects which engine produced the JSONL stream at `path`.
pub fn detect_engine(path: &Path) -> Result<Engine> {
    let file = File::open(path)
        .with_context(|| format!("failed to open session file: {}", path.display()))?;
    let reader = BufReader::new(file);
    let mut seen = 0usize;

    for line_result in reader.lines() {
        let line = line_result.context("failed to read JSONL line")?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        seen += 1;
        if seen > 10 {
            break;
        }

        let record: Value = match serde_json::from_str(trimmed) {
            Ok(value) => value,
            Err(_) => continue,
        };

        if record.get("sessionId").is_some()
            && record.get("projectHash").is_some()
            && record.get("messages").is_some()
        {
            return Ok(Engine::Gemini);
        }

        let record_type = record
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();

        if is_agy_record(&record) {
            return Ok(Engine::Agy);
        }
        if is_claude_type(record_type) {
            return Ok(Engine::Claude);
        }
        if is_codex_type(record_type) {
            return Ok(Engine::Codex);
        }
    }

    // Fallback: infer engine from the file's directory when content is empty or
    // lacks recognisable type fields (e.g. legacy / abandoned 0-byte sessions).
    if let Some(engine) = detect_engine_from_path(path) {
        return Ok(engine);
    }

    bail!(
        "could not detect engine from first 10 non-empty lines: {}",
        path.display()
    )
}

/// Parses a complete session JSONL file.
///
/// Uses the unified pipeline: JSONL → parse_events() → extract_parsed_session().
pub fn parse_session(path: &Path) -> Result<ParsedSession> {
    let engine = detect_engine(path)?;
    let events = match engine {
        Engine::Claude => claude::parse_events(path)?,
        Engine::Codex => codex::parse_events(path)?,
        Engine::Gemini => gemini::parse_events(path)?,
        Engine::Agy => agy::parse_events(path)?,
        Engine::Hermes => {
            bail!("Hermes sessions require a session id; use hermes::parse_session(db_path, id)")
        }
    };
    Ok(facts::extract_parsed_session(&events, engine, path))
}

/// Parse a discovered session, preserving source-specific addressing.
///
/// File-backed engines can be parsed from their JSONL path alone. Hermes stores
/// many logical sessions inside one SQLite state DB, so discovery must pass the
/// session id through the parser boundary.
pub fn parse_discovered_session(discovered: &DiscoveredSession) -> Result<ParsedSession> {
    match discovered.engine {
        Engine::Hermes => hermes::parse_session(&discovered.path, &discovered.id),
        _ => {
            let mut parsed = parse_session(&discovered.path)?;
            if parsed.meta.cwd.is_none() {
                parsed.meta.cwd = discovered.cwd.clone();
            }
            if parsed.meta.model.is_none() {
                parsed.meta.model = discovered.model.clone();
            }
            if parsed.meta.started_at.is_empty() {
                if let Some(started_at) = &discovered.started_at {
                    parsed.meta.started_at = started_at.clone();
                }
            }
            Ok(parsed)
        }
    }
}

/// Parses only newly appended JSONL content starting at byte `offset`.
///
/// Uses the unified pipeline: JSONL → parse_events_from_offset() → extract_parsed_session().
pub fn parse_session_incremental(path: &Path, offset: u64) -> Result<(ParsedSession, u64)> {
    let engine = detect_engine(path)?;
    let events = match engine {
        Engine::Claude => claude::parse_events_from_offset(path, offset)?,
        Engine::Codex => codex::parse_events_from_offset(path, offset)?,
        Engine::Gemini => gemini::parse_events_from_offset(path, offset)?,
        Engine::Agy => agy::parse_events_from_offset(path, offset)?,
        Engine::Hermes => {
            bail!("Hermes sessions are SQLite-backed and do not support byte-offset parsing")
        }
    };
    let parsed = facts::extract_parsed_session(&events, engine, path);

    let new_offset = std::fs::metadata(path)
        .with_context(|| format!("failed to stat session file: {}", path.display()))?
        .len();
    Ok((parsed, new_offset))
}

fn is_claude_type(value: &str) -> bool {
    matches!(
        value,
        "user" | "assistant" | "queue-operation" | "progress" | "system" | "file-history-snapshot"
    )
}

fn is_codex_type(value: &str) -> bool {
    matches!(
        value,
        "session_meta" | "response_item" | "turn_context" | "event_msg"
    )
}

/// True when a JSON object has the stable schema markers emitted by agy.
///
/// This intentionally uses record shape rather than Antigravity path names so
/// copied transcripts keep their engine identity outside the brain directory.
pub fn is_agy_record(record: &Value) -> bool {
    let Some(record_type) = record.get("type").and_then(Value::as_str) else {
        return false;
    };

    if !is_agy_type(record_type) {
        return false;
    }

    record.get("step_index").and_then(Value::as_i64).is_some()
        && (record.get("created_at").and_then(Value::as_str).is_some()
            || record.get("source").and_then(Value::as_str).is_some()
            || record.get("status").and_then(Value::as_str).is_some())
}

fn is_agy_type(value: &str) -> bool {
    matches!(
        value,
        "USER_INPUT"
            | "PLANNER_RESPONSE"
            | "VIEW_FILE"
            | "LIST_DIRECTORY"
            | "GREP_SEARCH"
            | "SEARCH_WEB"
            | "GENERATE_IMAGE"
            | "RUN_COMMAND"
            | "ERROR_MESSAGE"
    )
}

/// Extract the agy session id using the same precedence expected by native
/// Antigravity transcripts: brain UUID first, then content id fields.
pub fn extract_agy_session_id(path: &Path) -> Option<String> {
    extract_agy_session_id_from_path(path).or_else(|| extract_agy_session_id_from_content(path))
}

fn extract_agy_session_id_from_path(path: &Path) -> Option<String> {
    let mut previous_was_brain = false;
    for component in path.components() {
        let value = component.as_os_str().to_string_lossy();
        if previous_was_brain {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.chars().take(8).collect());
            }
        }
        previous_was_brain = value == "brain";
    }
    None
}

fn extract_agy_session_id_from_content(path: &Path) -> Option<String> {
    let file = File::open(path).ok()?;
    let reader = BufReader::new(file);

    for line in reader.lines().take(30).flatten() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };
        let id = value
            .get("sessionId")
            .or_else(|| value.get("session_id"))
            .or_else(|| value.get("conversation_id"))
            .or_else(|| value.get("brain_id"))
            .or_else(|| value.get("brainId"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty());
        if let Some(id) = id {
            return Some(id.chars().take(8).collect());
        }
    }

    None
}

/// Infer engine from the JSONL file's path when content-based detection fails.
///
/// Files under `~/.claude/projects/` are Claude sessions; files under
/// `~/.codex/sessions/` are Codex sessions.  This handles empty / abandoned
/// session files that contain no parseable records.
fn detect_engine_from_path(path: &Path) -> Option<Engine> {
    let path_str = path.to_str()?;
    if path_str.contains("/.claude/projects/") {
        return Some(Engine::Claude);
    }
    if path_str.contains("/.codex/sessions/") {
        return Some(Engine::Codex);
    }
    if path_str.contains("/.gemini/antigravity-cli/") {
        return Some(Engine::Agy);
    }
    if path_str.contains("/.gemini/tmp/") {
        return Some(Engine::Gemini);
    }
    None
}
