use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::discovery::discover::{read_head_lines, DiscoveredSession};
use crate::parser::types::Engine;

const HEAD_LINES: usize = 30;

/// Discover Codex session JSONL files from `~/.codex/sessions` recursively.
pub fn discover_codex_sessions() -> Result<Vec<DiscoveredSession>> {
    let Some(home) = dirs::home_dir() else {
        return Ok(Vec::new());
    };
    let root = home.join(".codex").join("sessions");
    if !root.exists() {
        return Ok(Vec::new());
    }

    let mut sessions = Vec::new();
    for path in collect_rollout_jsonl_files(&root) {
        let Ok(meta) = fs::metadata(&path) else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }

        let head = read_head_lines(&path, HEAD_LINES);
        let CodexHead {
            id,
            model,
            cwd,
            started_at,
            forked_from_id,
        } = parse_codex_head(&head);
        let fallback_id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();

        // Codex uses UUIDv7 (shared timestamp prefix, unique random suffix).
        // Truncate to last 8 hex chars of the dash-stripped UUID for short IDs.
        // Claude uses first-8 (UUIDv4, random throughout).
        let short_id = truncate_codex_id(&id.unwrap_or(fallback_id));

        sessions.push(DiscoveredSession {
            id: short_id,
            engine: Engine::Codex,
            path,
            model,
            cwd,
            started_at,
            forked_from_id,
            file_size: meta.len(),
        });
    }

    Ok(sessions)
}

fn collect_rollout_jsonl_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };

        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            let path = entry.path();

            if file_type.is_dir() {
                stack.push(path);
                continue;
            }
            if !file_type.is_file() {
                continue;
            }

            let is_rollout = path
                .file_name()
                .and_then(|name| name.to_str())
                .map(|name| name.starts_with("rollout-") && name.ends_with(".jsonl"))
                .unwrap_or(false);

            if is_rollout {
                out.push(path);
            }
        }
    }

    out
}

fn parse_codex_head(lines: &[String]) -> CodexHead {
    let mut id: Option<String> = None;
    let mut model: Option<String> = None;
    let mut cwd: Option<String> = None;
    let mut started_at: Option<String> = None;
    let mut forked_from_id: Option<String> = None;

    for line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(record) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            continue;
        };

        if started_at.is_none() {
            started_at = record
                .pointer("/payload/timestamp")
                .or_else(|| record.get("timestamp"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
        }
        if id.is_none() {
            id = record
                .pointer("/payload/id")
                .or_else(|| record.get("session_id"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
        }
        if forked_from_id.is_none() {
            forked_from_id = record
                .pointer("/payload/forked_from_id")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
        }
        if cwd.is_none() {
            cwd = record
                .pointer("/payload/cwd")
                .or_else(|| record.get("cwd"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
        }
        if model.is_none() {
            model = record
                .pointer("/payload/model")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
        }

        if id.is_some() && model.is_some() && cwd.is_some() && started_at.is_some() {
            break;
        }
    }

    CodexHead {
        id,
        model,
        cwd,
        started_at,
        forked_from_id,
    }
}

struct CodexHead {
    id: Option<String>,
    model: Option<String>,
    cwd: Option<String>,
    started_at: Option<String>,
    forked_from_id: Option<String>,
}

/// Truncate a Codex session ID (UUIDv7) to its last 8 hex characters.
///
/// UUIDv7 shares a timestamp prefix across sessions started in the same
/// millisecond; the random suffix is what provides uniqueness. Stripping
/// dashes and taking the last 8 hex chars gives a short, collision-free ID.
pub fn truncate_codex_id(raw: &str) -> String {
    let hex: String = raw.chars().filter(|c| *c != '-').collect();
    if hex.len() > 8 {
        hex[hex.len() - 8..].to_string()
    } else {
        hex
    }
}
