use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use serde_json::json;
use serde_json::Value;

use crate::db;
use crate::error::GaalError;

/// Arguments for `gaal find-salt`.
#[derive(Debug, Clone)]
pub struct FindArgs {
    /// Salt token to search for.
    pub salt: String,
    /// Human-readable output mode.
    pub human: bool,
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

        // Try enriching from DB
        let enriched = try_enrich(&session_id, engine, &home);

        if args.human {
            print_human(&session_id, engine, &path, &enriched);
        } else {
            print_json(&session_id, engine, &path, &enriched);
        }
        return Ok(());
    }

    Err(GaalError::NotFound(args.salt))
}

/// Enriched session data from the DB, if available.
struct Enrichment {
    model: Option<String>,
    cwd: Option<String>,
    session_type: String,
    last_event_at: Option<String>,
    turns: i64,
    total_input_tokens: i64,
    total_output_tokens: i64,
    transcript_path: Option<String>,
    transcript_exists: bool,
    handoff_exists: bool,
    handoff_generated_at: Option<String>,
}

fn to_db_id(raw_id: &str, engine: &str) -> String {
    match engine {
        "claude" => raw_id.chars().take(8).collect(),
        "codex" => {
            // Codex filenames: rollout-<timestamp>-<uuid>.jsonl
            // Extract UUID portion (last 36 chars of stem, or the whole thing),
            // strip dashes, take last 8 hex chars.
            let uuid_part = if raw_id.len() > 36 {
                &raw_id[raw_id.len() - 36..]
            } else {
                raw_id
            };
            let hex: String = uuid_part.chars().filter(|c| *c != '-').collect();
            if hex.len() > 8 {
                hex[hex.len() - 8..].to_string()
            } else {
                hex
            }
        }
        _ => raw_id.chars().take(8).collect(),
    }
}

fn try_enrich(session_id: &str, engine: &str, home: &Path) -> Option<Enrichment> {
    let conn = db::open_db_readonly().ok()?;
    let db_id = to_db_id(session_id, engine);
    let session = db::queries::get_session(&conn, &db_id).ok()??;

    // Compute transcript path (uses short DB ID, not raw filename stem)
    let (transcript_path, transcript_exists) =
        compute_transcript_path(home, engine, &session.started_at, &db_id);

    // Check for handoff (uses DB ID)
    let handoff = db::queries::get_handoff(&conn, &db_id).ok().flatten();

    Some(Enrichment {
        model: session.model,
        cwd: session.cwd,
        session_type: session.session_type,
        last_event_at: session.last_event_at,
        turns: session.total_turns,
        total_input_tokens: session.total_input_tokens,
        total_output_tokens: session.total_output_tokens,
        transcript_path,
        transcript_exists,
        handoff_exists: handoff.is_some(),
        handoff_generated_at: handoff.and_then(|h| h.generated_at),
    })
}

/// Compute expected transcript path: ~/.gaal/data/{engine}/sessions/YYYY/MM/DD/{short_id}.md
fn compute_transcript_path(
    home: &Path,
    engine: &str,
    started_at: &str,
    session_id: &str,
) -> (Option<String>, bool) {
    // Parse YYYY-MM-DD from started_at (RFC3339 or date prefix)
    if started_at.len() < 10 {
        return (None, false);
    }
    let date_part = &started_at[..10]; // "YYYY-MM-DD"
    let parts: Vec<&str> = date_part.split('-').collect();
    if parts.len() != 3 {
        return (None, false);
    }

    let short_id = &session_id[..session_id.len().min(8)];
    let gaal_home = std::env::var("GAAL_HOME")
        .ok()
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".gaal"));

    let path = gaal_home
        .join("data")
        .join(engine)
        .join("sessions")
        .join(parts[0])
        .join(parts[1])
        .join(parts[2])
        .join(format!("{short_id}.md"));

    let exists = path.exists();
    (Some(path.to_string_lossy().to_string()), exists)
}

fn print_json(session_id: &str, engine: &str, jsonl_path: &Path, enriched: &Option<Enrichment>) {
    let output = match enriched {
        Some(e) => {
            let total_tokens = e.total_input_tokens + e.total_output_tokens;
            let mut obj = json!({
                "session_id": session_id,
                "engine": engine,
                "jsonl_path": jsonl_path,
                "indexed": true,
                "model": e.model,
                "cwd": e.cwd,
                "session_type": e.session_type,
                "last_event_at": e.last_event_at,
                "turns": e.turns,
                "total_tokens": total_tokens,
                "input_tokens": e.total_input_tokens,
                "output_tokens": e.total_output_tokens,
            });
            if let Some(tp) = &e.transcript_path {
                obj["transcript_path"] = json!(tp);
                obj["transcript_exists"] = json!(e.transcript_exists);
            }
            obj["handoff"] = json!({
                "exists": e.handoff_exists,
                "generated_at": e.handoff_generated_at,
            });
            obj
        }
        None => json!({
            "session_id": session_id,
            "engine": engine,
            "jsonl_path": jsonl_path,
            "indexed": false,
        }),
    };
    println!("{output}");
}

fn print_human(session_id: &str, engine: &str, jsonl_path: &Path, enriched: &Option<Enrichment>) {
    match enriched {
        Some(e) => {
            let total_tokens = e.total_input_tokens + e.total_output_tokens;
            let model_label = e.model.as_deref().unwrap_or("unknown");
            let cwd_label = e.cwd.as_deref().unwrap_or("unknown");
            let last_label = e.last_event_at.as_deref().unwrap_or("unknown");
            let total_k = format_tokens_k(total_tokens);
            let in_k = format_tokens_k(e.total_input_tokens);
            let out_k = format_tokens_k(e.total_output_tokens);

            println!("Session: {session_id}");
            println!("Engine:  {engine} ({model_label})");
            println!("Type:    {}", e.session_type);
            println!("CWD:     {cwd_label}");
            println!("Tokens:  {total_k} ({in_k} in / {out_k} out) | {} turns", e.turns);
            println!("Last:    {last_label}");
            println!("JSONL:   {}", jsonl_path.display());

            if let Some(tp) = &e.transcript_path {
                let exists_label = if e.transcript_exists { "" } else { " (not rendered)" };
                println!("Transcript: {tp}{exists_label}");
            }

            let handoff_label = if e.handoff_exists {
                let gen = e
                    .handoff_generated_at
                    .as_deref()
                    .unwrap_or("unknown time");
                format!("yes (generated {gen})")
            } else {
                "no".to_string()
            };
            println!("Handoff: {handoff_label}");
        }
        None => {
            println!("Session: {session_id}");
            println!("Engine:  {engine}");
            println!("JSONL:   {}", jsonl_path.display());
            println!("Status:  not indexed (run 'gaal index backfill' to index)");
        }
    }
}

fn format_tokens_k(tokens: i64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{}K", tokens / 1_000)
    } else {
        format!("{tokens}")
    }
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
