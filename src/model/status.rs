use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::GaalError;

/// Computed runtime/archive status for a session.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SessionStatus {
    /// Process alive and events are flowing.
    Active,
    /// Process alive but no recent activity.
    Idle,
    /// Session ended cleanly.
    Completed,
    /// Session ended with failure conditions.
    Failed,
    /// Session was killed or crashed without clean exit.
    Interrupted,
    /// API-discovered session with no live process, very recent mtime (< 2 min).
    Starting,
    /// Status could not be determined.
    Unknown,
}

impl fmt::Display for SessionStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Active => "active",
            Self::Idle => "idle",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Interrupted => "interrupted",
            Self::Starting => "starting",
            Self::Unknown => "unknown",
        })
    }
}

impl FromStr for SessionStatus {
    type Err = GaalError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "active" => Ok(Self::Active),
            "idle" => Ok(Self::Idle),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "interrupted" => Ok(Self::Interrupted),
            "starting" => Ok(Self::Starting),
            "unknown" => Ok(Self::Unknown),
            other => Err(GaalError::ParseError(format!(
                "invalid session status: {other}"
            ))),
        }
    }
}

pub const IDLE_SECS: u64 = 120;

/// Inputs required to compute a session's authoritative status.
#[derive(Debug, Clone, Copy)]
pub struct StatusParams<'a> {
    pub ended_at: Option<&'a str>,
    pub exit_signal: Option<&'a str>,
    pub pid_alive: bool,
    pub silence_secs: u64,
    pub cpu_pct: f64,
}

pub fn compute_session_status(params: &StatusParams<'_>) -> SessionStatus {
    if params.ended_at.is_some() {
        if is_failed_exit(params.exit_signal) {
            return SessionStatus::Failed;
        }
        return SessionStatus::Completed;
    }

    if !params.pid_alive {
        // Process is dead but no ended_at was set — session was killed or crashed.
        if params.exit_signal.is_some() {
            if is_failed_exit(params.exit_signal) {
                return SessionStatus::Failed;
            }
            return SessionStatus::Completed;
        }
        // No exit signal, no ended_at — interrupted (killed mid-stream).
        return SessionStatus::Interrupted;
    }

    // CPU-awareness: if process CPU > 1%, it's actively working.
    if params.cpu_pct > 1.0 {
        if params.silence_secs >= IDLE_SECS {
            return SessionStatus::Idle;
        } else {
            return SessionStatus::Active;
        }
    }

    if params.silence_secs >= IDLE_SECS {
        SessionStatus::Idle
    } else {
        SessionStatus::Active
    }
}

fn is_failed_exit(exit_signal: Option<&str>) -> bool {
    matches!(
        exit_signal
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "error" | "max_tokens" | "killed" | "failed"
    )
}
