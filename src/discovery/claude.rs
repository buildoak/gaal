use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::discovery::discover::{read_head_lines, DiscoveredSession};
use crate::parser::types::Engine;

const HEAD_LINES: usize = 30;

/// Discover Claude session JSONL files from `~/.claude/projects/<hash>/*.jsonl`.
pub fn discover_claude_sessions() -> Result<Vec<DiscoveredSession>> {
    let Some(home) = dirs::home_dir() else {
        return Ok(Vec::new());
    };
    let root = home.join(".claude").join("projects");
    if !root.exists() {
        return Ok(Vec::new());
    }

    let mut sessions = Vec::new();
    for path in collect_project_jsonl_files(&root) {
        let Ok(meta) = fs::metadata(&path) else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }

        let head = read_head_lines(&path, HEAD_LINES);
        let (id, model, cwd, started_at) = parse_claude_head(&head);
        let fallback_id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();

        let raw_id = id.unwrap_or(fallback_id);
        let short_id: String = raw_id.chars().take(8).collect();

        sessions.push(DiscoveredSession {
            id: short_id,
            engine: Engine::Claude,
            path,
            model,
            cwd,
            started_at,
            file_size: meta.len(),
        });
    }

    Ok(sessions)
}

fn collect_project_jsonl_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let Ok(project_dirs) = fs::read_dir(root) else {
        return files;
    };

    for project_entry in project_dirs.flatten() {
        let Ok(ft) = project_entry.file_type() else {
            continue;
        };
        if !ft.is_dir() {
            continue;
        }
        let project_path = project_entry.path();
        let Ok(entries) = fs::read_dir(&project_path) else {
            continue;
        };

        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if !file_type.is_file() {
                continue;
            }

            let path = entry.path();
            let is_jsonl = path
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| ext.eq_ignore_ascii_case("jsonl"))
                .unwrap_or(false);

            if is_jsonl {
                files.push(path);
            }
        }
    }

    files
}

fn parse_claude_head(lines: &[String]) -> ClaudeHead {
    let mut id: Option<String> = None;
    let mut model: Option<String> = None;
    let mut cwd: Option<String> = None;
    let mut started_at: Option<String> = None;

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
                .get("timestamp")
                .and_then(serde_json::Value::as_str)
                .or_else(|| {
                    record
                        .pointer("/snapshot/timestamp")
                        .and_then(serde_json::Value::as_str)
                })
                .map(str::to_string);
        }
        if id.is_none() {
            id = record
                .get("sessionId")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
        }
        if cwd.is_none() {
            cwd = record
                .get("cwd")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
        }
        if model.is_none() {
            model = record
                .pointer("/message/model")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
        }

        if id.is_some() && model.is_some() && cwd.is_some() && started_at.is_some() {
            break;
        }
    }

    (id, model, cwd, started_at)
}

type ClaudeHead = (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);
