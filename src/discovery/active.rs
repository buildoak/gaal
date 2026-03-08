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

/// Discover currently running Claude/Codex sessions from process state.
pub fn find_active_sessions() -> Result<Vec<ActiveSession>> {
    #[cfg(unix)]
    {
        let mut sessions = Vec::new();
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
        Ok(sessions)
    }
    #[cfg(not(unix))]
    {
        Ok(Vec::new())
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

    if REJECT_PATTERNS
        .iter()
        .any(|pat| lowered_cmd.contains(pat))
    {
        return None;
    }

    // Only match if the basename of the executable is exactly "claude" or "codex".
    let basename = cmd0
        .rsplit('/')
        .next()
        .unwrap_or(cmd0)
        .to_ascii_lowercase();

    if basename == "claude" {
        return Some(Engine::Claude);
    }
    if basename == "codex" {
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
