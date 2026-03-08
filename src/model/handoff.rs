use serde::{Deserialize, Serialize};

/// Persisted handoff metadata produced by `gaal handoff`.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct HandoffRecord {
    /// Session ID for the handoff.
    pub session_id: String,
    /// One-line session headline.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headline: Option<String>,
    /// Project names extracted from the session.
    pub projects: Vec<String>,
    /// Topic keywords extracted from the session.
    pub keywords: Vec<String>,
    /// Substance score (0 = low signal).
    pub substance: i32,
    /// Session duration in minutes.
    pub duration_minutes: i32,
    /// Handoff generation timestamp.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generated_at: Option<String>,
    /// Generator engine/model label.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generated_by: Option<String>,
    /// Markdown file path for handoff content.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_path: Option<String>,
}
