use serde::{Deserialize, Serialize};

/// A denormalized session view used by `gaal ls` and `gaal show` JSON output.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SessionRecord {
    /// Session identifier.
    pub id: String,
    /// Engine name (for example: `claude`, `codex`).
    pub engine: String,
    /// Model name used in the session.
    pub model: String,
    /// Working directory associated with the session.
    pub cwd: String,
    /// Session start timestamp (RFC3339).
    pub started_at: String,
    /// Session end timestamp, if completed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<String>,
    /// Computed runtime status label.
    #[serde(default)]
    pub status: String,
    /// Session duration in seconds.
    pub duration_secs: u64,
    /// Aggregate token usage.
    pub tokens: TokenUsage,
    /// Maximum input tokens in any single API turn (peak context window usage).
    pub peak_context: u64,
    /// Total tool invocations.
    pub tools_used: u32,
    /// Total conversation turns.
    pub turns: u32,
    /// One-line session summary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headline: Option<String>,
    /// Files read/written/edited in this session.
    pub files: FileOps,
    /// Command execution entries.
    pub commands: Vec<CommandEntry>,
    /// Error entries and non-zero exits.
    pub errors: Vec<ErrorEntry>,
    /// Git operations observed in the session.
    pub git_ops: Vec<GitOp>,
    /// Source JSONL path.
    pub jsonl_path: String,
    /// Timestamp of the most recent indexed event.
    pub last_event_at: String,
    /// Exit signal if present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_signal: Option<String>,
    /// User-assigned or generated tags.
    pub tags: Vec<String>,
}

/// One command execution fact within a session.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CommandEntry {
    /// Command string.
    pub cmd: String,
    /// Command exit code.
    pub exit_code: i32,
    /// Timestamp of command execution.
    pub ts: String,
}

/// One error fact within a session.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ErrorEntry {
    /// Tool name that produced the error.
    pub tool: String,
    /// Command text or tool action summary.
    pub cmd: String,
    /// Exit code for the failed action.
    pub exit_code: i32,
    /// Truncated error snippet.
    pub snippet: String,
    /// Timestamp of error occurrence.
    pub ts: String,
}

/// One Git operation fact within a session.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct GitOp {
    /// Git operation type (for example: `commit`, `checkout`).
    pub op: String,
    /// Operation message or summary.
    pub message: String,
    /// Timestamp of the operation.
    pub ts: String,
}

/// Aggregate token counts for a session.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TokenUsage {
    /// Total input tokens.
    pub input: u64,
    /// Total output tokens.
    pub output: u64,
}

/// File operation buckets for a session.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FileOps {
    /// Files read or inspected.
    pub read: Vec<String>,
    /// Files newly written.
    pub written: Vec<String>,
    /// Files edited in-place.
    pub edited: Vec<String>,
}
