use std::fmt::{Display, Formatter};
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::GaalError;

/// Session engine type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    /// Claude Code JSONL format.
    Claude,
    /// Codex JSONL format.
    Codex,
    /// Gemini session JSON format.
    Gemini,
}

impl Display for Engine {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let value = match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Gemini => "gemini",
        };
        write!(f, "{value}")
    }
}

impl FromStr for Engine {
    type Err = GaalError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "claude" => Ok(Self::Claude),
            "codex" => Ok(Self::Codex),
            "gemini" => Ok(Self::Gemini),
            other => Err(GaalError::ParseError(format!("invalid engine: {other}"))),
        }
    }
}

/// Parsed session metadata extracted from JSONL.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    /// Session identifier.
    pub id: String,
    /// Source engine.
    pub engine: Engine,
    /// Model name, when present.
    pub model: Option<String>,
    /// Working directory, when present.
    pub cwd: Option<String>,
    /// Session start timestamp in RFC3339.
    pub started_at: String,
    /// CLI version, when present.
    pub version: Option<String>,
    /// Parent session ID for Codex forked child sessions.
    pub forked_from_id: Option<String>,
    /// Codex subagent role (for example, "explorer"), when present.
    pub agent_role: Option<String>,
    /// Codex subagent nickname, when present.
    pub agent_nickname: Option<String>,
}

/// Parsed session payload with normalized facts and counters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedSession {
    /// Parsed session metadata.
    pub meta: SessionMeta,
    /// Session facts.
    pub facts: Vec<crate::model::Fact>,
    /// Aggregate input tokens (non-cached portion only).
    pub total_input_tokens: i64,
    /// Aggregate output tokens.
    pub total_output_tokens: i64,
    /// Aggregate cache read tokens.
    pub cache_read_tokens: i64,
    /// Aggregate cache creation tokens.
    pub cache_creation_tokens: i64,
    /// Aggregate reasoning tokens (Codex reasoning_output_tokens, future Claude).
    pub reasoning_tokens: i64,
    /// Maximum input tokens seen in any single API turn (peak context window usage).
    pub peak_context: i64,
    /// Total tool calls seen.
    pub total_tools: i32,
    /// Total turn count.
    pub total_turns: i32,
    /// Session end timestamp.
    pub ended_at: Option<String>,
    /// Exit signal (for example, stop reason).
    pub exit_signal: Option<String>,
    /// Last event timestamp seen in stream.
    pub last_event_at: Option<String>,
    /// Session-level summary text, when present (e.g. Gemini root `summary` field).
    pub session_summary: Option<String>,
}
