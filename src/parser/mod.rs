use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use anyhow::{bail, Context, Result};
use serde_json::Value;

pub mod claude;
pub mod common;
pub mod codex;
pub mod event;
pub mod facts;
pub mod types;

pub use event::{SessionEvent, EventKind, ToolUseEvent, ContentBlock};
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

        let record_type = record
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();

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
    };
    Ok(facts::extract_parsed_session(&events, engine, path))
}

/// Parses only newly appended JSONL content starting at byte `offset`.
///
/// Uses the unified pipeline: JSONL → parse_events_from_offset() → extract_parsed_session().
pub fn parse_session_incremental(path: &Path, offset: u64) -> Result<(ParsedSession, u64)> {
    let engine = detect_engine(path)?;
    let events = match engine {
        Engine::Claude => claude::parse_events_from_offset(path, offset)?,
        Engine::Codex => codex::parse_events_from_offset(path, offset)?,
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
    None
}
