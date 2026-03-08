use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::parser::types::Engine;

/// Lightweight metadata discovered from a session JSONL file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredSession {
    /// Session identifier.
    pub id: String,
    /// Agent engine.
    pub engine: Engine,
    /// Full JSONL path.
    pub path: PathBuf,
    /// Model name if present in head records.
    pub model: Option<String>,
    /// Working directory if present in head records.
    pub cwd: Option<String>,
    /// Session start timestamp if found in head records (RFC3339).
    pub started_at: Option<String>,
    /// File size in bytes.
    pub file_size: u64,
}

/// Discover sessions across all supported engines.
pub fn discover_sessions(engine_filter: Option<Engine>) -> Result<Vec<DiscoveredSession>> {
    let mut sessions = Vec::new();
    sessions.extend(super::claude::discover_claude_sessions()?);
    sessions.extend(super::codex::discover_codex_sessions()?);

    if let Some(engine) = engine_filter {
        sessions.retain(|s| s.engine == engine);
    }

    sessions.sort_by(|a, b| {
        b.started_at
            .as_deref()
            .cmp(&a.started_at.as_deref())
            .then_with(|| a.id.cmp(&b.id))
    });
    Ok(sessions)
}

/// Read up to `n` lines from the start of a file.
pub(crate) fn read_head_lines(path: &Path, n: usize) -> Vec<String> {
    let Ok(file) = File::open(path) else {
        return Vec::new();
    };
    let reader = BufReader::new(file);
    reader
        .lines()
        .take(n)
        .filter_map(|line| line.ok())
        .collect()
}
