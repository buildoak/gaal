use serde_json::Value;

#[derive(Debug, Clone)]
pub struct SessionEvent {
    pub timestamp: Option<String>,
    pub kind: EventKind,
}

#[derive(Debug, Clone)]
pub enum EventKind {
    Meta {
        session_id: Option<String>,
        model: Option<String>,
        cwd: Option<String>,
        version: Option<String>,
    },
    UserMessage {
        content: Vec<ContentBlock>,
    },
    AssistantMessage {
        content: Vec<ContentBlock>,
        model: Option<String>,
        stop_reason: Option<String>,
    },
    ToolUse(ToolUseEvent),
    ToolResult {
        tool_use_id: String,
        content: Option<String>,
        is_error: bool,
    },
    Usage {
        input_tokens: i64,
        output_tokens: i64,
        dedup_key: Option<String>,
    },
    SubagentProgress {
        agent_id: String,
        prompt: String,
        message: Option<Value>,
        timestamp: Option<String>,
        total_tokens: Option<i64>,
        total_duration_ms: Option<i64>,
        total_tool_use_count: Option<i64>,
    },
    SubagentCompletion {
        tool_use_id: String,
        result: Option<String>,
    },
    Summary {
        text: String,
    },
    StopSignal {
        reason: String,
    },
}

#[derive(Debug, Clone)]
pub struct ToolUseEvent {
    pub id: String,
    pub name: String,
    pub input: Value,
}

#[derive(Debug, Clone)]
pub enum ContentBlock {
    Text(String),
    Thinking,
    ToolUse(ToolUseEvent),
    ToolResult {
        tool_use_id: String,
        content: String,
    },
}
