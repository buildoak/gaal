use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use serde_json::json;
use serde_json::Value;

use crate::error::GaalError;

/// Arguments for `gaal find-salt`.
#[derive(Debug, Clone)]
pub struct FindArgs {
    /// Salt token to search for.
    pub salt: String,
}

/// Find the first JSONL session file containing the provided salt token (`find-salt` command).
pub fn run(args: FindArgs) -> Result<(), GaalError> {
    let Some(home) = dirs::home_dir() else {
        return Err(GaalError::NotFound(args.salt));
    };

    let roots = [home.join(".claude").join("projects"), home.join(".codex")];

    for root in roots {
        let Some(path) = find_matching_jsonl(&root, &args.salt)? else {
            continue;
        };

        let session_id = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or_else(|| GaalError::ParseError("invalid jsonl filename".to_string()))?
            .to_string();

        let engine = infer_engine(&path);

        println!(
            "{}",
            json!({
                "session_id": session_id,
                "engine": engine,
                "jsonl_path": path,
            })
        );
        return Ok(());
    }

    Err(GaalError::NotFound(args.salt))
}

fn find_matching_jsonl(root: &Path, salt: &str) -> Result<Option<PathBuf>, GaalError> {
    if !root.exists() {
        return Ok(None);
    }

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
            if !file_type.is_file() || !is_jsonl(&path) {
                continue;
            }
            if file_contains_salt(&path, salt)? {
                return Ok(Some(path));
            }
        }
    }

    Ok(None)
}

fn file_contains_salt(path: &Path, salt: &str) -> Result<bool, GaalError> {
    let file = File::open(path).map_err(GaalError::from)?;
    let mut reader = BufReader::with_capacity(1024 * 1024, file);
    let mut line = String::new();

    loop {
        line.clear();
        let bytes_read = reader.read_line(&mut line).map_err(GaalError::from)?;
        if bytes_read == 0 {
            return Ok(false);
        }
        if line_contains_salt_output(&line, salt) {
            return Ok(true);
        }
    }
}

fn line_contains_salt_output(line: &str, salt: &str) -> bool {
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return false;
    };

    claude_salt_match(&value, salt) || codex_salt_match(&value, salt)
}

fn claude_salt_match(value: &Value, salt: &str) -> bool {
    if value
        .get("toolUseResult")
        .and_then(|result| result.get("stdout"))
        .and_then(Value::as_str)
        .is_some_and(|stdout| stdout.trim() == salt)
    {
        return true;
    }

    value
        .get("message")
        .and_then(|message| message.get("content"))
        .and_then(Value::as_array)
        .is_some_and(|items| {
            items.iter().any(|item| {
                item.get("type").and_then(Value::as_str) == Some("tool_result")
                    && item
                        .get("content")
                        .and_then(Value::as_str)
                        .is_some_and(|content| content.trim() == salt)
            })
        })
}

fn codex_salt_match(value: &Value, salt: &str) -> bool {
    let payload = match value.get("payload") {
        Some(payload) => payload,
        None => return false,
    };

    payload.get("type").and_then(Value::as_str) == Some("exec_command_end")
        && payload
            .get("aggregated_output")
            .and_then(Value::as_str)
            .is_some_and(|output| output.trim() == salt)
}

fn is_jsonl(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("jsonl"))
        .unwrap_or(false)
}

/// Infer the engine from a JSONL file path.
///
/// - `~/.claude/projects/` → "claude"
/// - `~/.codex/` → "codex"
/// - Otherwise → "unknown"
fn infer_engine(path: &Path) -> &'static str {
    let path_str = path.to_string_lossy();
    if path_str.contains("/.claude/projects/") {
        "claude"
    } else if path_str.contains("/.codex/") {
        "codex"
    } else {
        "unknown"
    }
}
