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
            default_model: "spark-high".to_string(),
            timeout_secs: 120,
        }
    }
}

/// Handoff extraction configuration.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct HandoffConfig {
    /// Prompt path used by `gaal handoff`.
    pub prompt: PathBuf,
    /// Output format identifier.
    pub format: String,
}

impl Default for HandoffConfig {
    fn default() -> Self {
        Self {
            prompt: gaal_home().join("prompts").join("handoff.md"),
            format: "eywa".to_string(),
        }
    }
}

/// Agent-mux command settings.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct AgentMuxConfig {
    /// Binary name or path for agent-mux dispatch.
    pub path: String,
    /// Default role for handoff dispatch.
    pub role: Option<String>,
    /// Default variant for role-based dispatch.
    pub variant: Option<String>,
    /// Timeout override for agent-mux invocations (in seconds).
    pub timeout_secs: Option<u64>,
    /// Default effort level for dispatch.
    pub effort: Option<String>,
}

impl Default for AgentMuxConfig {
    fn default() -> Self {
        Self {
            path: "agent-mux".to_string(),
            role: None,
            variant: None,
            timeout_secs: None,
            effort: None,
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

/// Returns the Gaal home directory path (`~/.gaal/`).
pub fn gaal_home() -> PathBuf {
    dirs::home_dir()
        .map(|path| path.join(".gaal"))
        .unwrap_or_else(|| PathBuf::from(".gaal"))
}
