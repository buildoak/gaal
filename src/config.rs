use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Root Gaal configuration loaded from `~/.gaal/config.toml`.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
#[derive(Default)]
pub struct GaalConfig {
    /// LLM defaults used for generation operations.
    pub llm: LlmConfig,
    /// Handoff generation prompt and output format.
    pub handoff: HandoffConfig,
    /// Agent multiplexer executable settings.
    #[serde(rename = "agent-mux")]
    pub agent_mux: AgentMuxConfig,
    /// Default output directory for session markdown files.
    /// Used by `index backfill` when `--output-dir` is not specified.
    pub markdown_output_dir: Option<PathBuf>,
}

/// Default LLM engine/model settings.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct LlmConfig {
    /// Default engine (for example: `codex`).
    pub default_engine: String,
    /// Default model name.
    pub default_model: String,
    /// Timeout for LLM-dependent commands (in seconds).
    pub timeout_secs: u64,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            default_engine: "codex".to_string(),
            default_model: "gpt-5.4-mini".to_string(),
            timeout_secs: 120,
        }
    }
}

/// Handoff extraction configuration.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct HandoffConfig {
    /// Prompt path used by `gaal create-handoff`.
    pub prompt: PathBuf,
    /// Output format identifier.
    pub format: String,
}

impl Default for HandoffConfig {
    fn default() -> Self {
        Self {
            prompt: gaal_home().join("prompts").join("handoff.md"),
            format: "markdown".to_string(),
        }
    }
}

/// Agent-mux command settings.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct AgentMuxConfig {
    /// Binary name or path for agent-mux dispatch.
    pub path: String,
    /// Profile name for dispatch (maps to a prompt file via -P).
    pub profile: Option<String>,
    /// Base timeout override for agent-mux invocations (in seconds).
    /// Gaal derives a provider soft deadline and a grace-aware wrapper deadline.
    pub timeout_secs: Option<u64>,
    /// Default effort level for dispatch.
    pub effort: Option<String>,
    /// Override cwd for agent-mux dispatch.
    /// When set, this cwd is passed to agent-mux instead of the session's cwd.
    pub cwd: Option<String>,
}

impl Default for AgentMuxConfig {
    fn default() -> Self {
        Self {
            path: "agent-mux".to_string(),
            profile: None,
            timeout_secs: None,
            effort: Some("xhigh".to_string()),
            cwd: None,
        }
    }
}

/// Loads config from `~/.gaal/config.toml`, falling back to defaults when absent.
pub fn load_config() -> GaalConfig {
    let path = gaal_home().join("config.toml");
    match fs::read_to_string(path) {
        Ok(raw) => toml::from_str::<GaalConfig>(&raw).unwrap_or_default(),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => GaalConfig::default(),
        Err(_) => GaalConfig::default(),
    }
}

/// Returns the Gaal home directory path.
///
/// Resolution order:
/// 1. `GAAL_HOME` environment variable (if set and non-empty)
/// 2. `~/.gaal/` (default)
pub fn gaal_home() -> PathBuf {
    if let Ok(val) = std::env::var("GAAL_HOME") {
        if !val.is_empty() {
            return PathBuf::from(val);
        }
    }
    dirs::home_dir()
        .map(|path| path.join(".gaal"))
        .unwrap_or_else(|| PathBuf::from(".gaal"))
}

#[cfg(test)]
mod tests {
    use super::LlmConfig;

    #[test]
    fn default_handoff_model_matches_current_agent_mux_codex_roster() {
        let config = LlmConfig::default();
        assert_eq!(config.default_engine, "codex");
        assert_eq!(config.default_model, "gpt-5.4-mini");
    }

    #[test]
    fn default_agent_mux_effort_is_xhigh_for_handoffs() {
        let config = super::AgentMuxConfig::default();
        assert_eq!(config.effort.as_deref(), Some("xhigh"));
    }
}
