use std::collections::{HashMap, HashSet};
use std::fs;
use std::mem;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::parser::types::Engine;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

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
    /// Process start time (seconds since epoch, from proc_pidinfo).
    /// 0 for API-spawned sessions with no live process.
    pub start_tvsec: u64,
}

// ---------------------------------------------------------------------------
// macOS FFI declarations (one block, no duplication)
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
mod ffi {
    use std::os::raw::{c_char, c_int, c_void};

    extern "C" {
        pub fn proc_listallpids(buffer: *mut c_int, buffersize: c_int) -> c_int;
        pub fn proc_pidpath(pid: c_int, buffer: *mut c_char, buffersize: u32) -> c_int;
        pub fn proc_pidinfo(
            pid: c_int,
            flavor: c_int,
            arg: u64,
            buffer: *mut c_char,
            buffersize: c_int,
        ) -> c_int;
        pub fn sysctl(
            name: *mut c_int,
            namelen: u32,
            oldp: *mut c_void,
            oldlenp: *mut usize,
            newp: *mut c_void,
            newlen: usize,
        ) -> c_int;
    }

    /// CTL_KERN = 1
    pub const CTL_KERN: c_int = 1;
    /// KERN_PROCARGS2 = 49
    pub const KERN_PROCARGS2: c_int = 49;

    /// PROC_PIDTBSDINFO = 3 — returns proc_bsdinfo (ppid, start time, comm).
    pub const PROC_PIDTBSDINFO: c_int = 3;
    /// PROC_PIDTASKINFO = 4 — returns proc_taskinfo (RSS, CPU time).
    pub const PROC_PIDTASKINFO: c_int = 4;
    /// PROC_PIDVNODEPATHINFO = 9 — returns CWD path.
    pub const PROC_PIDVNODEPATHINFO: c_int = 9;
    pub const MAXPATHLEN: usize = 1024;
    /// sizeof(vnode_info) = 152; vnode_info_path = 152 + 1024; vnodepathinfo = 2 * vip.
    pub const VNODE_INFO_SIZE: usize = 152;
    pub const PROC_VNODEPATHINFO_SIZE: usize = 2 * (VNODE_INFO_SIZE + MAXPATHLEN);

    /// Minimal proc_bsdinfo — only the fields we read.
    #[repr(C)]
    #[derive(Debug, Copy, Clone)]
    pub struct ProcBsdInfo {
        pub pbi_flags: u32,
        pub pbi_status: u32,
        pub pbi_xstatus: u32,
        pub pbi_pid: u32,
        pub pbi_ppid: u32,
        pub pbi_uid: u32,
        pub pbi_gid: u32,
        pub pbi_ruid: u32,
        pub pbi_rgid: u32,
        pub pbi_svuid: u32,
        pub pbi_svgid: u32,
        pub rfu_1: u32,
        pub pbi_comm: [u8; 16],
        pub pbi_name: [u8; 32],
        pub pbi_nfiles: u32,
        pub pbi_pgid: u32,
        pub pbi_pjobc: u32,
        pub e_tdev: u32,
        pub e_tpgid: u32,
        pub pbi_nice: i32,
        pub pbi_start_tvsec: u64,
        pub pbi_start_tvusec: u64,
    }

    /// proc_taskinfo for RSS.
    #[repr(C)]
    pub struct ProcTaskInfo {
        pub pti_virtual_size: u64,
        pub pti_resident_size: u64,
        pub pti_total_user: u64,
        pub pti_total_system: u64,
        pub pti_threads_user: u64,
        pub pti_threads_system: u64,
        pub pti_policy: i32,
        pub pti_faults: i32,
        pub pti_pageins: i32,
        pub pti_cow_faults: i32,
        pub pti_messages_sent: i32,
        pub pti_messages_received: i32,
        pub pti_syscalls_mach: i32,
        pub pti_syscalls_unix: i32,
        pub pti_csw: i32,
        pub pti_threadnum: i32,
        pub pti_numrunning: i32,
        pub pti_priority: i32,
    }
}

#[cfg(unix)]
mod libc {
    unsafe extern "C" {
        pub fn kill(pid: i32, sig: i32) -> i32;
    }
}

// ---------------------------------------------------------------------------
// Process tree node
// ---------------------------------------------------------------------------

/// A node in the process tree, built from one pass over the process table.
#[derive(Debug)]
#[allow(dead_code)]
struct ProcessNode {
    pid: u32,
    ppid: u32,
    /// Binary path (only resolved for agent-matched PIDs).
    binary_path: Option<String>,
    /// Engine type (only set for agent-matched PIDs).
    engine: Option<Engine>,
    /// CWD (only resolved for agent-matched PIDs).
    cwd: Option<String>,
    /// Process start time (seconds since epoch).
    start_tvsec: u64,
    /// Direct children PIDs.
    children: Vec<u32>,
}

// ---------------------------------------------------------------------------
// Primary entry point
// ---------------------------------------------------------------------------

/// Discover currently running Claude/Codex sessions by building a process
/// tree from the macOS process table. One pass to enumerate PIDs + ppids,
/// one pass to classify agent binaries, tree walks for context.
pub fn find_active_sessions() -> Result<Vec<ActiveSession>> {
    #[cfg(target_os = "macos")]
    {
        find_active_sessions_macos()
    }
    #[cfg(target_os = "linux")]
    {
        find_active_sessions_linux()
    }
    #[cfg(not(unix))]
    {
        Ok(Vec::new())
    }
}

// ---------------------------------------------------------------------------
// macOS implementation
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
fn find_active_sessions_macos() -> Result<Vec<ActiveSession>> {
    use std::os::raw::c_int;

    let self_pid = std::process::id();

    // --- Phase 1: Get all PIDs ---
    let mut pid_buf = vec![0i32; 8192];
    let pid_count = unsafe {
        ffi::proc_listallpids(
            pid_buf.as_mut_ptr(),
            (pid_buf.len() * mem::size_of::<c_int>()) as c_int,
        )
    };
    if pid_count <= 0 {
        return Ok(Vec::new());
    }
    pid_buf.truncate(pid_count as usize);

    // --- Phase 2: For each PID, get ppid via PROC_PIDTBSDINFO ---
    // Also get binary path via proc_pidpath. Build the tree.
    let mut tree: HashMap<u32, ProcessNode> = HashMap::with_capacity(pid_count as usize);

    for &raw_pid in &pid_buf {
        if raw_pid <= 0 {
            continue;
        }
        let pid = raw_pid as u32;
        if pid == self_pid {
            continue;
        }

        // Get bsd info (ppid, start time)
        let bsd = match get_bsd_info(pid) {
            Some(info) => info,
            None => continue,
        };

        // Get binary path
        let binary_path = get_binary_path(pid);

        // Classify engine from binary path (+ argv for interpreter processes)
        let engine = classify_engine(pid, binary_path.as_deref());

        tree.insert(
            pid,
            ProcessNode {
                pid,
                ppid: bsd.pbi_ppid,
                binary_path,
                engine,
                cwd: None, // deferred — only resolved for agent PIDs
                start_tvsec: bsd.pbi_start_tvsec,
                children: Vec::new(),
            },
        );
    }

    // --- Phase 3: Wire up children ---
    let pids: Vec<u32> = tree.keys().copied().collect();
    for &pid in &pids {
        let ppid = tree[&pid].ppid;
        if ppid > 1 && tree.contains_key(&ppid) {
            // SAFETY: ppid != pid (kernel guarantees), so these are distinct entries.
            let parent = tree.get_mut(&ppid).unwrap();
            parent.children.push(pid);
        }
    }

    // --- Phase 4: Identify agent PIDs and resolve CWD + metrics ---
    let agent_pids: Vec<u32> = tree
        .iter()
        .filter(|(_, node)| node.engine.is_some())
        .map(|(&pid, _)| pid)
        .collect();

    // Resolve CWD for all agent PIDs (native, no subprocess)
    for &pid in &agent_pids {
        let cwd = resolve_cwd_native(pid);
        if let Some(node) = tree.get_mut(&pid) {
            node.cwd = cwd;
        }
    }

    // Collect tmux pane mapping once (single subprocess call)
    let tmux_map = collect_tmux_pane_map();

    // --- Phase 5: Build ActiveSession for each agent PID ---
    let agent_pid_set: HashSet<u32> = agent_pids.iter().copied().collect();
    let mut sessions: Vec<ActiveSession> = Vec::new();

    // Batch lsof call for all agent PIDs to resolve session IDs from tasks dirs.
    let tasks_dir_ids = resolve_session_ids_from_tasks_dir_batch(&agent_pids);

    for &pid in &agent_pids {
        let node = &tree[&pid];
        let engine = node.engine.unwrap();
        let cwd = node.cwd.clone().unwrap_or_else(|| "unknown".to_string());

        // Walk UP to find parent agent (for child collapsing)
        let parent_pid = walk_up_to_agent(pid, &tree, &agent_pid_set);

        // Each PID is exactly one row — no collapsing.
        let all_pids = vec![pid];

        // Resolve JSONL path from CWD
        let jsonl_path = map_cwd_to_jsonl(&engine, &cwd, pid);

        // Resolve session ID: lsof tasks-dir takes priority (structural),
        // fall back to JSONL head scan (CWD-based heuristic).
        let id = tasks_dir_ids.get(&pid).cloned().or_else(|| {
            jsonl_path
                .as_ref()
                .and_then(|path| extract_session_id_from_jsonl(path, &engine))
        });

        // Find tmux session by walking up the tree
        let tmux_session = find_tmux_session_from_tree(pid, &tree, &tmux_map);

        // Probe process metrics
        let process = probe_pid(pid).unwrap_or(ProcessInfo {
            pid,
            cpu_pct: 0.0,
            rss_mb: 0.0,
        });

        // Extract summary
        let summary = extract_summary(id.as_deref(), jsonl_path.as_deref(), &cwd);

        sessions.push(ActiveSession {
            id,
            engine,
            pid,
            all_pids,
            parent_pid,
            cwd,
            jsonl_path,
            process,
            tmux_session,
            summary,
            start_tvsec: node.start_tvsec,
        });
    }

    // --- Phase 6: Check for API-spawned Codex sessions (no live process) ---
    let discovered_jsonl_paths: HashSet<PathBuf> = sessions
        .iter()
        .filter_map(|s| s.jsonl_path.clone())
        .collect();
    if let Some(api_sessions) = discover_api_active_codex_sessions(&discovered_jsonl_paths) {
        sessions.extend(api_sessions);
    }

    Ok(sessions)
}

// ---------------------------------------------------------------------------
// macOS native FFI helpers
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
fn get_bsd_info(pid: u32) -> Option<ffi::ProcBsdInfo> {
    use std::os::raw::{c_char, c_int};

    let mut info: ffi::ProcBsdInfo = unsafe { mem::zeroed() };
    let ret = unsafe {
        ffi::proc_pidinfo(
            pid as c_int,
            ffi::PROC_PIDTBSDINFO,
            0,
            &mut info as *mut _ as *mut c_char,
            mem::size_of::<ffi::ProcBsdInfo>() as c_int,
        )
    };
    if ret <= 0 {
        return None;
    }
    Some(info)
}

#[cfg(target_os = "macos")]
fn get_binary_path(pid: u32) -> Option<String> {
    use std::os::raw::c_int;

    let mut buf = vec![0u8; 4096];
    let len = unsafe {
        ffi::proc_pidpath(
            pid as c_int,
            buf.as_mut_ptr() as *mut std::os::raw::c_char,
            buf.len() as u32,
        )
    };
    if len <= 0 {
        return None;
    }
    buf.truncate(len as usize);
    String::from_utf8(buf).ok()
}

/// Resolve CWD via proc_pidinfo PROC_PIDVNODEPATHINFO — zero subprocess calls.
#[cfg(target_os = "macos")]
fn resolve_cwd_native(pid: u32) -> Option<String> {
    use std::os::raw::{c_char, c_int};

    let mut buffer = vec![0u8; ffi::PROC_VNODEPATHINFO_SIZE];
    let ret = unsafe {
        ffi::proc_pidinfo(
            pid as c_int,
            ffi::PROC_PIDVNODEPATHINFO,
            0,
            buffer.as_mut_ptr() as *mut c_char,
            buffer.len() as c_int,
        )
    };
    if ret <= 0 {
        return None;
    }

    // pvi_cdir.vip_path starts at offset VNODE_INFO_SIZE (after vnode_info struct)
    let path_start = ffi::VNODE_INFO_SIZE;
    let path_end = path_start + ffi::MAXPATHLEN;
    if (ret as usize) < path_end {
        return None;
    }

    let path_bytes = &buffer[path_start..path_end];
    let null_pos = path_bytes
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(ffi::MAXPATHLEN);
    String::from_utf8(path_bytes[..null_pos].to_vec()).ok()
}

// ---------------------------------------------------------------------------
// Linux implementation (kept for cross-platform build)
// ---------------------------------------------------------------------------

#[cfg(target_os = "linux")]
fn find_active_sessions_linux() -> Result<Vec<ActiveSession>> {
    let self_pid = std::process::id();
    let mut tree: HashMap<u32, ProcessNode> = HashMap::new();

    let Ok(proc_dir) = fs::read_dir("/proc") else {
        return Ok(Vec::new());
    };

    for entry in proc_dir.flatten() {
        let file_name = entry.file_name();
        let Some(pid_str) = file_name.to_str() else {
            continue;
        };
        let Ok(pid) = pid_str.parse::<u32>() else {
            continue;
        };
        if pid == self_pid {
            continue;
        }

        // Get ppid from /proc/pid/stat
        let ppid = linux_ppid(pid).unwrap_or(0);

        // Get binary path
        let binary_path = fs::read_link(format!("/proc/{}/exe", pid))
            .ok()
            .and_then(|p| p.to_str().map(String::from));

        let engine = classify_engine(pid, binary_path.as_deref());

        tree.insert(
            pid,
            ProcessNode {
                pid,
                ppid,
                binary_path,
                engine,
                cwd: None,
                start_tvsec: 0,
                children: Vec::new(),
            },
        );
    }

    // Wire up children
    let pids: Vec<u32> = tree.keys().copied().collect();
    for &pid in &pids {
        let ppid = tree[&pid].ppid;
        if ppid > 1 && tree.contains_key(&ppid) {
            let parent = tree.get_mut(&ppid).unwrap();
            parent.children.push(pid);
        }
    }

    // Identify agent PIDs
    let agent_pids: Vec<u32> = tree
        .iter()
        .filter(|(_, node)| node.engine.is_some())
        .map(|(&pid, _)| pid)
        .collect();

    // Resolve CWD for agent PIDs
    for &pid in &agent_pids {
        let cwd = fs::read_link(format!("/proc/{}/cwd", pid))
            .ok()
            .and_then(|p| p.to_str().map(String::from));
        if let Some(node) = tree.get_mut(&pid) {
            node.cwd = cwd;
        }
    }

    let tmux_map = collect_tmux_pane_map();
    let agent_pid_set: HashSet<u32> = agent_pids.iter().copied().collect();
    let mut sessions: Vec<ActiveSession> = Vec::new();

    // Batch lsof call for all agent PIDs to resolve session IDs from tasks dirs.
    let tasks_dir_ids = resolve_session_ids_from_tasks_dir_batch(&agent_pids);

    for &pid in &agent_pids {
        let node = &tree[&pid];
        let engine = node.engine.unwrap();
        let cwd = node.cwd.clone().unwrap_or_else(|| "unknown".to_string());
        let parent_pid = walk_up_to_agent(pid, &tree, &agent_pid_set);
        // Each PID is exactly one row — no collapsing.
        let all_pids = vec![pid];
        let jsonl_path = map_cwd_to_jsonl(&engine, &cwd, pid);
        let id = tasks_dir_ids.get(&pid).cloned().or_else(|| {
            jsonl_path
                .as_ref()
                .and_then(|path| extract_session_id_from_jsonl(path, &engine))
        });
        let tmux_session = find_tmux_session_from_tree(pid, &tree, &tmux_map);
        let process = probe_pid(pid).unwrap_or(ProcessInfo {
            pid,
            cpu_pct: 0.0,
            rss_mb: 0.0,
        });
        let summary = extract_summary(id.as_deref(), jsonl_path.as_deref(), &cwd);

        sessions.push(ActiveSession {
            id,
            engine,
            pid,
            all_pids,
            parent_pid,
            cwd,
            jsonl_path,
            process,
            tmux_session,
            summary,
            start_tvsec: node.start_tvsec,
        });
    }

    let discovered_jsonl_paths: HashSet<PathBuf> = sessions
        .iter()
        .filter_map(|s| s.jsonl_path.clone())
        .collect();
    if let Some(api_sessions) = discover_api_active_codex_sessions(&discovered_jsonl_paths) {
        sessions.extend(api_sessions);
    }
    Ok(sessions)
}

#[cfg(target_os = "linux")]
fn linux_ppid(pid: u32) -> Option<u32> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after_comm = stat.rsplit_once(") ")?.1;
    let mut fields = after_comm.split_whitespace();
    let _state = fields.next()?;
    fields.next()?.parse::<u32>().ok()
}

// ---------------------------------------------------------------------------
// Tree walk helpers
// ---------------------------------------------------------------------------

/// Walk UP the tree from `pid` to find the nearest ancestor that is also
/// an agent process. Returns None if no ancestor is an agent.
fn walk_up_to_agent(
    pid: u32,
    tree: &HashMap<u32, ProcessNode>,
    agent_pids: &HashSet<u32>,
) -> Option<u32> {
    let mut cur = pid;
    for _ in 0..64 {
        let Some(node) = tree.get(&cur) else {
            break;
        };
        let ppid = node.ppid;
        if ppid <= 1 || ppid == cur {
            break;
        }
        if ppid != pid && agent_pids.contains(&ppid) {
            return Some(ppid);
        }
        cur = ppid;
    }
    None
}

/// Recursively collect descendant PIDs that are also agents.
/// Kept for potential future use (tree visualization, parent-child display).
#[allow(dead_code)]
fn collect_descendant_agents(
    pid: u32,
    tree: &HashMap<u32, ProcessNode>,
    agent_pids: &HashSet<u32>,
    out: &mut Vec<u32>,
) {
    let Some(node) = tree.get(&pid) else {
        return;
    };
    for &child in &node.children {
        if agent_pids.contains(&child) && !out.contains(&child) {
            out.push(child);
        }
        collect_descendant_agents(child, tree, agent_pids, out);
    }
}

// ---------------------------------------------------------------------------
// Engine classification
// ---------------------------------------------------------------------------

/// Classify engine type from binary path + argv inspection.
///
/// Fast path: check binary path for known Claude/Codex patterns.
/// Slow path: for interpreter processes (node/bun), read argv via sysctl
/// KERN_PROCARGS2 and look for Claude Agent SDK markers.
fn classify_engine(pid: u32, binary_path: Option<&str>) -> Option<Engine> {
    // Step 1: Existing binary path check (fast path)
    if let Some(path) = binary_path {
        if path.contains(".local/share/claude/versions/") {
            return Some(Engine::Claude);
        }
        if path.contains("codex") && !path.contains(".app/") {
            return Some(Engine::Codex);
        }
    }

    // Step 2: For interpreter processes (node/bun), check argv for SDK markers
    if let Some(path) = binary_path {
        if path.contains("/bun") || path.contains("/node") {
            if let Some(argv) = get_process_argv(pid) {
                if argv.iter().any(|a| a.contains("claude-agent-sdk")) {
                    return Some(Engine::Claude);
                }
            }
        }
    }

    None
}

// ---------------------------------------------------------------------------
// Process argv reading (macOS)
// ---------------------------------------------------------------------------

/// Read process argv via sysctl KERN_PROCARGS2.
///
/// On macOS, KERN_PROCARGS2 returns: argc (i32) + exec_path (null-terminated)
/// + padding nulls + argv strings (null-terminated each).
/// This is a standard macOS API — no special permissions needed for own-user
/// processes.
#[cfg(target_os = "macos")]
fn get_process_argv(pid: u32) -> Option<Vec<String>> {
    use std::os::raw::c_void;

    let mut mib: [i32; 3] = [ffi::CTL_KERN, ffi::KERN_PROCARGS2, pid as i32];

    // First call: get required buffer size
    let mut size: usize = 0;
    let rc = unsafe {
        ffi::sysctl(
            mib.as_mut_ptr(),
            3,
            std::ptr::null_mut(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc != 0 || size == 0 {
        return None;
    }

    // Second call: read data
    let mut buf = vec![0u8; size];
    let rc = unsafe {
        ffi::sysctl(
            mib.as_mut_ptr(),
            3,
            buf.as_mut_ptr() as *mut c_void,
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc != 0 {
        return None;
    }
    buf.truncate(size);

    // Parse: first 4 bytes = argc (i32)
    if size < 4 {
        return None;
    }
    let argc = i32::from_ne_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;

    // Skip past argc field + executable path + padding nulls
    let mut pos = 4;
    // Skip executable path (null-terminated)
    while pos < size && buf[pos] != 0 {
        pos += 1;
    }
    // Skip null terminators / alignment padding
    while pos < size && buf[pos] == 0 {
        pos += 1;
    }

    // Read argv strings (each null-terminated)
    let mut argv = Vec::with_capacity(argc);
    for _ in 0..argc {
        if pos >= size {
            break;
        }
        let start = pos;
        while pos < size && buf[pos] != 0 {
            pos += 1;
        }
        if let Ok(s) = std::str::from_utf8(&buf[start..pos]) {
            argv.push(s.to_string());
        }
        pos += 1; // skip null terminator
    }

    Some(argv)
}

/// Stub for non-macOS — argv reading not implemented.
#[cfg(not(target_os = "macos"))]
fn get_process_argv(_pid: u32) -> Option<Vec<String>> {
    None
}

// ---------------------------------------------------------------------------
// Process metrics
// ---------------------------------------------------------------------------

/// Probe process metrics for a PID.
pub fn probe_pid(pid: u32) -> Option<ProcessInfo> {
    #[cfg(target_os = "macos")]
    {
        probe_pid_macos(pid)
    }
    #[cfg(target_os = "linux")]
    {
        probe_pid_linux(pid)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        None
    }
}

#[cfg(target_os = "macos")]
fn probe_pid_macos(pid: u32) -> Option<ProcessInfo> {
    use std::os::raw::{c_char, c_int};

    // Get RSS from proc_taskinfo
    let mut task_info: ffi::ProcTaskInfo = unsafe { mem::zeroed() };
    let ret = unsafe {
        ffi::proc_pidinfo(
            pid as c_int,
            ffi::PROC_PIDTASKINFO,
            0,
            &mut task_info as *mut _ as *mut c_char,
            mem::size_of::<ffi::ProcTaskInfo>() as c_int,
        )
    };

    let rss_mb = if ret > 0 {
        task_info.pti_resident_size as f64 / (1024.0 * 1024.0)
    } else {
        0.0
    };

    // CPU: still need ps for accurate percentage (proc_pidinfo gives cumulative ticks)
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
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<f64>()
        .ok()
}

#[cfg(target_os = "linux")]
fn probe_pid_linux(pid: u32) -> Option<ProcessInfo> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let parsed_pid = stat.split_whitespace().next()?.parse::<u32>().ok()?;

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

/// Check whether a PID exists (`kill(pid, 0)`).
pub fn is_pid_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    #[cfg(unix)]
    {
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

// ---------------------------------------------------------------------------
// Tmux integration
// ---------------------------------------------------------------------------

/// Collect all tmux pane_pid → session_name mappings in a single call.
fn collect_tmux_pane_map() -> HashMap<u32, String> {
    let mut map = HashMap::new();
    let output = match Command::new("tmux")
        .args(["list-panes", "-a", "-F", "#{session_name}\t#{pane_pid}"])
        .output()
    {
        Ok(output) if output.status.success() => output,
        _ => return map,
    };

    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let mut parts = line.split('\t');
        let Some(session) = parts.next() else {
            continue;
        };
        let Some(pid_str) = parts.next() else {
            continue;
        };
        let Ok(pane_pid) = pid_str.trim().parse::<u32>() else {
            continue;
        };
        map.insert(pane_pid, session.to_string());
    }
    map
}

/// Find tmux session for a PID by walking UP the process tree, using
/// the pre-collected tmux pane map. No per-PID subprocess calls.
pub fn find_tmux_session(pid: u32) -> Option<String> {
    let tmux_map = collect_tmux_pane_map();
    find_tmux_session_with_map(pid, &tmux_map)
}

/// Find tmux session by walking up ppid chain using `ps` (fallback).
fn find_tmux_session_with_map(pid: u32, tmux_map: &HashMap<u32, String>) -> Option<String> {
    if let Some(session) = tmux_map.get(&pid) {
        return Some(session.clone());
    }
    // Walk up via ps for the non-tree path
    let mut cur = pid;
    for _ in 0..64 {
        let parent = parent_pid_via_ps(cur)?;
        if let Some(session) = tmux_map.get(&parent) {
            return Some(session.clone());
        }
        if parent <= 1 || parent == cur {
            break;
        }
        cur = parent;
    }
    None
}

/// Find tmux session by walking up the process tree (no subprocess calls).
fn find_tmux_session_from_tree(
    pid: u32,
    tree: &HashMap<u32, ProcessNode>,
    tmux_map: &HashMap<u32, String>,
) -> Option<String> {
    if let Some(session) = tmux_map.get(&pid) {
        return Some(session.clone());
    }
    let mut cur = pid;
    for _ in 0..64 {
        let Some(node) = tree.get(&cur) else {
            break;
        };
        let ppid = node.ppid;
        if let Some(session) = tmux_map.get(&ppid) {
            return Some(session.clone());
        }
        if ppid <= 1 || ppid == cur {
            break;
        }
        cur = ppid;
    }
    None
}

fn parent_pid_via_ps(pid: u32) -> Option<u32> {
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

// ---------------------------------------------------------------------------
// Session ID resolution via lsof (tasks directory)
// ---------------------------------------------------------------------------

/// Attempt to resolve the Claude session ID for a live PID by inspecting
/// its open file descriptors for a `.claude/tasks/<uuid>/` path.
/// Falls back gracefully — returns None if lsof fails or finds nothing.
pub fn resolve_session_id_from_tasks_dir(pid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }
    let output = Command::new("lsof")
        .args(["-p", &pid.to_string()])
        .output()
        .ok()?;
    parse_tasks_uuid_from_lsof(&String::from_utf8_lossy(&output.stdout))
}

/// Batch variant: run `lsof -p PID1,PID2,...` once for multiple PIDs and
/// return a map of pid → session_uuid.
pub fn resolve_session_ids_from_tasks_dir_batch(pids: &[u32]) -> HashMap<u32, String> {
    if pids.is_empty() {
        return HashMap::new();
    }
    let pid_arg = pids
        .iter()
        .filter(|&&p| p > 0)
        .map(|p| p.to_string())
        .collect::<Vec<_>>()
        .join(",");
    if pid_arg.is_empty() {
        return HashMap::new();
    }

    let Ok(output) = Command::new("lsof").args(["-p", &pid_arg]).output() else {
        return HashMap::new();
    };

    let text = String::from_utf8_lossy(&output.stdout);
    let mut map: HashMap<u32, String> = HashMap::new();

    for line in text.lines() {
        // lsof columns: COMMAND PID USER FD TYPE DEVICE SIZE/OFF NODE NAME
        // NAME is the last whitespace-separated token.
        let mut cols = line.split_whitespace();
        let _cmd = cols.next();
        let Some(pid_str) = cols.next() else { continue };
        let Ok(pid) = pid_str.parse::<u32>() else {
            continue;
        };
        if map.contains_key(&pid) {
            continue;
        }
        let name = line.split_whitespace().last().unwrap_or("");
        if let Some(uuid) = extract_tasks_uuid(name) {
            map.insert(pid, uuid);
        }
    }
    map
}

/// Parse lsof output for a single PID and extract the first `.claude/tasks/<uuid>` UUID.
fn parse_tasks_uuid_from_lsof(lsof_output: &str) -> Option<String> {
    for line in lsof_output.lines() {
        let name = line.split_whitespace().last().unwrap_or("");
        if let Some(uuid) = extract_tasks_uuid(name) {
            return Some(uuid);
        }
    }
    None
}

/// Given a file path (from lsof NAME column), extract the UUID from
/// a `.claude/tasks/<uuid>/` segment if present.
fn extract_tasks_uuid(path: &str) -> Option<String> {
    // Look for the pattern `.claude/tasks/<uuid>` in the path.
    let marker = ".claude/tasks/";
    let pos = path.find(marker)?;
    let after = &path[pos + marker.len()..];
    // UUID is up to the next `/` or end of string.
    let uuid_candidate: &str = after.split('/').next()?;
    // Validate: UUID is 32+ hex chars (with or without dashes, typically 36 chars).
    if uuid_candidate.len() >= 32
        && uuid_candidate
            .chars()
            .all(|c| c.is_ascii_hexdigit() || c == '-')
    {
        return Some(uuid_candidate.to_string());
    }
    None
}

// ---------------------------------------------------------------------------
// JSONL mapping
// ---------------------------------------------------------------------------

fn map_cwd_to_jsonl(engine: &Engine, cwd: &str, pid: u32) -> Option<PathBuf> {
    match engine {
        Engine::Claude => map_claude_cwd_to_jsonl(cwd, pid),
        Engine::Codex => map_codex_cwd_to_jsonl(cwd),
    }
}

fn map_claude_cwd_to_jsonl(cwd: &str, pid: u32) -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let projects_root = home.join(".claude").join("projects");

    let pid_start = if pid > 0 {
        get_pid_start_time(pid)
    } else {
        None
    };

    let encoded = encode_claude_project_dir(cwd);
    let dir = projects_root.join(&encoded);
    if dir.is_dir() {
        if let Some(path) = jsonl_matching_pid_start(&dir, pid_start) {
            return Some(path);
        }
    }

    // Try canonicalized path
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

    // Fallback: scan all discovered sessions matching this CWD
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
        candidates.sort_by(|a, b| a.started_at.cmp(&b.started_at));
        let best = candidates.iter().rev().find(|s| {
            let mtime = fs::metadata(&s.path)
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::SystemTime::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(u64::MAX);
            mtime >= start_unix.saturating_sub(30) && mtime <= start_unix + 120
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

/// Find the JSONL in `dir` whose mtime falls within 120 seconds after `pid_start`.
fn jsonl_matching_pid_start(
    dir: &Path,
    pid_start: Option<std::time::SystemTime>,
) -> Option<PathBuf> {
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
        candidates.sort_by(|a, b| a.0.cmp(&b.0));
        let best = candidates.iter().rev().find(|(mtime, _)| {
            let file_unix = mtime
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .ok()
                .map(|d| d.as_secs())
                .unwrap_or(u64::MAX);
            file_unix >= start_unix.saturating_sub(30) && file_unix <= start_unix + 120
        });
        if let Some((_, path)) = best {
            return Some(path.clone());
        }
    }

    // Fallback: newest file.
    candidates
        .into_iter()
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, p)| p)
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

/// Get the process start time as SystemTime.
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

// ---------------------------------------------------------------------------
// Session ID extraction from JSONL
// ---------------------------------------------------------------------------

fn extract_session_id_from_jsonl(path: &Path, engine: &Engine) -> Option<String> {
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

// ---------------------------------------------------------------------------
// Summary extraction
// ---------------------------------------------------------------------------

/// Extract a one-line summary for a session.
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
    let project = cwd.trim_end_matches('/').rsplit('/').next().unwrap_or(cwd);
    if !project.is_empty() && project != "." {
        return Some(project.to_string());
    }

    None
}

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

fn extract_first_user_prompt(path: &Path) -> Option<String> {
    let lines = super::discover::read_head_lines(path, 50);
    for line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let record: serde_json::Value = serde_json::from_str(trimmed).ok()?;

        // Claude format: type == "human" or "user"
        let msg_type = record.get("type").and_then(serde_json::Value::as_str);
        if matches!(msg_type, Some("human") | Some("user")) {
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

        // Codex responses API format
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

// ---------------------------------------------------------------------------
// API-spawned Codex sessions (no live process, detected via JSONL mtime)
// ---------------------------------------------------------------------------

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

            if already_discovered.contains(&path) {
                continue;
            }

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

            if is_last_event_stale(&path, ghost_threshold) {
                continue;
            }

            let head = super::discover::read_head_lines(&path, 30);
            let (id, _model, cwd) = parse_codex_api_head(&head);

            let session_id = id.map(|raw| truncate_codex_id(&raw));
            let cwd_str = cwd.unwrap_or_else(|| "unknown".to_string());
            let summary = extract_summary(session_id.as_deref(), Some(path.as_path()), &cwd_str);

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
                start_tvsec: 0,
            });
        }
    }

    if results.is_empty() {
        None
    } else {
        Some(results)
    }
}

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
        let ts_str = record
            .get("timestamp")
            .and_then(serde_json::Value::as_str)
            .or_else(|| {
                record
                    .pointer("/payload/timestamp")
                    .and_then(serde_json::Value::as_str)
            });
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
        None => true,
    }
}

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

fn truncate_codex_id(raw: &str) -> String {
    let hex: String = raw.chars().filter(|c| *c != '-').collect();
    if hex.len() > 8 {
        hex[hex.len() - 8..].to_string()
    } else {
        hex
    }
}
