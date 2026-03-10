use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::GaalError;

/// Computed runtime/archive status for a session.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "lowercase")]
pub enum SessionStatus {
    /// Process alive and events are flowing.
    Active,
    /// Process alive but no recent activity.
    Idle,
    /// Process alive and one or more stuck signals present.
    Stuck,
    /// Session ended cleanly.
    Completed,
    /// Session ended with failure conditions.
    Failed,
    /// Session was killed or crashed without clean exit.
    Interrupted,
    /// Status could not be determined.
    Unknown,
}

impl fmt::Display for SessionStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Active => "active",
            Self::Idle => "idle",
            Self::Stuck => "stuck",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Interrupted => "interrupted",
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
            "stuck" => Ok(Self::Stuck),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "interrupted" => Ok(Self::Interrupted),
            "unknown" => Ok(Self::Unknown),
            other => Err(GaalError::ParseError(format!(
                "invalid session status: {other}"
            ))),
        }
    }
}

/// Inputs used by stuck-status detection heuristics.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct StuckSignals {
    /// Seconds since last observed activity.
    pub silence_secs: u64,
    /// True when rolling action signature indicates a loop.
    pub loop_detected: bool,
    /// Percentage of context window consumed (0-100).
    pub context_pct: f64,
    /// True when waiting on unresolved permission gate.
    pub permission_blocked: bool,
}

pub const IDLE_SECS: u64 = 120;
pub const STUCK_SILENCE_SECS: u64 = 300;

/// Inputs required to compute a session's authoritative status.
#[derive(Debug, Clone, Copy)]
pub struct StatusParams<'a> {
    pub ended_at: Option<&'a str>,
    pub exit_signal: Option<&'a str>,
    pub pid_alive: bool,
    pub silence_secs: u64,
    pub loop_detected: bool,
    pub context_pct: f64,
    pub permission_blocked: bool,
    pub stuck_silence_secs: u64,
    pub executing_command: bool,
    /// True when the last action is an Agent tool dispatch waiting for subagent completion.
    pub executing_agent: bool,
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

    // CPU-awareness: if process CPU > 1%, it's working, not stuck
    if params.cpu_pct > 1.0 {
        if params.silence_secs >= IDLE_SECS {
            return SessionStatus::Idle;
        } else {
            return SessionStatus::Active;
        }
    }

    // Extend silence tolerance when executing a command (build in progress)
    // or waiting for a subagent (Agent tool dispatch).
    let effective_silence = if params.executing_command || params.executing_agent {
        params.stuck_silence_secs.saturating_mul(3)
    } else {
        params.stuck_silence_secs
    };
    let silence_stuck = params.silence_secs >= effective_silence && !params.permission_blocked;

    // Don't mark as stuck if context percentage alone is the trigger - high context is normal
    let context_only_stuck = params.context_pct >= 95.0
        && !silence_stuck
        && !params.loop_detected
        && !params.permission_blocked;

    if (silence_stuck || params.loop_detected || params.permission_blocked) && !context_only_stuck {
        SessionStatus::Stuck
    } else if params.silence_secs >= IDLE_SECS {
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
