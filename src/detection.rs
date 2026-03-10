use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::GaalError;

#[derive(Debug, Clone)]
pub struct DetectedSession {
    pub engine: String,
    pub session_id: String,
    pub jsonl_path: PathBuf,
    pub pid: u32,
}

/// Returns all agent session candidates along the PID ancestor chain.
/// First element is closest to gaal (likely child), last is furthest (likely parent).
pub fn detect_session_candidates() -> Result<Vec<DetectedSession>, GaalError> {
    let mut current = std::process::id();
    let mut candidates = Vec::new();

    for _ in 0..20 {
        let Some(name) = get_process_name(current) else {
            break;
        };
        let engine = name.to_ascii_lowercase();
        if engine == "claude" || engine == "codex" {
            // Try lsof first (works if the process has the JSONL open),
            // then fall back to CWD-based resolution (Claude Code doesn't
            // keep the JSONL open permanently).
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
        Err(GaalError::Internal("Could not detect current session. Provide a session ID, use 'today', or run from within a Claude Code session.".to_string()))
    } else {
        Ok(candidates)
    }
}

pub fn detect_current_session() -> Result<DetectedSession, GaalError> {
    detect_session_candidates()?
        .into_iter()
        .next()
        .ok_or_else(|| {
            GaalError::Internal(
                "Could not detect current session. Provide a session ID, use 'today', or run from within a Claude Code session.".to_string(),
            )
        })
}

/// Detect the preferred session for handoff.
/// Parent-child preference is permanently disabled; this returns the current detected session.
pub fn detect_preferred_session() -> Result<DetectedSession, GaalError> {
    detect_current_session()
}

pub fn get_ppid(pid: u32) -> Option<u32> {
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

pub fn get_process_name(pid: u32) -> Option<String> {
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

pub fn resolve_jsonl_for_pid(pid: u32) -> Option<PathBuf> {
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

/// Resolve the JSONL path by finding the CWD of the agent process, then
/// looking up the most recently modified JSONL in the Claude/Codex projects
/// directory that matches that CWD.
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
            // Try canonicalized path
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
            // Codex stores sessions in ~/.codex/sessions/
            let sessions_dir = home.join(".codex").join("sessions");
            latest_jsonl_in_dir(&sessions_dir)
        }
        _ => None,
    }
}

/// Resolve the current working directory of a PID.
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

/// Find the most recently modified .jsonl file in a directory.
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

pub fn extract_session_id_from_jsonl(path: &Path, engine: &str) -> Option<String> {
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
