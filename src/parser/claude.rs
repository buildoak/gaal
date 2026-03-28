use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::Path;

use anyhow::{Context, Result};
use serde_json::Value;

use super::common::as_i64;
use super::event::{ContentBlock, EventKind, SessionEvent, ToolUseEvent};

/// Parses a full Claude JSONL session file into canonical events.
pub fn parse_events(path: &Path) -> Result<Vec<SessionEvent>> {
    parse_events_from_offset(path, 0)
}

/// Parses Claude JSONL events starting from a byte offset.
pub fn parse_events_from_offset(path: &Path, offset: u64) -> Result<Vec<SessionEvent>> {
    let file = File::open(path)
        .with_context(|| format!("failed to open Claude session file: {}", path.display()))?;
    let mut reader = BufReader::new(file);
    if offset > 0 {
        reader
            .seek(SeekFrom::Start(offset))
            .with_context(|| format!("failed to seek Claude session file: {}", path.display()))?;
    }

    let mut events: Vec<SessionEvent> = Vec::new();
    let mut emitted_meta = false;

    for line_result in reader.lines() {
        let line = line_result.context("failed to read Claude JSONL line")?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let record: Value = match serde_json::from_str(trimmed) {
            Ok(value) => value,
            Err(_) => continue,
        };

        let ts = record
            .get("timestamp")
            .and_then(Value::as_str)
            .map(str::to_string);

        if !emitted_meta {
            let session_id = record
                .get("sessionId")
                .and_then(Value::as_str)
                .map(str::to_string);
            let cwd = record
                .get("cwd")
                .and_then(Value::as_str)
                .map(str::to_string);
            let version = record
                .get("version")
                .and_then(Value::as_str)
                .map(str::to_string);
            let model = record
                .get("model")
                .and_then(Value::as_str)
                .or_else(|| record.pointer("/message/model").and_then(Value::as_str))
                .and_then(filter_model);
            if session_id.is_some() || cwd.is_some() || version.is_some() || model.is_some() {
                events.push(SessionEvent {
                    timestamp: ts.clone(),
                    kind: EventKind::Meta {
                        session_id,
                        model,
                        cwd,
                        version,
                    },
                });
                emitted_meta = true;
            }
        }

        if let Some(usage) = extract_claude_usage_event(&record) {
            events.push(SessionEvent {
                timestamp: ts.clone(),
                kind: usage,
            });
        }

        if let Some(stop_reason) = record
            .pointer("/message/stop_reason")
            .and_then(Value::as_str)
        {
            events.push(SessionEvent {
                timestamp: ts.clone(),
                kind: EventKind::StopSignal {
                    reason: stop_reason.to_string(),
                },
            });
        }

        let record_type = record
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();

        match record_type {
            "user" => {
                let content = record.pointer("/message/content").unwrap_or(&Value::Null);
                let blocks = extract_claude_user_blocks(content);
                events.push(SessionEvent {
                    timestamp: ts.clone(),
                    kind: EventKind::UserMessage { content: blocks },
                });

                for result in extract_claude_tool_result_events(content) {
                    events.push(SessionEvent {
                        timestamp: ts.clone(),
                        kind: result,
                    });
                }
            }
            "assistant" => {
                let content = record.pointer("/message/content").unwrap_or(&Value::Null);
                let blocks = extract_claude_assistant_blocks(content);
                let model = record
                    .pointer("/message/model")
                    .and_then(Value::as_str)
                    .and_then(filter_model);
                let stop_reason = record
                    .pointer("/message/stop_reason")
                    .and_then(Value::as_str)
                    .map(str::to_string);

                events.push(SessionEvent {
                    timestamp: ts.clone(),
                    kind: EventKind::AssistantMessage {
                        content: blocks,
                        model,
                        stop_reason,
                    },
                });
            }
            "progress" => {
                if let Some(progress) = extract_agent_progress_event(&record) {
                    events.push(SessionEvent {
                        timestamp: ts.clone(),
                        kind: progress,
                    });
                }
            }
            "summary" => {
                if let Some(text) = record.get("summary").and_then(Value::as_str) {
                    events.push(SessionEvent {
                        timestamp: ts.clone(),
                        kind: EventKind::Summary {
                            text: text.to_string(),
                        },
                    });
                }
            }
            _ => {}
        }
    }

    Ok(events)
}

fn filter_model(model: &str) -> Option<String> {
    (!model.is_empty() && !model.starts_with('<')).then(|| model.to_string())
}

fn extract_claude_usage_event(record: &Value) -> Option<EventKind> {
    let usage = record.pointer("/message/usage")?;
    if usage.is_null() {
        return None;
    }
    let input_tokens = as_i64(usage.get("input_tokens"));
    let cache_creation_input_tokens = as_i64(usage.get("cache_creation_input_tokens"));
    let cache_read_input_tokens = as_i64(usage.get("cache_read_input_tokens"));
    Some(EventKind::Usage {
        input_tokens: input_tokens + cache_creation_input_tokens + cache_read_input_tokens,
        output_tokens: as_i64(record.pointer("/message/usage/output_tokens")),
        cache_read_input_tokens,
        cache_creation_input_tokens,
        dedup_key: record
            .pointer("/message/id")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

fn extract_claude_user_blocks(content: &Value) -> Vec<ContentBlock> {
    if let Some(text) = content.as_str() {
        return vec![ContentBlock::Text(text.to_string())];
    }

    let Some(items) = content.as_array() else {
        return Vec::new();
    };

    let mut blocks = Vec::new();
    for item in items {
        if let Some(text) = item.as_str() {
            blocks.push(ContentBlock::Text(text.to_string()));
            continue;
        }

        let block_type = item.get("type").and_then(Value::as_str).unwrap_or_default();
        match block_type {
            "text" => {
                let text = item
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                blocks.push(ContentBlock::Text(text));
            }
            "tool_result" => {
                let tool_use_id = item
                    .get("tool_use_id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let content = tool_result_block_content(item.get("content"));
                blocks.push(ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                });
            }
            _ => {}
        }
    }

    blocks
}

fn extract_claude_assistant_blocks(content: &Value) -> Vec<ContentBlock> {
    if let Some(text) = content.as_str() {
        return vec![ContentBlock::Text(text.to_string())];
    }

    let Some(items) = content.as_array() else {
        return Vec::new();
    };

    let mut blocks = Vec::new();
    for item in items {
        if let Some(text) = item.as_str() {
            blocks.push(ContentBlock::Text(text.to_string()));
            continue;
        }

        let block_type = item.get("type").and_then(Value::as_str).unwrap_or_default();
        match block_type {
            "text" => {
                let text = item
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                blocks.push(ContentBlock::Text(text));
            }
            "thinking" => {
                blocks.push(ContentBlock::Thinking);
            }
            "tool_use" => {
                let id = item
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let name = item
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let input = item
                    .get("input")
                    .cloned()
                    .unwrap_or(Value::Object(serde_json::Map::new()));
                blocks.push(ContentBlock::ToolUse(ToolUseEvent { id, name, input }));
            }
            _ => {}
        }
    }

    blocks
}

fn extract_claude_tool_result_events(content: &Value) -> Vec<EventKind> {
    let Some(items) = content.as_array() else {
        return Vec::new();
    };

    let mut events = Vec::new();
    for item in items {
        if item.get("type").and_then(Value::as_str) != Some("tool_result") {
            continue;
        }
        let tool_use_id = item
            .get("tool_use_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let content = tool_result_event_content(item.get("content"));
        let is_error = item
            .get("is_error")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        events.push(EventKind::ToolResult {
            tool_use_id,
            content,
            is_error,
        });
    }

    events
}

fn tool_result_block_content(content: Option<&Value>) -> String {
    match content {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| {
                (item.get("type").and_then(Value::as_str) == Some("text"))
                    .then(|| item.get("text").and_then(Value::as_str))
                    .flatten()
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Some(Value::String(text)) => text.clone(),
        Some(other) => other.to_string(),
        None => Value::Null.to_string(),
    }
}

fn tool_result_event_content(content: Option<&Value>) -> Option<String> {
    let value = content?;
    match value {
        Value::Null => None,
        Value::String(text) => Some(text.clone()),
        Value::Array(items) => {
            let joined = items
                .iter()
                .filter_map(|item| {
                    (item.get("type").and_then(Value::as_str) == Some("text"))
                        .then(|| item.get("text").and_then(Value::as_str))
                        .flatten()
                })
                .collect::<Vec<_>>()
                .join("\n");
            if joined.is_empty() {
                Some(value.to_string())
            } else {
                Some(joined)
            }
        }
        other => Some(other.to_string()),
    }
}

fn extract_agent_progress_event(record: &Value) -> Option<EventKind> {
    if record.get("type").and_then(Value::as_str) != Some("progress") {
        return None;
    }

    let data = record.get("data")?;
    if data.get("type").and_then(Value::as_str) != Some("agent_progress") {
        return None;
    }

    let agent_id = data
        .get("agentId")
        .or_else(|| data.get("agent_id"))
        .and_then(Value::as_str)?
        .to_string();
    let prompt = data
        .get("prompt")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let message = data.get("message").filter(|v| !v.is_null()).cloned();
    let timestamp = data
        .get("timestamp")
        .and_then(Value::as_str)
        .or_else(|| record.get("timestamp").and_then(Value::as_str))
        .map(str::to_string);
    let total_tokens = as_i64_opt(data.get("totalTokens").or_else(|| data.get("total_tokens")));
    let total_duration_ms = as_i64_opt(
        data.get("totalDurationMs")
            .or_else(|| data.get("total_duration_ms")),
    );
    let total_tool_use_count = as_i64_opt(
        data.get("totalToolUseCount")
            .or_else(|| data.get("total_tool_use_count")),
    );

    Some(EventKind::SubagentProgress {
        agent_id,
        prompt,
        message,
        timestamp,
        total_tokens,
        total_duration_ms,
        total_tool_use_count,
    })
}

fn as_i64_opt(value: Option<&Value>) -> Option<i64> {
    value.and_then(|v| {
        v.as_i64()
            .or_else(|| v.as_u64().and_then(|raw| i64::try_from(raw).ok()))
    })
}
