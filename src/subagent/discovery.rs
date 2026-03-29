use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct SubagentFile {
    pub agent_id: String,
    pub path: PathBuf,
    pub parent_session_dir: PathBuf,
    pub file_size: u64,
}

/// Scan session_dir/subagents/ for agent-*.jsonl files.
/// Returns empty Vec if subagents/ doesn't exist. Never errors.
pub fn collect_subagent_files(session_dir: &Path) -> Vec<SubagentFile> {
    let subagents_dir = session_dir.join("subagents");
    if !subagents_dir.exists() || !subagents_dir.is_dir() {
        return Vec::new();
    }
    let mut files = Vec::new();
    let Ok(entries) = fs::read_dir(&subagents_dir) else {
        return files;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.starts_with("agent-") || !name.ends_with(".jsonl") {
            continue;
        }
        let agent_id = name
            .trim_start_matches("agent-")
            .trim_end_matches(".jsonl")
            .to_string();
        if agent_id.is_empty() {
            continue;
        }
        let file_size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        files.push(SubagentFile {
            agent_id,
            path,
            parent_session_dir: session_dir.to_path_buf(),
            file_size,
        });
    }
    files
}

/// Scan all session dirs under projects_root for subagent files.
pub fn collect_all_subagent_files(projects_root: &Path) -> Vec<SubagentFile> {
    let mut all = Vec::new();
    let Ok(project_dirs) = fs::read_dir(projects_root) else {
        return all;
    };
    for project_entry in project_dirs.flatten() {
        if !project_entry
            .file_type()
            .map(|ft| ft.is_dir())
            .unwrap_or(false)
        {
            continue;
        }
        let Ok(session_entries) = fs::read_dir(project_entry.path()) else {
            continue;
        };
        for session_entry in session_entries.flatten() {
            if !session_entry
                .file_type()
                .map(|ft| ft.is_dir())
                .unwrap_or(false)
            {
                continue;
            }
            all.extend(collect_subagent_files(&session_entry.path()));
        }
    }
    all
}
