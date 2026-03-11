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
    /// Process ID (representative — highest CPU among all_pids).
    pub pid: u32,
    /// All PIDs associated with this session (after dedup).
    pub all_pids: Vec<u32>,
    /// PID of the parent session's process, if this is a child worker.
    pub parent_pid: Option<u32>,
    /// Process current working directory.
    pub cwd: String,
    /// Best-effort resolved JSONL path.
    pub jsonl_path: Option<PathBuf>,
    /// Process runtime metrics.
    pub process: ProcessInfo,
    /// Owning tmux session name, if found.
    pub tmux_session: Option<String>,
    /// One-line summary of what the session is doing.
    pub summary: Option<String>,
}

/// Probe process metrics for a PID on Unix systems.
pub fn probe_pid(pid: u32) -> Option<ProcessInfo> {
    #[cfg(target_os = "macos")]
    {
        // Try proc_pidinfo first for better performance
        if let Some(info) = probe_pid_native_macos(pid) {
            return Some(info);
        }

        // Fall back to ps if native fails
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

        // Get CPU percentage from stat file
        let cpu_pct = linux_cpu_percentage(pid).unwrap_or(0.0);

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
            cpu_pct,
            rss_mb: rss_kb / 1024.0,
        })
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        None
    }
}

#[cfg(target_os = "macos")]
fn probe_pid_native_macos(pid: u32) -> Option<ProcessInfo> {
    use std::os::raw::{c_int, c_char};
    use std::mem;

    extern "C" {
        fn proc_pidinfo(pid: c_int, flavor: c_int, arg: u64, buffer: *mut c_char, buffersize: c_int) -> c_int;
    }

    // PROC_PIDTASKINFO = 4, proc_taskinfo struct
    const PROC_PIDTASKINFO: c_int = 4;

    #[repr(C)]
    struct ProcTaskInfo {
        pti_virtual_size: u64,
        pti_resident_size: u64,
        pti_total_user: u64,
        pti_total_system: u64,
        pti_threads_user: u64,
        pti_threads_system: u64,
        pti_policy: i32,
        pti_faults: i32,
        pti_pageins: i32,
        pti_cow_faults: i32,
        pti_messages_sent: i32,
        pti_messages_received: i32,
        pti_syscalls_mach: i32,
        pti_syscalls_unix: i32,
        pti_csw: i32,
        pti_threadnum: i32,
        pti_numrunning: i32,
        pti_priority: i32,
    }

    let mut task_info: ProcTaskInfo = unsafe { mem::zeroed() };
    let ret = unsafe {
        proc_pidinfo(
            pid as c_int,
            PROC_PIDTASKINFO,
            0,
            &mut task_info as *mut _ as *mut c_char,
            mem::size_of::<ProcTaskInfo>() as c_int,
        )
    };

    if ret <= 0 {
        return None;
    }

    // Convert resident size from bytes to MB
    let rss_mb = task_info.pti_resident_size as f64 / (1024.0 * 1024.0);

    // CPU calculation is complex for proc_pidinfo, fall back to ps for CPU
    let cpu_pct = get_cpu_via_ps(pid).unwrap_or(0.0);

    Some(ProcessInfo {
        pid,
        cpu_pct,
        rss_mb,
    })
}

#[cfg(target_os = "macos")]
fn get_cpu_via_ps(pid: u32) -> Option<f64> {
    let output = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "pcpu="])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let line = String::from_utf8_lossy(&output.stdout);
    line.trim().parse::<f64>().ok()
}

#[cfg(target_os = "linux")]
fn linux_cpu_percentage(pid: u32) -> Option<f64> {
    // Read /proc/stat for system CPU time
    let system_stat = fs::read_to_string("/proc/stat").ok()?;
    let system_line = system_stat.lines().next()?;
    let system_parts: Vec<&str> = system_line.split_whitespace().collect();
    if system_parts.len() < 8 {
        return None;
    }

    let total_system_time: u64 = system_parts[1..8]
        .iter()
        .filter_map(|s| s.parse::<u64>().ok())
        .sum();

    // Read /proc/pid/stat for process CPU time
    let proc_stat = fs::read_to_string(format!("/proc/{}/stat", pid)).ok()?;
    let proc_parts: Vec<&str> = proc_stat.split_whitespace().collect();
    if proc_parts.len() < 17 {
        return None;
    }

    let utime = proc_parts[13].parse::<u64>().ok()?;
    let stime = proc_parts[14].parse::<u64>().ok()?;
    let cutime = proc_parts[15].parse::<u64>().ok()?;
    let cstime = proc_parts[16].parse::<u64>().ok()?;

    let total_process_time = utime + stime + cutime + cstime;

    // Calculate CPU percentage (this is a simplified version)
    // In reality, you'd need to take measurements over time
    let hz = 100; // Typical system Hz, could read from sysconf
    let cpu_usage = (total_process_time as f64 / hz as f64) / 1.0; // Simplified

    Some(cpu_usage.min(100.0))
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
/// recent JSONL file mtime. Applies dedup (I28), child collapsing (I29),
/// ghost filtering (I30), and summary extraction (I20).
pub fn find_active_sessions() -> Result<Vec<ActiveSession>> {
    #[cfg(unix)]
    {
        let mut sessions = Vec::new();
        let mut discovered_jsonl_paths: HashSet<PathBuf> = HashSet::new();
        // Map from PID → index in sessions vec, for parent-child detection.
        let mut pid_to_idx: HashMap<u32, usize> = HashMap::new();

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

            let jsonl_path = map_cwd_to_jsonl(&engine, &cwd, pid);
            let id = jsonl_path
                .as_ref()
                .and_then(|path| extract_session_id(path, &engine));
            let tmux_session = find_tmux_session(pid);

            if let Some(path) = jsonl_path.as_ref() {
                discovered_jsonl_paths.insert(path.clone());
            }

            let summary = extract_summary(id.as_deref(), jsonl_path.as_deref(), &cwd);

            let idx = sessions.len();
            pid_to_idx.insert(pid, idx);

            sessions.push(ActiveSession {
                id,
                engine,
                pid,
                all_pids: vec![pid],
                parent_pid: None,
                cwd,
                jsonl_path,
                process,
                tmux_session,
                summary,
            });
        }

        // I29: Child worker collapsing via PID tree.
        // For each discovered PID, walk ppid chain up to 4 hops.
        // If an ancestor PID matches another discovered session → mark as child.
        let discovered_pids: HashSet<u32> = pid_to_idx.keys().copied().collect();
        for idx in 0..sessions.len() {
            let session_pid = sessions[idx].pid;
            let mut cur = session_pid;
            for _ in 0..4 {
                let Some(ppid) = parent_pid(cur) else {
                    break;
                };
                if ppid <= 1 || ppid == cur {
                    break;
                }
                if ppid != session_pid && discovered_pids.contains(&ppid) {
                    sessions[idx].parent_pid = Some(ppid);
                    break;
                }
                cur = ppid;
            }
        }

        // Check for API-spawned Codex sessions: JSONL files in
        // ~/.codex/sessions/ with mtime < 5 minutes and no matching
        // process-discovered session.
        // I30: Ghost filtering is applied inside discover_api_active_codex_sessions.
        if let Some(api_sessions) = discover_api_active_codex_sessions(&discovered_jsonl_paths) {
            sessions.extend(api_sessions);
        }

        // I28: Dedup by session ID — group entries by id, keep highest CPU.
        sessions = dedup_by_session_id(sessions);

        Ok(sessions)
    }
    #[cfg(not(unix))]
    {
        Ok(Vec::new())
    }
}

/// I28: Deduplicate sessions sharing the same session ID.
/// For each group, keep the entry with highest CPU%, merge all PIDs.
fn dedup_by_session_id(sessions: Vec<ActiveSession>) -> Vec<ActiveSession> {
    let mut by_id: HashMap<String, Vec<ActiveSession>> = HashMap::new();
    let mut no_id: Vec<ActiveSession> = Vec::new();

    for session in sessions {
        if let Some(ref id) = session.id {
            by_id.entry(id.clone()).or_default().push(session);
        } else {
            no_id.push(session);
        }
    }

    let mut result = Vec::new();
    for (_id, mut group) in by_id {
        if group.len() == 1 {
            result.push(group.remove(0));
            continue;
        }
        // Pick entry with highest CPU as representative.
        group.sort_by(|a, b| {
            b.process
                .cpu_pct
                .partial_cmp(&a.process.cpu_pct)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let mut best = group.remove(0);
        // Merge all PIDs from the group.
        let mut all_pids: Vec<u32> = best.all_pids.clone();
        for other in &group {
            for &pid in &other.all_pids {
                if !all_pids.contains(&pid) {
                    all_pids.push(pid);
                }
            }
            // Carry forward parent_pid if any entry had one.
            if best.parent_pid.is_none() && other.parent_pid.is_some() {
                best.parent_pid = other.parent_pid;
            }
            // Carry forward summary if the best didn't have one.
            if best.summary.is_none() && other.summary.is_some() {
                best.summary = other.summary.clone();
            }
            // Carry forward tmux session if the best didn't have one.
            if best.tmux_session.is_none() && other.tmux_session.is_some() {
                best.tmux_session = other.tmux_session.clone();
            }
        }
        best.all_pids = all_pids;
        result.push(best);
    }
    result.extend(no_id);
    result
}

/// I20: Extract a one-line summary for a session.
/// Priority: (1) handoff headline from DB, (2) first user prompt from JSONL, (3) CWD project name.
fn extract_summary(
    session_id: Option<&str>,
    jsonl_path: Option<&Path>,
    cwd: &str,
) -> Option<String> {
    // 1. Try handoff headline from DB.
    if let Some(id) = session_id {
        if let Some(headline) = get_handoff_headline(id) {
            return Some(headline);
        }
    }

    // 2. Try first user prompt from JSONL.
    if let Some(path) = jsonl_path {
        if let Some(prompt) = extract_first_user_prompt(path) {
            return Some(prompt);
        }
    }

    // 3. Fallback: CWD project name (last path component).
    let project = cwd
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(cwd);
    if !project.is_empty() && project != "." {
        return Some(project.to_string());
    }

    None
}

/// Query the handoffs table for a session's headline.
fn get_handoff_headline(session_id: &str) -> Option<String> {
    let conn = crate::db::open_db_readonly().ok()?;
    let headline: Option<String> = conn
        .query_row(
            "SELECT headline FROM handoffs WHERE session_id = ?1",
            [session_id],
            |row| row.get(0),
        )
        .ok()?;
    headline.filter(|h| !h.is_empty())
}

/// Read first ~30 lines of JSONL and extract the first user/human prompt text.
fn extract_first_user_prompt(path: &Path) -> Option<String> {
    let lines = super::discover::read_head_lines(path, 50);
    for line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let record: serde_json::Value = serde_json::from_str(trimmed).ok()?;

        // Claude format: type == "human" or "user", message.content is array or string
        let msg_type = record.get("type").and_then(serde_json::Value::as_str);
        if matches!(msg_type, Some("human") | Some("user")) {
            // Try message.content as array of blocks
            if let Some(blocks) = record
                .pointer("/message/content")
                .and_then(serde_json::Value::as_array)
            {
                for block in blocks {
                    if block.get("type").and_then(serde_json::Value::as_str) == Some("text") {
                        if let Some(text) = block.get("text").and_then(serde_json::Value::as_str) {
                            let truncated = truncate_summary(text, 60);
                            if !truncated.is_empty() {
                                return Some(truncated);
                            }
                        }
                    }
                }
            }
            // Try message.content as plain string
            if let Some(text) = record
                .pointer("/message/content")
                .and_then(serde_json::Value::as_str)
            {
                let truncated = truncate_summary(text, 60);
                if !truncated.is_empty() {
                    return Some(truncated);
                }
            }
        }

        // Codex format: type == "message" with role == "user"
        if msg_type == Some("message") {
            if record.get("role").and_then(serde_json::Value::as_str) == Some("user") {
                if let Some(text) = record.get("content").and_then(serde_json::Value::as_str) {
                    let truncated = truncate_summary(text, 60);
                    if !truncated.is_empty() {
                        return Some(truncated);
                    }
                }
            }
        }

        // Codex responses API format: payload with user input_text
        if let Some(text) = record
            .pointer("/payload/input_text")
            .and_then(serde_json::Value::as_str)
        {
            let truncated = truncate_summary(text, 60);
            if !truncated.is_empty() {
                return Some(truncated);
            }
        }
    }
    None
}

/// Truncate a string to max_chars, taking the first line and trimming whitespace.
fn truncate_summary(text: &str, max_chars: usize) -> String {
    let first_line = text.lines().next().unwrap_or(text).trim();
    if first_line.chars().count() <= max_chars {
        first_line.to_string()
    } else {
        let mut s: String = first_line.chars().take(max_chars - 3).collect();
        s.push_str("...");
        s
    }
}

/// Scan `~/.codex/sessions/` for JSONL files modified within the last 5
/// minutes that were not already discovered via process inspection.
/// These represent API-spawned Codex sessions (via agent-mux) that have
/// no live process on this machine.
///
/// I30: Ghost filtering — pid=0 sessions are excluded when BOTH the file
/// mtime AND the last JSONL event timestamp are older than 120s.  Checking
/// only mtime was insufficient because OS-level metadata writes can bump
/// mtime long after the session has died.
fn discover_api_active_codex_sessions(
    already_discovered: &HashSet<PathBuf>,
) -> Option<Vec<ActiveSession>> {
    let home = dirs::home_dir()?;
    let sessions_root = home.join(".codex").join("sessions");
    if !sessions_root.exists() {
        return None;
    }

    let five_minutes = std::time::Duration::from_secs(5 * 60);
    let ghost_threshold = std::time::Duration::from_secs(120);
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

            // I30: Ghost filtering — a pid=0 session is a ghost if its last
            // JSONL event timestamp exceeds the threshold.  File mtime is
            // unreliable (gaal reads, OS indexing, etc. can bump it), so we
            // use the actual event timestamp as the authoritative signal.
            // Fall back to mtime only when no event timestamp is parseable.
            if is_last_event_stale(&path, ghost_threshold) {
                continue;
            }

            // Parse head to get session metadata.
            let head = super::discover::read_head_lines(&path, 30);
            let (id, _model, cwd) = parse_codex_api_head(&head);

            let session_id = id.map(|raw| truncate_codex_id(&raw));
            let cwd_str = cwd.unwrap_or_else(|| "unknown".to_string());
            let summary = extract_summary(
                session_id.as_deref(),
                Some(path.as_path()),
                &cwd_str,
            );

            results.push(ActiveSession {
                id: session_id,
                engine: Engine::Codex,
                pid: 0,
                all_pids: vec![],
                parent_pid: None,
                cwd: cwd_str,
                jsonl_path: Some(path),
                process: ProcessInfo {
                    pid: 0,
                    cpu_pct: 0.0,
                    rss_mb: 0.0,
                },
                tmux_session: None,
                summary,
            });
        }
    }

    if results.is_empty() {
        None
    } else {
        Some(results)
    }
}

/// Check if the last event in a JSONL file is older than the given threshold.
/// Reads the tail of the file to find the most recent timestamp.
/// Returns true (stale) when no timestamp is found or it exceeds the threshold.
fn is_last_event_stale(path: &Path, threshold: std::time::Duration) -> bool {
    use std::io::{BufRead, BufReader, Seek, SeekFrom};

    let Ok(mut file) = fs::File::open(path) else {
        return true;
    };
    let Ok(file_len) = file.metadata().map(|m| m.len()) else {
        return true;
    };
    if file_len == 0 {
        return true;
    }

    // Read the last ~32KB to find recent timestamps.
    let read_from = file_len.saturating_sub(32 * 1024);
    if file.seek(SeekFrom::Start(read_from)).is_err() {
        return true;
    }
    let reader = BufReader::new(file);
    let mut latest_ts: Option<chrono::DateTime<chrono::Utc>> = None;

    for line in reader.lines().map_while(Result::ok) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(record) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            continue;
        };
        // Try common timestamp fields.
        let ts_str = record
            .get("timestamp")
            .and_then(serde_json::Value::as_str)
            .or_else(|| record.pointer("/payload/timestamp").and_then(serde_json::Value::as_str));
        if let Some(ts_str) = ts_str {
            if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(ts_str) {
                let dt_utc = dt.with_timezone(&chrono::Utc);
                if latest_ts.map_or(true, |prev| dt_utc > prev) {
                    latest_ts = Some(dt_utc);
                }
            }
        }
    }

    match latest_ts {
        Some(ts) => {
            let age = chrono::Utc::now().signed_duration_since(ts);
            age.num_seconds() > threshold.as_secs() as i64
        }
        // No timestamp found — treat as stale.
        None => true,
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

#[cfg(target_os = "macos")]
fn list_agent_processes() -> Vec<(u32, Engine)> {
    // Use proc_pidpath for reliable process detection on macOS
    use std::os::raw::c_int;

    extern "C" {
        fn proc_listallpids(buffer: *mut c_int, buffersize: c_int) -> c_int;
        fn proc_pidpath(pid: c_int, buffer: *mut std::os::raw::c_char, buffersize: u32) -> c_int;
    }

    let mut found = Vec::new();
    let excluded = excluded_pids();

    // Get all PIDs
    let mut pid_buffer = vec![0i32; 4096];
    let pid_count = unsafe {
        proc_listallpids(pid_buffer.as_mut_ptr(), (pid_buffer.len() * std::mem::size_of::<c_int>()) as c_int)
    };

    if pid_count <= 0 {
        // Fall back to pgrep on error
        return list_agent_processes_fallback();
    }

    // proc_listallpids returns the number of PIDs (not bytes).
    pid_buffer.truncate(pid_count as usize);

    for &pid in &pid_buffer {
        if pid <= 0 || excluded.contains(&(pid as u32)) {
            continue;
        }

        // Get binary path
        let mut path_buffer = vec![0u8; 4096];
        let path_len = unsafe {
            proc_pidpath(pid, path_buffer.as_mut_ptr() as *mut std::os::raw::c_char, path_buffer.len() as u32)
        };

        if path_len <= 0 {
            continue;
        }

        path_buffer.truncate(path_len as usize);
        let Ok(binary_path) = String::from_utf8(path_buffer) else {
            continue;
        };

        if let Some(engine) = classify_engine_by_path(&binary_path) {
            found.push((pid as u32, engine));
        }
    }

    found
}

#[cfg(target_os = "linux")]
fn list_agent_processes() -> Vec<(u32, Engine)> {
    let mut found = Vec::new();
    let excluded = excluded_pids();

    let Ok(proc_dir) = fs::read_dir("/proc") else {
        return list_agent_processes_fallback();
    };

    for entry in proc_dir.flatten() {
        let file_name = entry.file_name();
        let Some(pid_str) = file_name.to_str() else { continue };
        let Ok(pid) = pid_str.parse::<u32>() else { continue };

        if excluded.contains(&pid) {
            continue;
        }

        // Get binary path via /proc/{pid}/exe
        let exe_path = format!("/proc/{}/exe", pid);
        let Ok(binary_path) = fs::read_link(&exe_path) else { continue };
        let Some(binary_path_str) = binary_path.to_str() else { continue };

        if let Some(engine) = classify_engine_by_path(binary_path_str) {
            found.push((pid, engine));
        }
    }

    found
}

#[cfg(not(unix))]
fn list_agent_processes() -> Vec<(u32, Engine)> {
    Vec::new()
}

/// Classify engine type based on binary path
fn classify_engine_by_path(path: &str) -> Option<Engine> {
    // Claude Code detection: path contains .local/share/claude/versions/
    if path.contains(".local/share/claude/versions/") {
        return Some(Engine::Claude);
    }

    // Codex detection: path contains "codex" but NOT ".app/"
    if path.contains("codex") && !path.contains(".app/") {
        return Some(Engine::Codex);
    }

    None
}

/// Fallback to pgrep-based detection when proc_pidpath fails
#[cfg(unix)]
fn list_agent_processes_fallback() -> Vec<(u32, Engine)> {
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
    use std::os::raw::{c_int, c_char};

    extern "C" {
        fn proc_pidinfo(pid: c_int, flavor: c_int, arg: u64, buffer: *mut c_char, buffersize: c_int) -> c_int;
    }

    // PROC_PIDVNODEPATHINFO = 9 (from sys/proc_info.h)
    const PROC_PIDVNODEPATHINFO: c_int = 9;
    const MAXPATHLEN: usize = 1024;

    // struct vnode_info = 152 bytes, struct vnode_info_path = vnode_info + char[MAXPATHLEN]
    // struct proc_vnodepathinfo = 2 * vnode_info_path = 2 * (152 + 1024) = 2352 bytes
    const VNODE_INFO_SIZE: usize = 152;
    const VNODE_INFO_PATH_SIZE: usize = VNODE_INFO_SIZE + MAXPATHLEN;
    const PROC_VNODEPATHINFO_SIZE: usize = 2 * VNODE_INFO_PATH_SIZE;

    let mut buffer = vec![0u8; PROC_VNODEPATHINFO_SIZE];
    let ret = unsafe {
        proc_pidinfo(
            pid as c_int,
            PROC_PIDVNODEPATHINFO,
            0,
            buffer.as_mut_ptr() as *mut c_char,
            buffer.len() as c_int,
        )
    };

    if ret <= 0 {
        // Fall back to lsof if proc_pidinfo fails
        return resolve_cwd_fallback(pid);
    }

    // pvi_cdir.vip_path starts at offset VNODE_INFO_SIZE (after vnode_info struct)
    let path_start = VNODE_INFO_SIZE;
    let path_end = path_start + MAXPATHLEN;
    if ret as usize >= path_end {
        let path_bytes = &buffer[path_start..path_end];
        let null_pos = path_bytes.iter().position(|&b| b == 0).unwrap_or(MAXPATHLEN);
        let path_slice = &path_bytes[..null_pos];
        return String::from_utf8(path_slice.to_vec()).ok();
    }

    // Insufficient data returned — fall back to lsof
    resolve_cwd_fallback(pid)
}

#[cfg(target_os = "macos")]
fn resolve_cwd_fallback(pid: u32) -> Option<String> {
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

fn map_cwd_to_jsonl(engine: &Engine, cwd: &str, pid: u32) -> Option<PathBuf> {
    match engine {
        Engine::Claude => map_claude_cwd_to_jsonl(cwd, pid),
        Engine::Codex => map_codex_cwd_to_jsonl(cwd),
    }
}

/// I37: Get the process start time as SystemTime, used to match each PID to its JSONL.
/// On macOS, parses `ps -p PID -o lstart=` output.
#[cfg(unix)]
fn get_pid_start_time(pid: u32) -> Option<std::time::SystemTime> {
    let output = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "lstart="])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let raw = String::from_utf8_lossy(&output.stdout);
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Parse "Tue Mar 10 15:10:02 2026" — %e handles single/double-digit day with padding.
    let naive = chrono::NaiveDateTime::parse_from_str(trimmed, "%a %b %e %H:%M:%S %Y").ok()?;
    let dt = naive.and_local_timezone(chrono::Local).single()?;
    let unix_secs = dt.timestamp();
    if unix_secs <= 0 {
        return None;
    }
    Some(std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(unix_secs as u64))
}

#[cfg(not(unix))]
fn get_pid_start_time(_pid: u32) -> Option<std::time::SystemTime> {
    None
}

fn map_claude_cwd_to_jsonl(cwd: &str, pid: u32) -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let projects_root = home.join(".claude").join("projects");

    // I37: Prefer the JSONL file whose mtime matches the process start time,
    // rather than always picking the newest. This prevents multiple Claude
    // processes in the same CWD from all resolving to the same session.
    let pid_start = if pid > 0 { get_pid_start_time(pid) } else { None };

    let encoded = encode_claude_project_dir(cwd);
    let dir = projects_root.join(&encoded);
    if dir.is_dir() {
        if let Some(path) = jsonl_matching_pid_start(&dir, pid_start) {
            return Some(path);
        }
    }

    if let Ok(real) = fs::canonicalize(cwd) {
        if let Some(real_cwd) = real.to_str() {
            let encoded_real = encode_claude_project_dir(real_cwd);
            let real_dir = projects_root.join(encoded_real);
            if real_dir != dir && real_dir.is_dir() {
                if let Some(path) = jsonl_matching_pid_start(&real_dir, pid_start) {
                    return Some(path);
                }
            }
        }
    }

    let Ok(discovered) = super::claude::discover_claude_sessions() else {
        return None;
    };
    let mut candidates: Vec<_> = discovered
        .into_iter()
        .filter(|s| same_cwd(s.cwd.as_deref(), cwd))
        .collect();

    if let Some(start) = pid_start {
        let start_unix = start
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .ok()
            .map(|d| d.as_secs())
            .unwrap_or(0);
        // Sort ascending and pick the newest that's within the start window.
        candidates.sort_by(|a, b| a.started_at.cmp(&b.started_at));
        let best = candidates.iter().rev().find(|s| {
            let mtime = fs::metadata(&s.path)
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::SystemTime::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(u64::MAX);
            mtime <= start_unix + 120
        });
        if let Some(s) = best {
            return Some(s.path.clone());
        }
    }

    candidates
        .into_iter()
        .max_by(|a, b| a.started_at.cmp(&b.started_at))
        .map(|s| s.path)
}

/// I37: Find the JSONL in `dir` whose mtime falls within 120 seconds after `pid_start`.
/// Returns the most-recent such file. Falls back to the newest file overall.
fn jsonl_matching_pid_start(dir: &Path, pid_start: Option<std::time::SystemTime>) -> Option<PathBuf> {
    let entries = fs::read_dir(dir).ok()?;
    let mut candidates: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();

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
        candidates.push((modified, path));
    }

    if candidates.is_empty() {
        return None;
    }

    if let Some(start) = pid_start {
        let start_unix = start
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .ok()
            .map(|d| d.as_secs())
            .unwrap_or(0);
        // Sort ascending, then find the most-recent file within the window.
        candidates.sort_by(|a, b| a.0.cmp(&b.0));
        let best = candidates.iter().rev().find(|(mtime, _)| {
            let file_unix = mtime
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .ok()
                .map(|d| d.as_secs())
                .unwrap_or(u64::MAX);
            file_unix <= start_unix + 120
        });
        if let Some((_, path)) = best {
            return Some(path.clone());
        }
    }

    // Fallback: newest file.
    candidates.into_iter().max_by(|a, b| a.0.cmp(&b.0)).map(|(_, p)| p)
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
