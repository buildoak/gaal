use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use anyhow::{bail, Context, Result};
use serde_json::Value;

pub mod claude;
pub mod common;
pub mod codex;
pub mod types;

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

    bail!(
        "could not detect engine from first 10 non-empty lines: {}",
        path.display()
    )
}

/// Parses a complete session JSONL file.
pub fn parse_session(path: &Path) -> Result<ParsedSession> {
    match detect_engine(path)? {
        Engine::Claude => claude::parse(path),
        Engine::Codex => codex::parse(path),
    }
}

/// Parses only newly appended JSONL content starting at byte `offset`.
pub fn parse_session_incremental(path: &Path, offset: u64) -> Result<(ParsedSession, u64)> {
    let parsed = match detect_engine(path)? {
        Engine::Claude => claude::parse_from_offset(path, offset),
        Engine::Codex => codex::parse_from_offset(path, offset),
    }?;

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
