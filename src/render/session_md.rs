//! Core session-to-markdown renderer.
//!
//! Converts session event streams into human-readable markdown output.
//! Uses the unified parser pipeline: JSONL → parse_events() → SessionData → markdown.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use anyhow::Result;
use chrono::{DateTime, FixedOffset, Utc};
use regex::Regex;
use serde_json::Value;

use crate::parser::event::{
    ContentBlock as ParserContentBlock, EventKind, SessionEvent, ToolUseEvent,
};
use crate::parser::{claude, codex, detect_engine};
use crate::parser::types::Engine;

// Dubai timezone: UTC+4.
const DUBAI_OFFSET_SECS: i32 = 4 * 3600;

/// Default truncation limit for turn content (chars).
const TRUNCATION_LIMIT: usize = 100_000;
/// Preview size when truncation applies.
const TRUNCATION_PREVIEW: usize = 5_000;

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

/// A single content block within a turn.
#[derive(Debug, Clone)]
enum ContentBlock {
    Text {
        text: String,
    },
    Thinking,
    ToolUse {
        name: String,
        input: Value,
        id: String,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
    },
}

/// A conversation turn (one user message + one or more assistant responses).
#[derive(Debug, Clone)]
struct Turn {
    #[allow(dead_code)]
    turn_number: i32,
    user_content: Vec<ContentBlock>,
    assistant_content: Vec<ContentBlock>,
    timestamp_start: Option<String>,
    timestamp_end: Option<String>,
    #[allow(dead_code)]
    model: Option<String>,
}

/// Parsed session ready for rendering.
#[derive(Debug)]
struct SessionData {
    session_id: String,
    summary: Option<String>,
    turns: Vec<Turn>,
    timestamp_start: Option<String>,
    timestamp_end: Option<String>,
    duration_seconds: Option<f64>,
    models_used: Vec<String>,
    subagent_deltas: Vec<SubagentDelta>,
}

/// Rich Task annotation returned by tool annotation formatting.
#[derive(Debug, Clone)]
struct TaskInfo {
    description: String,
    prompt: String,
    model: String,
    #[allow(dead_code)]
    subagent_type: String,
    tool_id: String,
}

/// Tool annotation: either a simple string or a rich Task.
#[derive(Debug, Clone)]
enum ToolAnnotation {
    Simple(String),
    Task(TaskInfo),
}

/// Subagent activity delta extracted from progress records.
#[derive(Debug, Clone)]
struct SubagentDelta {
    agent_id: String,
    #[allow(dead_code)]
    prompt: String,
    files_read: Vec<String>,
    files_written: Vec<(String, String)>,
    commands: Vec<String>,
    tool_counts: HashMap<String, usize>,
    timestamps: Vec<String>,
    total_tokens: Option<i64>,
    total_duration_ms: Option<i64>,
    total_tool_use_count: Option<i64>,
}

/// Collected subagent info from Task tool calls.
#[derive(Debug)]
struct SubagentInfo {
    #[allow(dead_code)]
    description: String,
    #[allow(dead_code)]
    prompt: String,
    model: String,
}

/// Aggregated stats pulled from subagent tool results.
#[derive(Debug, Clone, Default)]
struct SubagentToolResultStats {
    total_tokens: Option<i64>,
    total_duration_ms: Option<i64>,
    total_tool_use_count: Option<i64>,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Render a JSONL session file to a markdown string.
///
/// Uses the unified pipeline: JSONL → parse_events() → SessionData → markdown.
pub fn render_session_markdown(path: &Path) -> Result<String> {
    let engine = detect_engine(path)?;
    let events = match engine {
        Engine::Claude => claude::parse_events(path)?,
        Engine::Codex => codex::parse_events(path)?,
    };
    let session = events_to_session_data(&events, path);
    Ok(session_to_markdown(&session))
}

// ---------------------------------------------------------------------------
// Truncation helpers
// ---------------------------------------------------------------------------

/// Truncate human prompt: full if <= limit, else first preview chars + note.
fn truncate_human(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if trimmed.len() <= TRUNCATION_LIMIT {
        return trimmed.to_string();
    }
    let preview: String = trimmed.chars().take(TRUNCATION_PREVIEW).collect();
    format!(
        "{}\n\n[... truncated from {} chars]",
        preview,
        trimmed.len()
    )
}

/// Truncate Claude response: full if <= limit, else first preview chars + note.
fn truncate_claude(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if trimmed.len() <= TRUNCATION_LIMIT {
        return trimmed.to_string();
    }
    let preview: String = trimmed.chars().take(TRUNCATION_PREVIEW).collect();
    format!(
        "{}\n\n[... continued, {} chars total]",
        preview,
        trimmed.len()
    )
}

/// Detect skill injection messages and replace with succinct note.
fn filter_skill_injection(text: &str) -> String {
    if text.is_empty() {
        return text.to_string();
    }
    let trimmed = text.trim();
    if trimmed.starts_with("Base directory for this skill:") {
        if let Some(first_line) = trimmed.lines().next() {
            if let Some((_prefix, path)) = first_line.split_once(':') {
                return format!("[Skill loaded: `{}`]", path.trim());
            }
        }
    }
    text.to_string()
}

// ---------------------------------------------------------------------------
// Time formatting
// ---------------------------------------------------------------------------

fn dubai_offset() -> FixedOffset {
    FixedOffset::east_opt(DUBAI_OFFSET_SECS).expect("Dubai UTC+4 offset is valid")
}

/// Parse ISO timestamp and convert to Dubai time (UTC+4).
fn parse_ts(ts: Option<&str>) -> Option<DateTime<FixedOffset>> {
    let raw = ts?;
    // Try RFC3339 first (handles Z and offset suffixes).
    if let Ok(dt) = DateTime::parse_from_rfc3339(raw) {
        return Some(dt.with_timezone(&dubai_offset()));
    }
    // Fallback: replace Z with +00:00 and retry.
    let normalized = raw.replace('Z', "+00:00");
    if let Ok(dt) = DateTime::parse_from_rfc3339(&normalized) {
        return Some(dt.with_timezone(&dubai_offset()));
    }
    // Last resort: try parsing as UTC naive.
    if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S%.f") {
        let utc = DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc);
        return Some(utc.with_timezone(&dubai_offset()));
    }
    None
}

/// Format timestamp as HH:MM in Dubai time.
fn fmt_time(ts: Option<&str>) -> String {
    match parse_ts(ts) {
        Some(dt) => dt.format("%H:%M").to_string(),
        None => "??:??".to_string(),
    }
}

/// Format duration in seconds as Xh Ym or Xm.
fn fmt_duration(seconds: Option<f64>) -> String {
    let Some(secs) = seconds else {
        return "unknown".to_string();
    };
    if secs <= 0.0 {
        return "unknown".to_string();
    }
    let total = secs as i64;
    let h = total / 3600;
    let m = (total % 3600) / 60;
    if h > 0 {
        format!("{h}h {m}m")
    } else {
        format!("{m}m")
    }
}

/// Simplify model name for display.
fn fmt_model(model: &str) -> String {
    if model.is_empty() {
        return "unknown".to_string();
    }
    let lower = model.to_lowercase();
    if lower.contains("opus") {
        return "Opus".to_string();
    }
    if lower.contains("sonnet") {
        return "Sonnet".to_string();
    }
    if lower.contains("haiku") {
        return "Haiku".to_string();
    }
    if model.contains('-') {
        model.split('-').next().unwrap_or(model).to_string()
    } else {
        model.to_string()
    }
}

// ---------------------------------------------------------------------------
// Tool annotation formatters
// ---------------------------------------------------------------------------

/// Format a tool_use content block as an inline annotation.
fn fmt_tool_annotation(name: &str, input: &Value, tool_id: &str) -> Option<ToolAnnotation> {
    let inp = resolve_input(input);

    match name {
        "Read" => {
            let path = get_str(&inp, "file_path").unwrap_or("?");
            let offset = inp.get("offset").and_then(Value::as_i64);
            let limit = inp.get("limit").and_then(Value::as_i64);
            let annotation = match (offset, limit) {
                (Some(off), Some(lim)) => {
                    format!("-> Read: `{path}` (lines {off}-{})", off + lim)
                }
                (None, Some(lim)) => {
                    format!("-> Read: `{path}` (first {lim} lines)")
                }
                _ => format!("-> Read: `{path}`"),
            };
            Some(ToolAnnotation::Simple(annotation))
        }
        "Write" => {
            let path = get_str(&inp, "file_path").unwrap_or("?");
            Some(ToolAnnotation::Simple(format!("-> Write: `{path}`")))
        }
        "Edit" => {
            let path = get_str(&inp, "file_path").unwrap_or("?");
            Some(ToolAnnotation::Simple(format!("-> Edit: `{path}`")))
        }
        "Grep" | "Glob" => {
            let pattern = get_str(&inp, "pattern").unwrap_or("?");
            Some(ToolAnnotation::Simple(format!("-> Search: `{pattern}`")))
        }
        "Bash" => {
            let cmd = get_str(&inp, "command").unwrap_or("?");
            let display = if cmd.len() > 60 {
                format!("{}...", &cmd[..57.min(cmd.len())])
            } else {
                cmd.to_string()
            };
            Some(ToolAnnotation::Simple(format!("-> Bash: `{display}`")))
        }
        "Task" => {
            let desc = get_str(&inp, "description").unwrap_or("").to_string();
            let prompt = get_str(&inp, "prompt").unwrap_or("").to_string();
            let model = get_str(&inp, "model").unwrap_or("sonnet").to_string();
            let subagent_type = get_str(&inp, "subagent_type").unwrap_or("").to_string();
            Some(ToolAnnotation::Task(TaskInfo {
                description: desc,
                prompt,
                model,
                subagent_type,
                tool_id: tool_id.to_string(),
            }))
        }
        "WebFetch" => {
            let url = get_str(&inp, "url").unwrap_or("?");
            Some(ToolAnnotation::Simple(format!("-> WebFetch: `{url}`")))
        }
        "WebSearch" => {
            let query = get_str(&inp, "query").unwrap_or("?");
            Some(ToolAnnotation::Simple(format!("-> WebSearch: `{query}`")))
        }
        _ => Some(ToolAnnotation::Simple(format!("-> {name}"))),
    }
}

/// Resolve tool input, handling truncated inputs.
fn resolve_input(input: &Value) -> Value {
    if let Some(obj) = input.as_object() {
        if obj.contains_key("_truncated") {
            if let Some(raw) = obj.get("_truncated").and_then(Value::as_str) {
                // Try to extract useful info from truncated string.
                if raw.contains("file_path") {
                    let attempt =
                        format!("{}}}", raw.trim_end_matches("...").trim_end_matches('}'));
                    if let Ok(parsed) = serde_json::from_str::<Value>(&format!("{attempt}}}")) {
                        return parsed;
                    }
                }
            }
            return Value::Object(serde_json::Map::new());
        }
    }
    input.clone()
}

fn get_str<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

/// Format an integer with comma separators (e.g., 1234567 -> "1,234,567").
fn format_with_commas(n: i64) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let len = bytes.len();
    if len <= 3 {
        return s;
    }
    let mut result = String::with_capacity(len + len / 3);
    for (i, &b) in bytes.iter().enumerate() {
        if i > 0 && (len - i) % 3 == 0 {
            result.push(',');
        }
        result.push(b as char);
    }
    result
}

// ---------------------------------------------------------------------------
// Content extraction
// ---------------------------------------------------------------------------

/// Extract concatenated text from content blocks.
fn extract_text_from_blocks(blocks: &[ContentBlock]) -> String {
    let texts: Vec<&str> = blocks
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text } => {
                let trimmed = text.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed)
                }
            }
            _ => None,
        })
        .collect();
    texts.join("\n\n")
}

/// Extract tool annotations from content blocks.
fn extract_tool_annotations(blocks: &[ContentBlock]) -> Vec<ToolAnnotation> {
    blocks
        .iter()
        .filter_map(|b| match b {
            ContentBlock::ToolUse { name, input, id } => fmt_tool_annotation(name, input, id),
            _ => None,
        })
        .collect()
}

/// Extract tool_result blocks indexed by tool_use_id.
fn extract_tool_results(blocks: &[ContentBlock]) -> HashMap<String, String> {
    let mut results = HashMap::new();
    for block in blocks {
        if let ContentBlock::ToolResult {
            tool_use_id,
            content,
        } = block
        {
            if !tool_use_id.is_empty() {
                results.insert(tool_use_id.clone(), content.clone());
            }
        }
    }
    results
}

// ---------------------------------------------------------------------------
// Task block formatting
// ---------------------------------------------------------------------------

/// Format a Task (subagent) block with prompt, result, and inline delta.
fn fmt_task_block(task: &TaskInfo, result: Option<&str>, delta: Option<&SubagentDelta>) -> String {
    let mut lines = Vec::new();
    let model = fmt_model(&task.model);
    let agent_label = if task.subagent_type.is_empty() {
        String::new()
    } else {
        format!("{} ", task.subagent_type)
    };
    lines.push(format!(
        "-> **Subagent** ({agent_label}{model}): {}",
        task.description
    ));
    lines.push(String::new());

    // Full prompt.
    if !task.prompt.is_empty() {
        lines.push("**Prompt given:**".to_string());
        lines.push(format!("> {}", task.prompt.replace('\n', "\n> ")));
        lines.push(String::new());
    }

    // Full result.
    if let Some(raw_content) = result {
        let mut content = raw_content.to_string();

        // Strip <usage> block.
        if content.contains("<usage>") {
            let re = Regex::new(r"(?s)<usage>.*?</usage>").expect("usage regex");
            content = re.replace_all(&content, "").trim().to_string();
        }

        // Strip agentId line.
        if content.contains("agentId:") {
            let re = Regex::new(r"agentId: \w+[^\n]*\n?").expect("agentId regex");
            content = re.replace_all(&content, "").trim().to_string();
        }

        lines.push("**Result returned:**".to_string());
        lines.push(format!("> {}", content.replace('\n', "\n> ")));
        lines.push(String::new());

        // Delta info.
        if let Some(d) = delta {
            let mut delta_parts = Vec::new();

            // Files written.
            if !d.files_written.is_empty() {
                if d.files_written.len() == 1 {
                    let filename = Path::new(&d.files_written[0].0)
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or(&d.files_written[0].0);
                    delta_parts.push(format!("1 file written (`{filename}`)"));
                } else {
                    delta_parts.push(format!("{} files written", d.files_written.len()));
                }
            }

            // Commands.
            if !d.commands.is_empty() {
                if d.commands.len() == 1 {
                    let cmd = &d.commands[0];
                    let cmd_short = if cmd.len() > 40 {
                        format!("{}...", &cmd[..37.min(cmd.len())])
                    } else {
                        cmd.clone()
                    };
                    delta_parts.push(format!("1 command (`{cmd_short}`)"));
                } else {
                    delta_parts.push(format!("{} commands", d.commands.len()));
                }
            }

            // Key tool counts.
            for tool in &["WebSearch", "WebFetch", "Read", "Edit", "Grep", "Glob"] {
                if let Some(&count) = d.tool_counts.get(*tool) {
                    if count > 0 {
                        delta_parts.push(format!("{count} {tool}"));
                    }
                }
            }

            // Stats line.
            let mut stats_parts = Vec::new();
            if let Some(duration_ms) = d.total_duration_ms {
                let secs = duration_ms / 1000;
                let mins = secs / 60;
                let rem = secs % 60;
                if mins > 0 {
                    stats_parts.push(format!("{mins}m {rem}s"));
                } else {
                    stats_parts.push(format!("{secs}s"));
                }
            }
            if let Some(tokens) = d.total_tokens {
                if tokens >= 1000 {
                    stats_parts.push(format!("{}k tokens", tokens / 1000));
                } else {
                    stats_parts.push(format!("{tokens} tokens"));
                }
            }
            if let Some(tool_count) = d.total_tool_use_count {
                stats_parts.push(format!("{tool_count} tool calls"));
            }

            if !delta_parts.is_empty() {
                lines.push(format!("**Delta:** {}", delta_parts.join(", ")));
            }
            if !stats_parts.is_empty() {
                lines.push(format!("*({})*", stats_parts.join(", ")));
            }
        }
    }

    lines.push(String::new());
    lines.join("\n")
}

// ---------------------------------------------------------------------------
// File / command / subagent collection
// ---------------------------------------------------------------------------

/// Collect files read and written from all turns.
fn collect_files(turns: &[Turn]) -> (Vec<String>, Vec<(String, String)>) {
    let mut reads = Vec::new();
    let mut writes: Vec<(String, String)> = Vec::new();

    for turn in turns {
        for block in &turn.assistant_content {
            let ContentBlock::ToolUse { name, input, .. } = block else {
                continue;
            };
            if input
                .as_object()
                .map_or(false, |o| o.contains_key("_truncated"))
            {
                continue;
            }
            match name.as_str() {
                "Read" => {
                    if let Some(path) = get_str(input, "file_path") {
                        if !reads.contains(&path.to_string()) {
                            reads.push(path.to_string());
                        }
                    }
                }
                "Write" => {
                    if let Some(path) = get_str(input, "file_path") {
                        writes.push((path.to_string(), "created".to_string()));
                    }
                }
                "Edit" => {
                    if let Some(path) = get_str(input, "file_path") {
                        writes.push((path.to_string(), "edited".to_string()));
                    }
                }
                _ => {}
            }
        }
    }

    // Dedupe writes, keep last action.
    let mut seen: HashMap<String, String> = HashMap::new();
    for (path, action) in &writes {
        seen.insert(path.clone(), action.clone());
    }
    let writes_deduped: Vec<(String, String)> = seen.into_iter().collect();

    (reads, writes_deduped)
}

/// Collect bash commands from all turns.
fn collect_commands(turns: &[Turn]) -> Vec<String> {
    let mut commands = Vec::new();
    for turn in turns {
        for block in &turn.assistant_content {
            if let ContentBlock::ToolUse { name, input, .. } = block {
                if name == "Bash" {
                    if let Some(cmd) = get_str(input, "command") {
                        if !commands.contains(&cmd.to_string()) {
                            commands.push(cmd.to_string());
                        }
                    }
                }
            }
        }
    }
    commands
}

/// Collect subagent info from Task tool calls (for model mapping).
fn collect_subagents(turns: &[Turn]) -> Vec<SubagentInfo> {
    let mut agents = Vec::new();
    for turn in turns {
        for block in &turn.assistant_content {
            if let ContentBlock::ToolUse { name, input, .. } = block {
                if name == "Task" {
                    let desc = get_str(input, "description").unwrap_or("").to_string();
                    let prompt_raw = get_str(input, "prompt").unwrap_or("");
                    let prompt: String = prompt_raw.chars().take(100).collect();
                    let model = get_str(input, "model").unwrap_or("sonnet").to_string();
                    agents.push(SubagentInfo {
                        description: desc,
                        prompt,
                        model,
                    });
                }
            }
        }
    }
    agents
}

// ---------------------------------------------------------------------------
// Open threads extraction
// ---------------------------------------------------------------------------

/// Extract TODOs and next steps from final responses.
fn extract_open_threads(turns: &[Turn]) -> Vec<String> {
    let mut threads = Vec::new();
    if turns.is_empty() {
        return threads;
    }

    let start = if turns.len() >= 2 { turns.len() - 2 } else { 0 };

    for turn in &turns[start..] {
        let text = extract_text_from_blocks(&turn.assistant_content);
        for line in text.lines() {
            let lower = line.to_lowercase();
            let trimmed = lower.trim();
            if ["todo", "fixme", "next step", "remain", "still need"]
                .iter()
                .any(|marker| trimmed.contains(marker))
            {
                let clean = line.trim().trim_start_matches(&['-', '*', '#'][..]).trim();
                if !clean.is_empty() && clean.len() < 200 {
                    threads.push(clean.to_string());
                }
            }
        }
    }

    threads.truncate(5);
    threads
}

// ---------------------------------------------------------------------------
// Event-to-SessionData conversion
// ---------------------------------------------------------------------------

/// Convert a canonical event stream into a `SessionData` ready for rendering.
///
/// Replaces the old `parse_jsonl_to_session` — works with both Claude and
/// Codex events since both emit the same `SessionEvent` types.
fn events_to_session_data(events: &[SessionEvent], path: &Path) -> SessionData {
    let mut session_id: Option<String> = None;
    let mut summary: Option<String> = None;
    let mut timestamps: Vec<String> = Vec::new();
    let mut models: BTreeSet<String> = BTreeSet::new();

    // -- Build turns from events --
    let mut turns: Vec<Turn> = Vec::new();
    let mut current_turn: Option<Turn> = None;
    let mut turn_number = 0i32;

    // Subagent tracking.
    let mut subagent_deltas_map: HashMap<String, SubagentDelta> = HashMap::new();
    // Extracted from tool_result payloads (legacy toolUseResult parity).
    let mut subagent_stats_by_agent_id: HashMap<String, SubagentToolResultStats> = HashMap::new();
    // Map tool_use_id → tool_result content for Task matching.
    let mut tool_result_contents: HashMap<String, String> = HashMap::new();

    for event in events {
        if let Some(ts) = &event.timestamp {
            timestamps.push(ts.clone());
        }

        match &event.kind {
            EventKind::Meta {
                session_id: sid,
                model,
                ..
            } => {
                if session_id.is_none() {
                    if let Some(id) = sid {
                        session_id = Some(id.clone());
                    }
                }
                if let Some(m) = model {
                    if !m.starts_with('<') {
                        models.insert(m.clone());
                    }
                }
            }

            EventKind::Summary { text } => {
                summary = Some(text.clone());
            }

            EventKind::UserMessage { content } => {
                // Check for interruption.
                let is_interrupt = content.iter().any(|block| {
                    if let ParserContentBlock::Text(text) = block {
                        text.contains("[Request interrupted by user]")
                    } else {
                        false
                    }
                });
                if is_interrupt {
                    continue;
                }

                // Save previous turn.
                if let Some(t) = current_turn.take() {
                    turns.push(t);
                }

                turn_number += 1;
                let user_blocks = convert_content_blocks(content);

                // Also extract tool results from user message (Claude sends them here).
                for block in content {
                    if let ParserContentBlock::ToolResult {
                        tool_use_id,
                        content: result_content,
                    } = block
                    {
                        if !tool_use_id.is_empty() {
                            tool_result_contents
                                .insert(tool_use_id.clone(), result_content.clone());
                        }
                        if let Some((agent_id, stats)) =
                            extract_subagent_tool_result_stats(result_content)
                        {
                            subagent_stats_by_agent_id.insert(agent_id, stats);
                        }
                    }
                }

                current_turn = Some(Turn {
                    turn_number,
                    user_content: user_blocks,
                    assistant_content: Vec::new(),
                    timestamp_start: event.timestamp.clone(),
                    timestamp_end: None,
                    model: None,
                });
            }

            EventKind::AssistantMessage {
                content,
                model,
                ..
            } => {
                if current_turn.is_none() {
                    turn_number += 1;
                    current_turn = Some(Turn {
                        turn_number,
                        user_content: Vec::new(),
                        assistant_content: Vec::new(),
                        timestamp_start: event.timestamp.clone(),
                        timestamp_end: None,
                        model: None,
                    });
                }

                if let Some(m) = model {
                    if !m.starts_with('<') {
                        models.insert(m.clone());
                    }
                }

                let blocks = convert_content_blocks(content);
                if let Some(ref mut turn) = current_turn {
                    turn.assistant_content.extend(blocks);
                    turn.timestamp_end = event.timestamp.clone();
                    turn.model = model.clone();
                }
            }

            EventKind::ToolResult {
                tool_use_id,
                content: result_content,
                ..
            } => {
                // Store for Task matching.
                if !tool_use_id.is_empty() {
                    if let Some(text) = result_content {
                        tool_result_contents.insert(tool_use_id.clone(), text.clone());
                        if let Some((agent_id, stats)) =
                            extract_subagent_tool_result_stats(text)
                        {
                            subagent_stats_by_agent_id.insert(agent_id, stats);
                        }
                    }
                }
            }

            EventKind::SubagentProgress {
                agent_id,
                prompt,
                message,
                timestamp: progress_timestamp,
                total_tokens,
                total_duration_ms,
                total_tool_use_count,
                ..
            } => {
                let entry =
                    subagent_deltas_map
                        .entry(agent_id.clone())
                        .or_insert_with(|| SubagentDelta {
                            agent_id: agent_id.clone(),
                            prompt: prompt.clone(),
                            files_read: Vec::new(),
                            files_written: Vec::new(),
                            commands: Vec::new(),
                            tool_counts: HashMap::new(),
                            timestamps: Vec::new(),
                            total_tokens: None,
                            total_duration_ms: None,
                            total_tool_use_count: None,
                        });

                if let Some(ts) = event
                    .timestamp
                    .as_deref()
                    .or(progress_timestamp.as_deref())
                {
                    entry.timestamps.push(ts.to_string());
                }

                // Update stats (take latest non-None).
                if total_tokens.is_some() {
                    entry.total_tokens = *total_tokens;
                }
                if total_duration_ms.is_some() {
                    entry.total_duration_ms = *total_duration_ms;
                }
                if total_tool_use_count.is_some() {
                    entry.total_tool_use_count = *total_tool_use_count;
                }

                // Extract tool usage from the progress message content.
                if let Some(msg) = message {
                    extract_subagent_tool_usage(entry, msg);
                }
            }

            // ToolUse events are already captured as content blocks within
            // AssistantMessage. Standalone ToolUse events don't need separate
            // handling for markdown rendering.
            EventKind::ToolUse(_)
            | EventKind::Usage { .. }
            | EventKind::SubagentCompletion { .. }
            | EventKind::StopSignal { .. } => {}
        }
    }

    if let Some(t) = current_turn.take() {
        turns.push(t);
    }

    // Apply tool-result-derived aggregate stats to matching agents.
    for (agent_id, stats) in subagent_stats_by_agent_id {
        if let Some(entry) = subagent_deltas_map.get_mut(&agent_id) {
            if stats.total_tokens.is_some() {
                entry.total_tokens = stats.total_tokens;
            }
            if stats.total_duration_ms.is_some() {
                entry.total_duration_ms = stats.total_duration_ms;
            }
            if stats.total_tool_use_count.is_some() {
                entry.total_tool_use_count = stats.total_tool_use_count;
            }
        }
    }

    // Merge tool_result_contents into turn user_content for Task matching.
    // The rendering code looks up tool results by ID in user_content blocks.
    // We already captured ToolResult events above; now inject them into
    // the appropriate turn's user_content if not already present.
    // (For Claude, tool_results come inside UserMessage content blocks, which
    //  we already handled. For standalone ToolResult events, inject them.)
    for turn in &mut turns {
        // Check which tool_use_ids appear in assistant_content
        let tool_ids: Vec<String> = turn
            .assistant_content
            .iter()
            .filter_map(|b| {
                if let ContentBlock::ToolUse { id, .. } = b {
                    Some(id.clone())
                } else {
                    None
                }
            })
            .collect();

        for tid in tool_ids {
            // If there's a result for this tool but it's not in user_content, add it.
            let already_has = turn.user_content.iter().any(|b| {
                if let ContentBlock::ToolResult { tool_use_id, .. } = b {
                    tool_use_id == &tid
                } else {
                    false
                }
            });
            if !already_has {
                if let Some(content) = tool_result_contents.get(&tid) {
                    turn.user_content.push(ContentBlock::ToolResult {
                        tool_use_id: tid,
                        content: content.clone(),
                    });
                }
            }
        }
    }

    // Timestamps and duration.
    timestamps.sort();
    let ts_start = timestamps.first().cloned();
    let ts_end = timestamps.last().cloned();
    let duration_seconds = compute_duration(ts_start.as_deref(), ts_end.as_deref());

    // Subagent deltas sorted by first timestamp.
    let mut subagent_deltas: Vec<SubagentDelta> = subagent_deltas_map.into_values().collect();
    subagent_deltas.sort_by(|a, b| {
        let a_min = a.timestamps.iter().min().cloned().unwrap_or_default();
        let b_min = b.timestamps.iter().min().cloned().unwrap_or_default();
        a_min.cmp(&b_min)
    });

    let fallback_id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();

    SessionData {
        session_id: session_id.unwrap_or(fallback_id),
        summary,
        turns,
        timestamp_start: ts_start,
        timestamp_end: ts_end,
        duration_seconds,
        models_used: models.into_iter().collect(),
        subagent_deltas,
    }
}

/// Convert parser content blocks to renderer content blocks.
fn convert_content_blocks(blocks: &[ParserContentBlock]) -> Vec<ContentBlock> {
    blocks
        .iter()
        .map(|block| match block {
            ParserContentBlock::Text(text) => ContentBlock::Text {
                text: text.clone(),
            },
            ParserContentBlock::Thinking => ContentBlock::Thinking,
            ParserContentBlock::ToolUse(ToolUseEvent { id, name, input }) => {
                ContentBlock::ToolUse {
                    name: name.clone(),
                    input: input.clone(),
                    id: id.clone(),
                }
            }
            ParserContentBlock::ToolResult {
                tool_use_id,
                content,
            } => ContentBlock::ToolResult {
                tool_use_id: tool_use_id.clone(),
                content: content.clone(),
            },
        })
        .collect()
}

/// Extract tool usage from a subagent progress message.
fn extract_subagent_tool_usage(entry: &mut SubagentDelta, msg: &Value) {
    // Progress messages can have nested assistant message with tool_use blocks.
    if msg.get("type").and_then(Value::as_str) != Some("assistant") {
        return;
    }
    let inner_msg = msg.get("message").unwrap_or(msg);
    let Some(content) = inner_msg.get("content").and_then(Value::as_array) else {
        return;
    };

    for block in content {
        if block.get("type").and_then(Value::as_str) != Some("tool_use") {
            continue;
        }
        let tool_name = block
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let inp = block.get("input").cloned().unwrap_or(Value::Null);

        *entry.tool_counts.entry(tool_name.clone()).or_insert(0) += 1;

        match tool_name.as_str() {
            "Read" => {
                if let Some(path) = get_str(&inp, "file_path") {
                    if !entry.files_read.contains(&path.to_string()) {
                        entry.files_read.push(path.to_string());
                    }
                }
            }
            "Write" => {
                if let Some(path) = get_str(&inp, "file_path") {
                    let existing = entry.files_written.iter().any(|(p, _)| p == path);
                    if !existing {
                        entry
                            .files_written
                            .push((path.to_string(), "created".to_string()));
                    }
                }
            }
            "Edit" => {
                if let Some(path) = get_str(&inp, "file_path") {
                    let existing = entry.files_written.iter().any(|(p, _)| p == path);
                    if !existing {
                        entry
                            .files_written
                            .push((path.to_string(), "edited".to_string()));
                    }
                }
            }
            "Bash" => {
                if let Some(cmd) = get_str(&inp, "command") {
                    if !entry.commands.contains(&cmd.to_string()) {
                        entry.commands.push(cmd.to_string());
                    }
                }
            }
            _ => {}
        }
    }
}

/// Extract aggregate subagent stats from tool_result content.
///
/// This preserves legacy behavior where markdown parsing also consumed
/// `toolUseResult` totals in addition to progress records.
fn extract_subagent_tool_result_stats(
    content: &str,
) -> Option<(String, SubagentToolResultStats)> {
    let parsed_json = serde_json::from_str::<Value>(content).ok();

    let agent_id = parsed_json
        .as_ref()
        .and_then(|value| {
            value
                .get("agentId")
                .or_else(|| value.get("agent_id"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .or_else(|| extract_agent_id_from_result(content))
        .or_else(|| extract_string_field(content, &["agentId", "agent_id"]))?;

    let mut stats = SubagentToolResultStats::default();
    if let Some(json) = parsed_json.as_ref() {
        stats.total_tokens = extract_i64_from_value(
            json.get("totalTokens")
                .or_else(|| json.get("total_tokens")),
        );
        stats.total_duration_ms = extract_i64_from_value(
            json.get("totalDurationMs")
                .or_else(|| json.get("total_duration_ms")),
        );
        stats.total_tool_use_count = extract_i64_from_value(
            json.get("totalToolUseCount")
                .or_else(|| json.get("total_tool_use_count")),
        );
    }

    if stats.total_tokens.is_none() {
        stats.total_tokens = extract_i64_field(content, &["totalTokens", "total_tokens"]);
    }
    if stats.total_duration_ms.is_none() {
        stats.total_duration_ms =
            extract_i64_field(content, &["totalDurationMs", "total_duration_ms"]);
    }
    if stats.total_tool_use_count.is_none() {
        stats.total_tool_use_count =
            extract_i64_field(content, &["totalToolUseCount", "total_tool_use_count"]);
    }

    Some((agent_id, stats))
}

fn extract_i64_from_value(value: Option<&Value>) -> Option<i64> {
    value.and_then(|v| {
        v.as_i64()
            .or_else(|| v.as_u64().and_then(|raw| i64::try_from(raw).ok()))
            .or_else(|| v.as_str().and_then(|raw| raw.parse::<i64>().ok()))
    })
}

fn extract_string_field(content: &str, keys: &[&str]) -> Option<String> {
    for key in keys {
        let pattern = format!(
            r#"(?i)(?:"{}"|{})\s*[:=]\s*"([^"]+)""#,
            regex::escape(key),
            regex::escape(key)
        );
        let Ok(re) = Regex::new(&pattern) else {
            continue;
        };
        if let Some(caps) = re.captures(content) {
            if let Some(m) = caps.get(1) {
                return Some(m.as_str().to_string());
            }
        }
    }
    None
}

fn extract_i64_field(content: &str, keys: &[&str]) -> Option<i64> {
    for key in keys {
        let pattern = format!(
            r#"(?i)(?:"{}"|{})\s*[:=]\s*"?(?P<num>\d+)"?"#,
            regex::escape(key),
            regex::escape(key)
        );
        let Ok(re) = Regex::new(&pattern) else {
            continue;
        };
        if let Some(caps) = re.captures(content) {
            if let Some(num) = caps.name("num") {
                if let Ok(value) = num.as_str().parse::<i64>() {
                    return Some(value);
                }
            }
        }
    }
    None
}


/// Compute duration in seconds between two timestamps.
fn compute_duration(start: Option<&str>, end: Option<&str>) -> Option<f64> {
    let s = parse_ts(start)?;
    let e = parse_ts(end)?;
    let diff = e.signed_duration_since(s);
    Some(diff.num_seconds() as f64)
}

// ---------------------------------------------------------------------------
// Subagent delta extraction
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Markdown rendering
// ---------------------------------------------------------------------------

/// Convert a SessionData to a markdown string.
fn session_to_markdown(session: &SessionData) -> String {
    let mut parts = Vec::new();

    // Frontmatter.
    parts.push(render_frontmatter(session));
    parts.push(String::new());

    // Title.
    let summary = session
        .summary
        .clone()
        .or_else(|| get_first_user_prompt(&session.turns))
        .unwrap_or_else(|| "Untitled".to_string());
    let title = if summary.len() > 80 {
        let truncated: String = summary.chars().take(77).collect();
        format!("{truncated}...")
    } else {
        summary
    };
    parts.push(format!("# Session: {title}"));
    parts.push(String::new());

    // Executive Summary.
    let exec = render_executive_summary(&session.turns, &session.subagent_deltas);
    if !exec.is_empty() {
        parts.push(exec);
    }

    // Conversation.
    parts.push(render_conversation(
        &session.turns,
        &session.subagent_deltas,
    ));

    // Open Threads.
    let threads = render_open_threads(&session.turns);
    if !threads.is_empty() {
        parts.push(threads);
    }

    // Subagent Activity.
    let subagent_section = render_subagent_activity(&session.subagent_deltas, &session.turns);
    if !subagent_section.is_empty() {
        parts.push(subagent_section);
    }

    parts.join("\n")
}

/// Render YAML frontmatter.
fn render_frontmatter(session: &SessionData) -> String {
    let sid: String = session.session_id.chars().take(8).collect();
    let ts_start = parse_ts(session.timestamp_start.as_deref());
    let ts_end = parse_ts(session.timestamp_end.as_deref());

    let date_str = ts_start
        .map(|dt| dt.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let start_str = ts_start
        .map(|dt| dt.format("%H:%M").to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let end_str = ts_end
        .map(|dt| dt.format("%H:%M").to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let duration_str = fmt_duration(session.duration_seconds);
    let model_str = session
        .models_used
        .first()
        .map(|m| fmt_model(m))
        .unwrap_or_else(|| "unknown".to_string());

    format!(
        "---\nsession_id: {sid}\ndate: {date_str}\nstart: {start_str}\nend: {end_str}\nduration: {duration_str}\nmodel: {model_str}\nturns: {}\n---",
        session.turns.len()
    )
}

/// Extract agentId from tool result content string.
fn extract_agent_id_from_result(content: &str) -> Option<String> {
    if !content.contains("agentId:") {
        return None;
    }
    let re = Regex::new(r"agentId: (\w+)").ok()?;
    re.captures(content).map(|c| c[1].to_string())
}

/// Look up a subagent delta for a given task tool_id.
fn lookup_delta_for_task<'a>(
    tool_id: &str,
    all_tool_results: &HashMap<String, String>,
    deltas_by_agent_id: &'a HashMap<String, &'a SubagentDelta>,
) -> Option<&'a SubagentDelta> {
    let content = all_tool_results.get(tool_id)?;
    let agent_id = extract_agent_id_from_result(content)?;
    deltas_by_agent_id.get(&agent_id).copied()
}

/// Flush accumulated tool-only annotations into output lines.
fn flush_pending_tools(
    lines: &mut Vec<String>,
    pending_tools: &mut Vec<ToolAnnotation>,
    pending_time: &mut Option<String>,
    all_tool_results: &HashMap<String, String>,
    deltas_by_agent_id: &HashMap<String, &SubagentDelta>,
) {
    if pending_tools.is_empty() {
        return;
    }
    let time = pending_time.as_deref().unwrap_or("??:??");
    lines.push(format!("### [{time}] Claude"));
    for ann in pending_tools.drain(..) {
        match ann {
            ToolAnnotation::Simple(s) => lines.push(s),
            ToolAnnotation::Task(task) => {
                let result = all_tool_results.get(&task.tool_id).map(String::as_str);
                let delta =
                    lookup_delta_for_task(&task.tool_id, all_tool_results, deltas_by_agent_id);
                lines.push(fmt_task_block(&task, result, delta));
            }
        }
    }
    lines.push(String::new());
    *pending_time = None;
}

/// Render tool annotations inline.
fn render_tool_annotations_inline(
    lines: &mut Vec<String>,
    annotations: &[ToolAnnotation],
    all_tool_results: &HashMap<String, String>,
    deltas_by_agent_id: &HashMap<String, &SubagentDelta>,
) {
    for ann in annotations {
        match ann {
            ToolAnnotation::Simple(s) => lines.push(s.clone()),
            ToolAnnotation::Task(task) => {
                let result = all_tool_results.get(&task.tool_id).map(String::as_str);
                let delta =
                    lookup_delta_for_task(&task.tool_id, all_tool_results, deltas_by_agent_id);
                lines.push(fmt_task_block(task, result, delta));
            }
        }
    }
    lines.push(String::new());
}

/// Render the conversation section.
fn render_conversation(turns: &[Turn], subagent_deltas: &[SubagentDelta]) -> String {
    let mut lines = vec!["## Conversation".to_string(), String::new()];

    // Build index of tool_results across all turns for Task matching.
    let mut all_tool_results: HashMap<String, String> = HashMap::new();
    for turn in turns {
        let results = extract_tool_results(&turn.user_content);
        all_tool_results.extend(results);
    }

    // Build index of subagent deltas by agentId.
    let mut deltas_by_agent_id: HashMap<String, &SubagentDelta> = HashMap::new();
    for delta in subagent_deltas {
        deltas_by_agent_id.insert(delta.agent_id.clone(), delta);
    }

    // Accumulator for merging tool-only Claude turns.
    let mut pending_tools: Vec<ToolAnnotation> = Vec::new();
    let mut pending_time: Option<String> = None;

    for turn in turns {
        let ts = turn.timestamp_start.as_deref();
        let time_str = fmt_time(ts);

        // User message.
        let user_text = extract_text_from_blocks(&turn.user_content);
        if !user_text.is_empty() {
            flush_pending_tools(
                &mut lines,
                &mut pending_tools,
                &mut pending_time,
                &all_tool_results,
                &deltas_by_agent_id,
            );
            let filtered = filter_skill_injection(&user_text);
            lines.push(format!("### [{time_str}] User"));
            lines.push(truncate_human(&filtered));
            lines.push(String::new());
        }

        // Claude response.
        let claude_text = extract_text_from_blocks(&turn.assistant_content);
        let tool_annotations = extract_tool_annotations(&turn.assistant_content);

        if !claude_text.is_empty() || !tool_annotations.is_empty() {
            let ts_end = turn.timestamp_end.as_deref().or(ts);
            let time_str_end = fmt_time(ts_end);

            if !claude_text.is_empty() {
                // Has text -- flush pending and render normally.
                flush_pending_tools(
                    &mut lines,
                    &mut pending_tools,
                    &mut pending_time,
                    &all_tool_results,
                    &deltas_by_agent_id,
                );
                lines.push(format!("### [{time_str_end}] Claude"));
                lines.push(truncate_claude(&claude_text));
                lines.push(String::new());

                if !tool_annotations.is_empty() {
                    render_tool_annotations_inline(
                        &mut lines,
                        &tool_annotations,
                        &all_tool_results,
                        &deltas_by_agent_id,
                    );
                }
            } else if !tool_annotations.is_empty() {
                // Tool-only turn -- accumulate for merging.
                if pending_tools.is_empty() {
                    pending_time = Some(time_str_end);
                }
                pending_tools.extend(tool_annotations);
            }
        }
    }

    // Flush remaining.
    flush_pending_tools(
        &mut lines,
        &mut pending_tools,
        &mut pending_time,
        &all_tool_results,
        &deltas_by_agent_id,
    );

    lines.join("\n")
}

/// Render the Executive Summary section.
fn render_executive_summary(turns: &[Turn], subagent_deltas: &[SubagentDelta]) -> String {
    let mut lines = vec!["## Executive Summary".to_string(), String::new()];

    // Files Touched (Main Session).
    let (reads, writes) = collect_files(turns);
    if !reads.is_empty() || !writes.is_empty() {
        lines.push("### Files Touched (Main Session)".to_string());
        lines.push(String::new());

        if !reads.is_empty() {
            lines.push(format!("**Read ({}):**", reads.len()));
            for path in reads.iter().take(20) {
                lines.push(format!("- `{path}`"));
            }
            if reads.len() > 20 {
                lines.push(format!("- ... and {} more", reads.len() - 20));
            }
            lines.push(String::new());
        }

        if !writes.is_empty() {
            lines.push(format!("**Written ({}):**", writes.len()));
            for (path, action) in writes.iter().take(20) {
                lines.push(format!("- `{path}` ({action})"));
            }
            if writes.len() > 20 {
                lines.push(format!("- ... and {} more", writes.len() - 20));
            }
            lines.push(String::new());
        }
    }

    // Files Touched by Subagents.
    let mut subagent_reads: Vec<(&str, &str)> = Vec::new();
    let mut subagent_writes: Vec<(&str, &str)> = Vec::new();
    for agent in subagent_deltas {
        let short_id: &str = if agent.agent_id.len() > 7 {
            &agent.agent_id[..7]
        } else {
            &agent.agent_id
        };
        for path in &agent.files_read {
            subagent_reads.push((path.as_str(), short_id));
        }
        for (path, _) in &agent.files_written {
            subagent_writes.push((path.as_str(), short_id));
        }
    }

    if !subagent_reads.is_empty() || !subagent_writes.is_empty() {
        lines.push("### Files Touched by Subagents".to_string());
        lines.push(String::new());

        if !subagent_writes.is_empty() {
            lines.push(format!("**Written ({}):**", subagent_writes.len()));
            for (path, agent_id) in subagent_writes.iter().take(20) {
                lines.push(format!("- `{path}` (by {agent_id})"));
            }
            if subagent_writes.len() > 20 {
                lines.push(format!("- ... and {} more", subagent_writes.len() - 20));
            }
            lines.push(String::new());
        }

        if !subagent_reads.is_empty() {
            lines.push(format!("**Read ({}):**", subagent_reads.len()));
            for (path, agent_id) in subagent_reads.iter().take(20) {
                lines.push(format!("- `{path}` (by {agent_id})"));
            }
            if subagent_reads.len() > 20 {
                lines.push(format!("- ... and {} more", subagent_reads.len() - 20));
            }
            lines.push(String::new());
        }
    }

    // Commands Executed.
    let commands = collect_commands(turns);
    if !commands.is_empty() {
        lines.push("### Commands Executed".to_string());
        lines.push(String::new());
        for cmd in commands.iter().take(15) {
            let display = if cmd.len() > 120 {
                let truncated: String = cmd.chars().take(117).collect();
                format!("{truncated}...")
            } else {
                cmd.clone()
            };
            lines.push(format!("- `{display}`"));
        }
        if commands.len() > 15 {
            lines.push(format!("- ... and {} more", commands.len() - 15));
        }
        lines.push(String::new());
    }

    // Subagents table.
    if !subagent_deltas.is_empty() {
        let task_models = collect_subagents(turns);

        lines.push(format!("### Subagents ({})", subagent_deltas.len()));
        lines.push(String::new());
        lines.push(
            "| Agent | Task | Model | Duration | Tokens | Files Written | Commands |".to_string(),
        );
        lines.push(
            "|-------|------|-------|----------|--------|---------------|----------|".to_string(),
        );

        for (i, agent) in subagent_deltas.iter().enumerate() {
            let short_id: &str = if agent.agent_id.len() > 7 {
                &agent.agent_id[..7]
            } else {
                &agent.agent_id
            };

            let prompt_raw = agent.prompt.replace('\n', " ");
            let prompt_short = if prompt_raw.len() > 40 {
                let truncated: String = prompt_raw.chars().take(40).collect();
                format!("{truncated}...")
            } else {
                prompt_raw
            };
            let prompt_escaped = prompt_short.replace('|', "/");

            let model = if i < task_models.len() {
                fmt_model(&task_models[i].model)
            } else {
                "-".to_string()
            };

            let duration_str = match agent.total_duration_ms {
                Some(ms) => {
                    let secs = ms / 1000;
                    let mins = secs / 60;
                    let rem = secs % 60;
                    if mins > 0 {
                        format!("{mins}m {rem}s")
                    } else {
                        format!("{secs}s")
                    }
                }
                None => "-".to_string(),
            };

            let tokens_str = match agent.total_tokens {
                Some(t) if t >= 1000 => format!("{}k", t / 1000),
                Some(t) => t.to_string(),
                None => "-".to_string(),
            };

            let files_str = if agent.files_written.is_empty() {
                "-".to_string()
            } else {
                agent.files_written.len().to_string()
            };

            let commands_str = if agent.commands.is_empty() {
                "-".to_string()
            } else {
                agent.commands.len().to_string()
            };

            lines.push(format!(
                "| {short_id} | {prompt_escaped} | {model} | {duration_str} | {tokens_str} | {files_str} | {commands_str} |"
            ));
        }
        lines.push(String::new());
    }

    lines.push("---".to_string());
    lines.push(String::new());

    lines.join("\n")
}

/// Render the Open Threads section.
fn render_open_threads(turns: &[Turn]) -> String {
    let threads = extract_open_threads(turns);
    if threads.is_empty() {
        return String::new();
    }

    let mut lines = vec![
        "---".to_string(),
        String::new(),
        "## Open Threads".to_string(),
    ];
    for thread in &threads {
        lines.push(format!("- {thread}"));
    }
    lines.push(String::new());
    lines.join("\n")
}

/// Render detailed Subagent Activity section at end of document.
fn render_subagent_activity(subagent_deltas: &[SubagentDelta], turns: &[Turn]) -> String {
    if subagent_deltas.is_empty() {
        return String::new();
    }

    let _task_models = collect_subagents(turns);

    let mut lines = vec![
        "---".to_string(),
        String::new(),
        "## Subagent Activity".to_string(),
        String::new(),
    ];

    for agent in subagent_deltas {
        let short_id: &str = if agent.agent_id.len() > 7 {
            &agent.agent_id[..7]
        } else {
            &agent.agent_id
        };

        let prompt_raw = agent.prompt.replace('\n', " ");
        let prompt_short = if prompt_raw.len() > 60 {
            let truncated: String = prompt_raw.chars().take(60).collect();
            format!("{truncated}...")
        } else {
            prompt_raw
        };

        lines.push(format!("### Agent {short_id} ({prompt_short})"));

        // Duration and tokens.
        let mut stats_parts = Vec::new();
        if let Some(ms) = agent.total_duration_ms {
            let secs = ms / 1000;
            let mins = secs / 60;
            let rem = secs % 60;
            if mins > 0 {
                stats_parts.push(format!("**Duration:** {mins}m {rem}s"));
            } else {
                stats_parts.push(format!("**Duration:** {secs}s"));
            }
        }
        if let Some(tokens) = agent.total_tokens {
            stats_parts.push(format!("**Tokens:** {}", format_with_commas(tokens)));
        }
        if !stats_parts.is_empty() {
            lines.push(format!("- {}", stats_parts.join(" | ")));
        }

        // Files written.
        if !agent.files_written.is_empty() {
            let filenames: Vec<String> = agent
                .files_written
                .iter()
                .map(|(path, _)| format!("`{path}`"))
                .collect();
            lines.push(format!("- **Files written:** {}", filenames.join(", ")));
        } else {
            lines.push("- **Files written:** (none)".to_string());
        }

        // Files read.
        if !agent.files_read.is_empty() {
            let filenames: Vec<String> = agent
                .files_read
                .iter()
                .take(5)
                .map(|p| format!("`{p}`"))
                .collect();
            let suffix = if agent.files_read.len() > 5 {
                format!(" (+{} more)", agent.files_read.len() - 5)
            } else {
                String::new()
            };
            lines.push(format!(
                "- **Files read:** {}{}",
                filenames.join(", "),
                suffix
            ));
        } else {
            lines.push("- **Files read:** (none)".to_string());
        }

        // Commands.
        if !agent.commands.is_empty() {
            let cmd = &agent.commands[0];
            let cmd_short = if cmd.len() > 60 {
                let truncated: String = cmd.chars().take(57).collect();
                format!("{truncated}...")
            } else {
                cmd.clone()
            };
            let suffix = if agent.commands.len() > 1 {
                format!(" (+{} more)", agent.commands.len() - 1)
            } else {
                String::new()
            };
            lines.push(format!("- **Commands:** `{cmd_short}`{suffix}"));
        } else {
            lines.push("- **Commands:** (none)".to_string());
        }

        // Tool call breakdown.
        if !agent.tool_counts.is_empty() {
            let mut sorted: Vec<(&String, &usize)> = agent.tool_counts.iter().collect();
            sorted.sort_by(|a, b| b.1.cmp(a.1));
            let parts: Vec<String> = sorted
                .iter()
                .map(|(name, count)| format!("{count} {name}"))
                .collect();
            lines.push(format!("- **Tool calls:** {}", parts.join(", ")));
        }

        lines.push(String::new());
    }

    lines.join("\n")
}

/// Extract first user prompt text for fallback title.
fn get_first_user_prompt(turns: &[Turn]) -> Option<String> {
    for turn in turns {
        let text = extract_text_from_blocks(&turn.user_content);
        if !text.is_empty() {
            let clean = text.trim().replace('\n', " ");
            let truncated: String = if clean.len() > 50 {
                let prefix: String = clean.chars().take(47).collect();
                format!("{prefix}...")
            } else {
                clean
            };
            return Some(truncated);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_human_short() {
        let text = "Hello world";
        assert_eq!(truncate_human(text), "Hello world");
    }

    #[test]
    fn test_truncate_human_long() {
        let text = "a".repeat(200_000);
        let result = truncate_human(&text);
        assert!(result.contains("[... truncated from 200000 chars]"));
        assert!(result.len() < 10_000);
    }

    #[test]
    fn test_truncate_claude_short() {
        let text = "Response text";
        assert_eq!(truncate_claude(text), "Response text");
    }

    #[test]
    fn test_truncate_claude_long() {
        let text = "b".repeat(200_000);
        let result = truncate_claude(&text);
        assert!(result.contains("[... continued, 200000 chars total]"));
    }

    #[test]
    fn test_filter_skill_injection() {
        let text = "Base directory for this skill: /path/to/skill\nMore content here";
        let result = filter_skill_injection(text);
        assert_eq!(result, "[Skill loaded: `/path/to/skill`]");
    }

    #[test]
    fn test_filter_skill_injection_normal_text() {
        let text = "Just a normal message";
        assert_eq!(filter_skill_injection(text), "Just a normal message");
    }

    #[test]
    fn test_fmt_time_valid() {
        let result = fmt_time(Some("2026-03-07T10:30:00Z"));
        assert_eq!(result, "14:30"); // UTC+4
    }

    #[test]
    fn test_fmt_time_invalid() {
        assert_eq!(fmt_time(None), "??:??");
        assert_eq!(fmt_time(Some("garbage")), "??:??");
    }

    #[test]
    fn test_fmt_duration() {
        assert_eq!(fmt_duration(Some(3700.0)), "1h 1m");
        assert_eq!(fmt_duration(Some(300.0)), "5m");
        assert_eq!(fmt_duration(None), "unknown");
    }

    #[test]
    fn test_fmt_model() {
        assert_eq!(fmt_model("claude-opus-4-20250514"), "Opus");
        assert_eq!(fmt_model("claude-sonnet-4-20250514"), "Sonnet");
        assert_eq!(fmt_model("claude-3-haiku-20240307"), "Haiku");
        assert_eq!(fmt_model("gpt-4o"), "gpt");
        assert_eq!(fmt_model(""), "unknown");
    }

    #[test]
    fn test_render_frontmatter() {
        let session = SessionData {
            session_id: "abcdef1234567890".to_string(),
            summary: None,
            turns: vec![],
            timestamp_start: Some("2026-03-07T10:00:00Z".to_string()),
            timestamp_end: Some("2026-03-07T11:30:00Z".to_string()),
            duration_seconds: Some(5400.0),
            models_used: vec!["claude-opus-4-20250514".to_string()],
            subagent_deltas: vec![],
        };
        let fm = render_frontmatter(&session);
        assert!(fm.contains("session_id: abcdef12"));
        assert!(fm.contains("date: 2026-03-07"));
        assert!(fm.contains("start: 14:00")); // UTC+4
        assert!(fm.contains("end: 15:30"));
        assert!(fm.contains("duration: 1h 30m"));
        assert!(fm.contains("model: Opus"));
        assert!(fm.contains("turns: 0"));
    }

    #[test]
    fn test_tool_annotation_read() {
        let input = serde_json::json!({"file_path": "/src/main.rs"});
        let ann = fmt_tool_annotation("Read", &input, "id1");
        match ann {
            Some(ToolAnnotation::Simple(s)) => assert_eq!(s, "-> Read: `/src/main.rs`"),
            _ => panic!("expected Simple annotation"),
        }
    }

    #[test]
    fn test_tool_annotation_bash_truncate() {
        let long_cmd = "a".repeat(100);
        let input = serde_json::json!({"command": long_cmd});
        let ann = fmt_tool_annotation("Bash", &input, "id2");
        match ann {
            Some(ToolAnnotation::Simple(s)) => {
                assert!(s.contains("..."));
                assert!(s.len() < 80);
            }
            _ => panic!("expected Simple annotation"),
        }
    }

    #[test]
    fn test_interruption_detection() {
        use crate::parser::event::{
            ContentBlock as PB, EventKind as EK, SessionEvent as SE,
        };
        // An interruption user message should be skipped during conversion.
        let events = vec![SE {
            timestamp: Some("2026-03-07T10:00:00Z".to_string()),
            kind: EK::UserMessage {
                content: vec![PB::Text("[Request interrupted by user]".to_string())],
            },
        }];
        let session = events_to_session_data(&events, Path::new("test.jsonl"));
        assert!(session.turns.is_empty(), "interruption turn should be skipped");

        // A normal user message should produce a turn.
        let events2 = vec![SE {
            timestamp: Some("2026-03-07T10:00:00Z".to_string()),
            kind: EK::UserMessage {
                content: vec![PB::Text("Hello".to_string())],
            },
        }];
        let session2 = events_to_session_data(&events2, Path::new("test.jsonl"));
        assert_eq!(session2.turns.len(), 1, "normal message should produce a turn");
    }

    #[test]
    fn test_extract_open_threads() {
        let turn = Turn {
            turn_number: 1,
            user_content: vec![],
            assistant_content: vec![ContentBlock::Text {
                text: "- TODO: fix the build\n- FIXME: handle edge case".to_string(),
            }],
            timestamp_start: None,
            timestamp_end: None,
            model: None,
        };
        let threads = extract_open_threads(&[turn]);
        assert_eq!(threads.len(), 2);
        assert!(threads[0].contains("fix the build"));
    }

    #[test]
    fn test_subagent_stats_merged_from_tool_result_payload() {
        use crate::parser::event::{
            ContentBlock as PB, EventKind as EK, SessionEvent as SE,
        };

        let events = vec![
            SE {
                timestamp: Some("2026-03-07T10:00:00Z".to_string()),
                kind: EK::SubagentProgress {
                    agent_id: "agent123".to_string(),
                    prompt: "Investigate parser output".to_string(),
                    message: None,
                    timestamp: Some("2026-03-07T10:00:00Z".to_string()),
                    total_tokens: None,
                    total_duration_ms: None,
                    total_tool_use_count: None,
                },
            },
            SE {
                timestamp: Some("2026-03-07T10:01:00Z".to_string()),
                kind: EK::UserMessage {
                    content: vec![
                        PB::Text("tool result incoming".to_string()),
                        PB::ToolResult {
                            tool_use_id: "task_1".to_string(),
                            content: r#"{"agentId":"agent123","totalTokens":1200,"totalDurationMs":3000,"totalToolUseCount":4}"#.to_string(),
                        },
                    ],
                },
            },
        ];

        let session = events_to_session_data(&events, Path::new("test.jsonl"));
        assert_eq!(session.subagent_deltas.len(), 1);
        let delta = &session.subagent_deltas[0];
        assert_eq!(delta.agent_id, "agent123");
        assert_eq!(delta.total_tokens, Some(1200));
        assert_eq!(delta.total_duration_ms, Some(3000));
        assert_eq!(delta.total_tool_use_count, Some(4));
    }
}
