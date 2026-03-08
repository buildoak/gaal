use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::GaalError;

/// Fact category stored in the `facts` table.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "snake_case")]
pub enum FactType {
    /// File read access (Read/Grep/Glob).
    FileRead,
    /// File creation or modification.
    FileWrite,
    /// Shell command execution.
    Command,
    /// Error event or non-zero exit.
    Error,
    /// Git operation event.
    GitOp,
    /// User prompt content.
    UserPrompt,
    /// Assistant reply content.
    AssistantReply,
    /// Child task/session spawn event.
    TaskSpawn,
}

impl FactType {
    /// Returns the canonical snake_case name used by SQLite constraints.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::FileRead => "file_read",
            Self::FileWrite => "file_write",
            Self::Command => "command",
            Self::Error => "error",
            Self::GitOp => "git_op",
            Self::UserPrompt => "user_prompt",
            Self::AssistantReply => "assistant_reply",
            Self::TaskSpawn => "task_spawn",
        }
    }
}

impl FromStr for FactType {
    type Err = GaalError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "file_read" => Ok(Self::FileRead),
            "file_write" => Ok(Self::FileWrite),
            "command" => Ok(Self::Command),
            "error" => Ok(Self::Error),
            "git_op" => Ok(Self::GitOp),
            "user_prompt" => Ok(Self::UserPrompt),
            "assistant_reply" => Ok(Self::AssistantReply),
            "task_spawn" => Ok(Self::TaskSpawn),
            other => Err(GaalError::ParseError(format!("invalid fact_type: {other}"))),
        }
    }
}

/// A normalized atomic event extracted from a session.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Fact {
    /// Autoincrement row ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<i64>,
    /// Session ID that owns this fact.
    pub session_id: String,
    /// Event timestamp.
    pub ts: String,
    /// Turn number associated with the fact.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_number: Option<i32>,
    /// Event type.
    pub fact_type: FactType,
    /// Optional subject (file path, command summary, etc.).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    /// Optional detail payload.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Optional command/tool exit code.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// Optional success flag.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub success: Option<bool>,
}
