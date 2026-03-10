use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::parser::types::Engine;

/// Live process metrics for an agent session PID.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessInfo {
    /// Process ID.
    pub pid: u32,
    /// CPU percentage reported by `ps`.
    pub cpu_pct: f64,
    /// Resident set size in MB.
    pub rss_mb: f64,
}

/// Active (running) agent session detected from live processes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveSession {
    /// Session ID when discoverable from JSONL head records.
    pub id: Option<String>,
    /// Source engine.
    pub engine: Engine,
    /// Process ID.
    pub pid: u32,
    /// Process current working directory.
    pub cwd: String,
    /// Best-effort resolved JSONL path.
    pub jsonl_path: Option<PathBuf>,
    /// Process runtime metrics.
    pub process: ProcessInfo,
    /// Owning tmux session name, if found.
    pub tmux_session: Option<String>,
}

/// Probe process metrics for a PID on Unix systems.
pub fn probe_pid(pid: u32) -> Option<ProcessInfo> {
    #[cfg(target_os = "macos")]
    {
        let output = Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "pid=,pcpu=,rss="])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }

        let line = String::from_utf8_lossy(&output.stdout)
            .lines()
            .find(|l| !l.trim().is_empty())?
            .to_string();
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 3 {
            return None;
        }

        let parsed_pid = cols[0].parse::<u32>().ok()?;
        let cpu_pct = cols[1].parse::<f64>().ok()?;
        let rss_kb = cols[2].parse::<f64>().ok()?;

        Some(ProcessInfo {
            pid: parsed_pid,
            cpu_pct,
            rss_mb: rss_kb / 1024.0,
        })
    }
    #[cfg(target_os = "linux")]
    {
        let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let parsed_pid = stat.split_whitespace().next()?.parse::<u32>().ok()?;
        let _ = linux_cpu_ticks(&stat)?;

        let status = fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
        let rss_kb = status
            .lines()
            .find_map(|line| {
                line.strip_prefix("VmRSS:")
                    .and_then(|rest| rest.split_whitespace().next())
                    .and_then(|value| value.parse::<f64>().ok())
            })
            .unwrap_or(0.0);

        Some(ProcessInfo {
            pid: parsed_pid,
            cpu_pct: 0.0,
            rss_mb: rss_kb / 1024.0,
        })
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        None
    }
}

/// Check whether a PID exists (`kill(pid, 0)`).
pub fn is_pid_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    #[cfg(unix)]
    {
        // SAFETY: kill with signal 0 performs existence/permission check only.
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

/// Discover currently running Claude/Codex sessions from process state,
/// then also check for API-spawned Codex sessions (no live process) via
/// recent JSONL file mtime.
pub fn find_active_sessions() -> Result<Vec<ActiveSession>> {
    #[cfg(unix)]
    {
        let mut sessions = Vec::new();
        let mut discovered_jsonl_paths: HashSet<PathBuf> = HashSet::new();

        for (pid, engine) in list_agent_processes() {
            if !is_pid_alive(pid) {
                continue;
            }
            let Some(process) = probe_pid(pid) else {
                continue;
            };
            let Some(cwd) = resolve_cwd(pid) else {
                continue;
            };

            let jsonl_path = map_cwd_to_jsonl(&engine, &cwd);
            let id = jsonl_path
                .as_ref()
                .and_then(|path| extract_session_id(path, &engine));
            let tmux_session = find_tmux_session(pid);

            if let Some(path) = jsonl_path.as_ref() {
                discovered_jsonl_paths.insert(path.clone());
            }

            sessions.push(ActiveSession {
                id,
                engine,
                pid,
                cwd,
                jsonl_path,
                process,
                tmux_session,
            });
        }

        // Check for API-spawned Codex sessions: JSONL files in
        // ~/.codex/sessions/ with mtime < 5 minutes and no matching
        // process-discovered session.
        if let Some(api_sessions) = discover_api_active_codex_sessions(&discovered_jsonl_paths) {
            sessions.extend(api_sessions);
        }

        Ok(sessions)
    }
    #[cfg(not(unix))]
    {
        Ok(Vec::new())
    }
}

/// Scan `~/.codex/sessions/` for JSONL files modified within the last 5
/// minutes that were not already discovered via process inspection.
/// These represent API-spawned Codex sessions (via agent-mux) that have
/// no live process on this machine.
fn discover_api_active_codex_sessions(
    already_discovered: &HashSet<PathBuf>,
) -> Option<Vec<ActiveSession>> {
    let home = dirs::home_dir()?;
    let sessions_root = home.join(".codex").join("sessions");
    if !sessions_root.exists() {
        return None;
    }

    let five_minutes = std::time::Duration::from_secs(5 * 60);
    let mut results = Vec::new();

    // Walk directories recursively looking for rollout-*.jsonl files.
    let mut stack = vec![sessions_root];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(ft) = entry.file_type() else { continue };
            let path = entry.path();

            if ft.is_dir() {
                stack.push(path);
                continue;
            }
            if !ft.is_file() {
                continue;
            }

            let is_rollout = path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("rollout-") && n.ends_with(".jsonl"))
                .unwrap_or(false);
            if !is_rollout {
                continue;
            }

            // Skip files already matched to a live process.
            if already_discovered.contains(&path) {
                continue;
            }

            // Check mtime is within 5 minutes.
            let Ok(meta) = fs::metadata(&path) else {
                continue;
            };
            let Ok(modified) = meta.modified() else {
                continue;
            };
            let Ok(elapsed) = modified.elapsed() else {
                continue;
            };
            if elapsed > five_minutes {
                continue;
            }

            // Parse head to get session metadata.
            let head = super::discover::read_head_lines(&path, 30);
            let (id, _model, cwd) = parse_codex_api_head(&head);

            let session_id = id.map(|raw| truncate_codex_id(&raw));
            let cwd_str = cwd.unwrap_or_else(|| "unknown".to_string());

            results.push(ActiveSession {
                id: session_id,
                engine: Engine::Codex,
                pid: 0,
                cwd: cwd_str,
                jsonl_path: Some(path),
                process: ProcessInfo {
                    pid: 0,
                    cpu_pct: 0.0,
                    rss_mb: 0.0,
                },
                tmux_session: None,
            });
        }
    }

    if results.is_empty() {
        None
    } else {
        Some(results)
    }
}

/// Parse head lines of a Codex JSONL to extract session ID, model, and CWD.
fn parse_codex_api_head(lines: &[String]) -> (Option<String>, Option<String>, Option<String>) {
    let mut id: Option<String> = None;
    let mut model: Option<String> = None;
    let mut cwd: Option<String> = None;

    for line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(record) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            continue;
        };

        if id.is_none() {
            id = record
                .pointer("/payload/id")
                .or_else(|| record.get("session_id"))
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

        if id.is_some() && model.is_some() && cwd.is_some() {
            break;
        }
    }

    (id, model, cwd)
}

/// Truncate a Codex session ID (UUIDv7) to its last 8 hex characters.
fn truncate_codex_id(raw: &str) -> String {
    let hex: String = raw.chars().filter(|c| *c != '-').collect();
    if hex.len() > 8 {
        hex[hex.len() - 8..].to_string()
    } else {
        hex
    }
}

/// Resolve tmux session name that owns the target PID.
pub fn find_tmux_session(pid: u32) -> Option<String> {
    let output = Command::new("tmux")
        .args(["list-panes", "-a", "-F", "#{session_name}\t#{pane_pid}"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let mut pane_owners: HashMap<u32, String> = HashMap::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let mut parts = line.split('\t');
        let Some(session) = parts.next() else {
            continue;
        };
        let Some(pane_pid_raw) = parts.next() else {
            continue;
        };
        let Ok(pane_pid) = pane_pid_raw.trim().parse::<u32>() else {
            continue;
        };
        pane_owners.insert(pane_pid, session.to_string());
    }

    if let Some(owner) = pane_owners.get(&pid) {
        return Some(owner.clone());
    }

    let mut cur = pid;
    for _ in 0..64 {
        let parent = parent_pid(cur)?;
        if let Some(owner) = pane_owners.get(&parent) {
            return Some(owner.clone());
        }
        if parent <= 1 || parent == cur {
            break;
        }
        cur = parent;
    }
    None
}

#[cfg(unix)]
mod libc {
    unsafe extern "C" {
        pub fn kill(pid: i32, sig: i32) -> i32;
    }
}

#[cfg(unix)]
fn list_agent_processes() -> Vec<(u32, Engine)> {
    let mut found = Vec::new();
    let mut seen = HashSet::new();
    let excluded = excluded_pids();

    collect_pgrep_matches("claude", &excluded, &mut seen, &mut found);
    collect_pgrep_matches("codex", &excluded, &mut seen, &mut found);
    collect_pgrep_matches("codex-cli", &excluded, &mut seen, &mut found);
    collect_pgrep_matches("codex-rs", &excluded, &mut seen, &mut found);

    let output = match Command::new("ps").args(["aux"]).output() {
        Ok(output) if output.status.success() => output,
        _ => return found,
    };

    for line in String::from_utf8_lossy(&output.stdout).lines().skip(1) {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 11 {
            continue;
        }
        let Ok(pid) = cols[1].parse::<u32>() else {
            continue;
        };
        if excluded.contains(&pid) || seen.contains(&pid) {
            continue;
        }

        let cmd = cols[10..].join(" ");
        let Some(engine) = engine_from_ps_command(cols[10], &cmd) else {
            continue;
        };

        seen.insert(pid);
        found.push((pid, engine));
    }

    found
}

#[cfg(unix)]
fn collect_pgrep_matches(
    process_name: &str,
    excluded: &HashSet<u32>,
    seen: &mut HashSet<u32>,
    out: &mut Vec<(u32, Engine)>,
) {
    let output = match Command::new("pgrep").args(["-x", process_name]).output() {
        Ok(output) if output.status.success() => output,
        _ => return,
    };

    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let Ok(pid) = line.trim().parse::<u32>() else {
            continue;
        };
        if excluded.contains(&pid) || !seen.insert(pid) {
            continue;
        }

        let engine = if process_name == "claude" {
            Engine::Claude
        } else if process_name == "codex"
            || process_name == "codex-cli"
            || process_name == "codex-rs"
        {
            Engine::Codex
        } else {
            Engine::Codex
        };
        out.push((pid, engine));
    }
}

/// Build the set of PIDs to exclude from process discovery.
///
/// We only exclude gaal's own PID.  We intentionally do NOT walk the parent
/// chain because gaal is often invoked *inside* a Claude/Codex session (e.g.
/// via a Bash tool call).  Excluding parent PIDs would hide the very session
/// the user is running inside of.
#[cfg(unix)]
fn excluded_pids() -> HashSet<u32> {
    let mut excluded = HashSet::new();
    excluded.insert(std::process::id());
    excluded
}

/// Determine engine from a `ps aux` output row.
///
/// `cmd0` is the first token of the command column (cols[10]), usually the
/// executable path.  `full_cmd` is the entire command string.
///
/// We are deliberately **very strict** in this fallback path.  The primary
/// discovery mechanism is `pgrep -x claude` / `pgrep -x codex` which matches
/// only processes whose executable name is exactly "claude" or "codex".  This
/// function only fires for rows that `pgrep` somehow missed.  It must reject:
///
/// - Claude Desktop App (`/Applications/Claude.app/...`)
/// - Claude Agent SDK child workers (`bun .../claude-agent-sdk/...`)
/// - Shell-snapshot bash commands spawned by Claude Code
/// - tmux sessions whose name contains "claude"
/// - Python/Node daemons that happen to live under a `.claude/` directory
/// - Any Electron helper processes
#[cfg(unix)]
fn engine_from_ps_command(cmd0: &str, full_cmd: &str) -> Option<Engine> {
    let lowered_cmd = full_cmd.to_ascii_lowercase();

    // Hard-reject patterns that are never real CLI sessions.
    const REJECT_PATTERNS: &[&str] = &[
        ".app/contents/",   // macOS .app bundles (Claude Desktop, Electron helpers)
        "claude-agent-sdk", // Subagent child workers
        "shell-snapshots",  // Tool-execution shell wrappers
        "tmux",             // tmux attach/new-session commands
        "python",           // Python daemons in .claude/ dirs
        "bun ",             // Bun-run SDK workers
        "node ",            // Node-run SDK workers
        "crashpad",         // Crash reporter helpers
        "--type=",          // Electron/Chromium child processes
    ];

    if REJECT_PATTERNS.iter().any(|pat| lowered_cmd.contains(pat)) {
        return None;
    }

    // Only match if the basename of the executable is exactly "claude" or "codex".
    let basename = cmd0.rsplit('/').next().unwrap_or(cmd0).to_ascii_lowercase();

    if basename == "claude" {
        return Some(Engine::Claude);
    }
    if basename == "codex" || basename == "codex-cli" || basename == "codex-rs" {
        return Some(Engine::Codex);
    }

    None
}

#[cfg(target_os = "macos")]
fn resolve_cwd(pid: u32) -> Option<String> {
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
fn resolve_cwd(pid: u32) -> Option<String> {
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
fn resolve_cwd(_pid: u32) -> Option<String> {
    None
}

#[cfg(target_os = "macos")]
fn parent_pid(pid: u32) -> Option<u32> {
    let output = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "ppid="])
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

#[cfg(target_os = "linux")]
fn parent_pid(pid: u32) -> Option<u32> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after_comm = stat.rsplit_once(") ")?.1;
    let mut fields = after_comm.split_whitespace();
    let _state = fields.next()?;
    fields.next()?.parse::<u32>().ok()
}

#[cfg(not(unix))]
fn parent_pid(_pid: u32) -> Option<u32> {
    None
}

#[cfg(target_os = "linux")]
fn linux_cpu_ticks(stat: &str) -> Option<(u64, u64)> {
    let after_comm = stat.rsplit_once(") ")?.1;
    let mut fields = after_comm.split_whitespace();

    let _state = fields.next()?;
    let _ppid = fields.next()?;
    for _ in 0..9 {
        fields.next()?;
    }

    let utime = fields.next()?.parse::<u64>().ok()?;
    let stime = fields.next()?.parse::<u64>().ok()?;
    Some((utime, stime))
}

fn map_cwd_to_jsonl(engine: &Engine, cwd: &str) -> Option<PathBuf> {
    match engine {
        Engine::Claude => map_claude_cwd_to_jsonl(cwd),
        Engine::Codex => map_codex_cwd_to_jsonl(cwd),
    }
}

fn map_claude_cwd_to_jsonl(cwd: &str) -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let projects_root = home.join(".claude").join("projects");

    let encoded = encode_claude_project_dir(cwd);
    if let Some(path) = latest_jsonl_in_dir(&projects_root.join(&encoded)) {
        return Some(path);
    }

    if let Ok(real) = fs::canonicalize(cwd) {
        if let Some(real_cwd) = real.to_str() {
            let encoded_real = encode_claude_project_dir(real_cwd);
            if let Some(path) = latest_jsonl_in_dir(&projects_root.join(encoded_real)) {
                return Some(path);
            }
        }
    }

    let Ok(discovered) = super::claude::discover_claude_sessions() else {
        return None;
    };
    discovered
        .into_iter()
        .filter(|s| same_cwd(s.cwd.as_deref(), cwd))
        .max_by(|a, b| a.started_at.cmp(&b.started_at))
        .map(|s| s.path)
}

fn map_codex_cwd_to_jsonl(cwd: &str) -> Option<PathBuf> {
    let Ok(discovered) = super::codex::discover_codex_sessions() else {
        return None;
    };
    discovered
        .into_iter()
        .filter(|s| same_cwd(s.cwd.as_deref(), cwd))
        .max_by(|a, b| a.started_at.cmp(&b.started_at))
        .map(|s| s.path)
}

fn same_cwd(candidate: Option<&str>, target: &str) -> bool {
    let Some(candidate) = candidate else {
        return false;
    };
    normalize_path(candidate) == normalize_path(target)
}

fn normalize_path(path: &str) -> String {
    path.trim_end_matches('/').to_string()
}

fn encode_claude_project_dir(cwd: &str) -> String {
    cwd.replace('/', "-")
}

fn latest_jsonl_in_dir(dir: &Path) -> Option<PathBuf> {
    let entries = fs::read_dir(dir).ok()?;
    let mut newest: Option<(std::time::SystemTime, PathBuf)> = None;

    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(ft) = entry.file_type() else {
            continue;
        };
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

fn extract_session_id(path: &Path, engine: &Engine) -> Option<String> {
    let lines = super::discover::read_head_lines(path, 30);
    for line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(record) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            continue;
        };

        match engine {
            Engine::Claude => {
                if let Some(id) = record
                    .get("sessionId")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
                {
                    return Some(id);
                }
            }
            Engine::Codex => {
                if let Some(id) = record
                    .pointer("/payload/id")
                    .or_else(|| record.get("session_id"))
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
                {
                    return Some(id);
                }
            }
        }
    }
    None
}
