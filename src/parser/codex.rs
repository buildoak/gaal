use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use anyhow::{Context, Result};
use serde_json::Value;

use super::common::as_i64;
use super::event::{ContentBlock, EventKind, SessionEvent, ToolUseEvent};

/// Parses a full Codex JSONL session file into canonical events.
pub fn parse_events(path: &Path) -> Result<Vec<SessionEvent>> {
    parse_events_from_offset(path, 0)
}

/// Parses Codex JSONL events starting from a byte offset.
pub fn parse_events_from_offset(path: &Path, offset: u64) -> Result<Vec<SessionEvent>> {
    let file = File::open(path)
        .with_context(|| format!("failed to open Codex session file: {}", path.display()))?;
    let mut reader = BufReader::new(file);

    let mut events: Vec<SessionEvent> = Vec::new();
    let mut tool_meta_by_call_id: HashMap<String, (String, Value)> = HashMap::new();
    let mut line_offset = 0_u64;

    loop {
        let mut line = String::new();
        let bytes_read = reader
            .read_line(&mut line)
            .context("failed to read Codex JSONL line")?;
        if bytes_read == 0 {
            break;
        }

        let line_start = line_offset;
        line_offset = line_offset.saturating_add(bytes_read as u64);
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let record: Value = match serde_json::from_str(trimmed) {
            Ok(value) => value,
            Err(_) => continue,
        };

        let ts = codex_event_timestamp(&record);
        let record_type = record
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let should_emit = line_start >= offset;

        match record_type {
            "session_meta" => {
                if !should_emit {
                    continue;
                }
                let session_id = record
                    .pointer("/payload/id")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let cwd = record
                    .pointer("/payload/cwd")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let version = record
                    .pointer("/payload/cli_version")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let forked_from_id = record
                    .pointer("/payload/forked_from_id")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let agent_role = record
                    .pointer("/payload/agent_role")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let agent_nickname = record
                    .pointer("/payload/agent_nickname")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                events.push(SessionEvent {
                    timestamp: ts.clone(),
                    kind: EventKind::Meta {
                        session_id,
                        model: None,
                        cwd,
                        version,
                        forked_from_id,
                        agent_role,
                        agent_nickname,
                    },
                });
            }
            "turn_context" => {
                if !should_emit {
                    continue;
                }
                let session_id = record
                    .pointer("/payload/id")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let cwd = record
                    .pointer("/payload/cwd")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let model = record
                    .pointer("/payload/model")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                events.push(SessionEvent {
                    timestamp: ts.clone(),
                    kind: EventKind::Meta {
                        session_id,
                        model,
                        cwd,
                        version: None,
                        forked_from_id: None,
                        agent_role: None,
                        agent_nickname: None,
                    },
                });
            }
            "event_msg" => {
                if !should_emit {
                    continue;
                }
                let event_type = record
                    .pointer("/payload/type")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                match event_type {
                    "user_message" => {
                        let content = event_msg_text_block(record.pointer("/payload/message"));
                        events.push(SessionEvent {
                            timestamp: ts.clone(),
                            kind: EventKind::UserMessage { content },
                        });
                    }
                    "agent_message" => {
                        let assistant_text = extract_assistant_text(&record, event_type);
                        let content = assistant_text
                            .map(|text| vec![ContentBlock::Text(text)])
                            .unwrap_or_default();
                        let model = record
                            .pointer("/payload/model")
                            .and_then(Value::as_str)
                            .map(str::to_string);

                        events.push(SessionEvent {
                            timestamp: ts.clone(),
                            kind: EventKind::AssistantMessage {
                                content,
                                model,
                                stop_reason: None,
                            },
                        });
                    }
                    "task_complete" => {
                        let assistant_text = extract_assistant_text(&record, event_type);
                        let model = record
                            .pointer("/payload/model")
                            .and_then(Value::as_str)
                            .map(str::to_string);
                        let stop_reason = Some(extract_codex_stop_reason(&record));

                        // Deduplicate: task_complete's last_agent_message often
                        // repeats the preceding agent_message. Only emit an
                        // AssistantMessage if the text is new.
                        let is_duplicate = assistant_text.as_ref().map_or(true, |text| {
                            events.iter().rev().any(|ev| {
                                if let EventKind::AssistantMessage { content, .. } = &ev.kind {
                                    content.iter().any(|block| {
                                        if let ContentBlock::Text(prev) = block {
                                            prev == text
                                        } else {
                                            false
                                        }
                                    })
                                } else {
                                    false
                                }
                            })
                        });

                        if !is_duplicate {
                            let content = assistant_text
                                .map(|text| vec![ContentBlock::Text(text)])
                                .unwrap_or_default();
                            events.push(SessionEvent {
                                timestamp: ts.clone(),
                                kind: EventKind::AssistantMessage {
                                    content,
                                    model,
                                    stop_reason: stop_reason.clone(),
                                },
                            });
                        }

                        events.push(SessionEvent {
                            timestamp: ts.clone(),
                            kind: EventKind::StopSignal {
                                reason: stop_reason.unwrap_or_else(|| "task_complete".to_string()),
                            },
                        });
                    }
                    "token_count" => {
                        // Codex emits duplicate token_count events per turn
                        // (one per rate-limit bucket: "codex", "codex_bengalfox",
                        // etc.).  All duplicates carry identical token counts but
                        // different rate_limits metadata.  Without dedup the
                        // totals inflate 2-3x.
                        //
                        // payload.id is always null on token_count events, so we
                        // build a dedup key from total_token_usage.total_tokens
                        // which is the running cumulative total — identical
                        // across duplicates for the same turn, unique across
                        // different turns.
                        let dedup_key = record
                            .pointer("/payload/info/total_token_usage/total_tokens")
                            .and_then(|v| v.as_i64().or_else(|| v.as_u64().map(|u| u as i64)))
                            .map(|n| format!("codex_tc_{n}"));

                        let raw_input =
                            as_i64(record.pointer("/payload/info/last_token_usage/input_tokens"));
                        // Codex uses `cached_input_tokens` (input_tokens includes cached).
                        // Take max of both field names for robustness.
                        let cached = std::cmp::max(
                            as_i64(
                                record
                                    .pointer("/payload/info/last_token_usage/cached_input_tokens"),
                            ),
                            as_i64(
                                record.pointer(
                                    "/payload/info/last_token_usage/cache_read_input_tokens",
                                ),
                            ),
                        );
                        let reasoning = as_i64(
                            record
                                .pointer("/payload/info/last_token_usage/reasoning_output_tokens"),
                        );

                        events.push(SessionEvent {
                            timestamp: ts.clone(),
                            kind: EventKind::Usage {
                                // Subtract cached from raw to avoid double-counting.
                                input_tokens: (raw_input - cached).max(0),
                                output_tokens: as_i64(
                                    record.pointer("/payload/info/last_token_usage/output_tokens"),
                                ),
                                cache_read_input_tokens: cached,
                                cache_creation_input_tokens: 0,
                                reasoning_tokens: reasoning,
                                dedup_key,
                            },
                        });
                    }
                    _ => {}
                }
            }
            "response_item" => {
                let payload_type = record
                    .pointer("/payload/type")
                    .and_then(Value::as_str)
                    .unwrap_or_default();

                if matches!(payload_type, "function_call" | "custom_tool_call") {
                    let id = record
                        .pointer("/payload/call_id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let name = extract_tool_name(&record, payload_type);
                    let input = extract_tool_input_event(&record, payload_type);
                    if !id.is_empty() {
                        tool_meta_by_call_id.insert(id.clone(), (name.clone(), input.clone()));
                    }
                    if should_emit {
                        events.push(SessionEvent {
                            timestamp: ts.clone(),
                            kind: EventKind::ToolUse(ToolUseEvent { id, name, input }),
                        });
                    }
                }

                if matches!(
                    payload_type,
                    "function_call_output" | "custom_tool_call_output"
                ) {
                    if !should_emit {
                        continue;
                    }
                    let tool_use_id = record
                        .pointer("/payload/call_id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let (tool_name, tool_input) = tool_meta_by_call_id
                        .get(&tool_use_id)
                        .cloned()
                        .map(|(name, input)| (Some(name), Some(input)))
                        .unwrap_or((None, None));
                    let content = codex_tool_result_content(record.pointer("/payload/output"));
                    let is_error = record
                        .pointer("/payload/is_error")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    events.push(SessionEvent {
                        timestamp: ts.clone(),
                        kind: EventKind::ToolResult {
                            tool_use_id,
                            content,
                            is_error,
                            tool_name,
                            tool_input,
                        },
                    });
                }

                if should_emit {
                    if let Some(usage) = extract_response_item_usage_event(&record) {
                        events.push(SessionEvent {
                            timestamp: ts.clone(),
                            kind: usage,
                        });
                    }
                }
            }
            _ => {}
        }
    }

    Ok(events)
}

fn event_msg_text_block(value: Option<&Value>) -> Vec<ContentBlock> {
    match value {
        Some(Value::String(text)) => vec![ContentBlock::Text(text.to_string())],
        Some(Value::Null) | None => Vec::new(),
        Some(other) => vec![ContentBlock::Text(other.to_string())],
    }
}

fn codex_event_timestamp(record: &Value) -> Option<String> {
    record
        .get("timestamp")
        .and_then(Value::as_str)
        .or_else(|| record.pointer("/payload/timestamp").and_then(Value::as_str))
        .map(str::to_string)
}

fn extract_codex_stop_reason(record: &Value) -> String {
    record
        .pointer("/payload/stop_reason")
        .or_else(|| record.pointer("/payload/reason"))
        .and_then(Value::as_str)
        .unwrap_or("task_complete")
        .to_string()
}

fn extract_tool_input_event(record: &Value, payload_type: &str) -> Value {
    if payload_type == "function_call" {
        return parse_json_or_value(record.pointer("/payload/arguments"));
    }
    parse_json_or_value(record.pointer("/payload/input"))
}

fn parse_json_or_value(value: Option<&Value>) -> Value {
    match value {
        Some(Value::String(raw)) => {
            serde_json::from_str::<Value>(raw).unwrap_or_else(|_| Value::String(raw.to_string()))
        }
        Some(other) => other.clone(),
        None => Value::Null,
    }
}

fn codex_tool_result_content(value: Option<&Value>) -> Option<String> {
    match value {
        Some(Value::Null) | None => None,
        Some(Value::String(text)) => Some(text.to_string()),
        Some(other) => Some(other.to_string()),
    }
}

fn extract_response_item_usage_event(record: &Value) -> Option<EventKind> {
    let usage = record.pointer("/payload/usage")?;
    if usage.is_null() {
        return None;
    }
    let raw_input = as_i64(usage.get("input_tokens"));
    let cached = std::cmp::max(
        as_i64(usage.get("cached_input_tokens")),
        as_i64(usage.get("cache_read_input_tokens")),
    );
    let reasoning = as_i64(usage.get("reasoning_output_tokens"));
    Some(EventKind::Usage {
        input_tokens: (raw_input - cached).max(0),
        output_tokens: as_i64(usage.get("output_tokens")),
        cache_read_input_tokens: cached,
        cache_creation_input_tokens: 0,
        reasoning_tokens: reasoning,
        dedup_key: record
            .pointer("/payload/id")
            .or_else(|| record.pointer("/payload/call_id"))
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

fn extract_tool_name(record: &Value, payload_type: &str) -> String {
    let fallback = if payload_type == "custom_tool_call" {
        "custom_tool"
    } else {
        "unknown_tool"
    };
    record
        .pointer("/payload/name")
        .and_then(Value::as_str)
        .unwrap_or(fallback)
        .to_string()
}

fn extract_assistant_text(record: &Value, event_type: &str) -> Option<String> {
    let primary = if event_type == "task_complete" {
        "/payload/last_agent_message"
    } else {
        "/payload/message"
    };
    let fallback = if event_type == "task_complete" {
        "/payload/message"
    } else {
        "/payload/last_agent_message"
    };

    let text = record
        .pointer(primary)
        .and_then(Value::as_str)
        .or_else(|| record.pointer(fallback).and_then(Value::as_str))
        .map(str::trim)
        .unwrap_or_default();
    (!text.is_empty()).then(|| text.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_path(name: &str) -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        std::env::temp_dir().join(format!("gaal-codex-{unique}-{name}.jsonl"))
    }

    #[test]
    fn incremental_parse_keeps_tool_metadata_for_prior_call_ids() {
        let path = temp_path("incremental");
        let call = concat!(
            "{\"timestamp\":\"2026-03-07T10:00:00Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"function_call\",\"name\":\"exec_command\",\"arguments\":\"{\\\"cmd\\\":\\\"cargo build\\\"}\",\"call_id\":\"call_1\"}}\n"
        );
        let output = concat!(
            "{\"timestamp\":\"2026-03-07T10:00:01Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"function_call_output\",\"call_id\":\"call_1\",\"output\":\"Command failed\\nProcess exited with code 1\"}}\n"
        );
        fs::write(&path, format!("{call}{output}")).unwrap();

        let events = parse_events_from_offset(&path, call.len() as u64).unwrap();
        let _ = fs::remove_file(&path);

        assert_eq!(events.len(), 1);
        match &events[0].kind {
            EventKind::ToolResult {
                tool_use_id,
                tool_name,
                tool_input,
                ..
            } => {
                assert_eq!(tool_use_id, "call_1");
                assert_eq!(tool_name.as_deref(), Some("exec_command"));
                assert_eq!(
                    tool_input
                        .as_ref()
                        .and_then(|value| value.get("cmd"))
                        .and_then(Value::as_str),
                    Some("cargo build")
                );
            }
            other => panic!("expected ToolResult, got {other:?}"),
        }
    }
}
