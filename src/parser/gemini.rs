use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde_json::{Map, Value};

use super::common::as_i64;
use super::event::{ContentBlock, EventKind, SessionEvent, ToolUseEvent};

/// Parses a full Gemini session JSON file into canonical events.
pub fn parse_events(path: &Path) -> Result<Vec<SessionEvent>> {
    let contents = fs::read_to_string(path)
        .with_context(|| format!("failed to read Gemini session file: {}", path.display()))?;
    let root: Value = serde_json::from_str(&contents)
        .with_context(|| format!("failed to parse Gemini session JSON: {}", path.display()))?;

    let mut events = Vec::new();
    let root_ts = root
        .get("startTime")
        .and_then(Value::as_str)
        .map(str::to_string);
    let session_id = root
        .get("sessionId")
        .and_then(Value::as_str)
        .map(str::to_string);
    let model = first_gemini_model(&root);

    events.push(SessionEvent {
        timestamp: root_ts.clone(),
        kind: EventKind::Meta {
            session_id,
            model,
            cwd: None,
            version: None,
            forked_from_id: None,
            agent_role: None,
            agent_nickname: None,
        },
    });

    // Emit root-level summary if present so session headlines use it.
    if let Some(summary_text) = root.get("summary").and_then(Value::as_str) {
        if !summary_text.is_empty() {
            events.push(SessionEvent {
                timestamp: root_ts,
                kind: EventKind::Summary {
                    text: summary_text.to_string(),
                },
            });
        }
    }

    let messages = root
        .get("messages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    for message in messages {
        let Some(message_obj) = message.as_object() else {
            continue;
        };

        let ts = message_obj
            .get("timestamp")
            .and_then(Value::as_str)
            .map(str::to_string);

        match message_obj
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            "user" => {
                let content = extract_user_content(message_obj);
                if content.is_empty() {
                    continue;
                }
                events.push(SessionEvent {
                    timestamp: ts,
                    kind: EventKind::UserMessage { content },
                });
            }
            "gemini" => {
                let assistant_content = extract_assistant_content(message_obj);
                let model = message_obj
                    .get("model")
                    .and_then(Value::as_str)
                    .map(str::to_string);

                events.push(SessionEvent {
                    timestamp: ts.clone(),
                    kind: EventKind::AssistantMessage {
                        content: assistant_content,
                        model,
                        stop_reason: None,
                    },
                });

                for tool_event in extract_tool_events(message_obj) {
                    events.push(SessionEvent {
                        timestamp: ts.clone(),
                        kind: tool_event,
                    });
                }

                if let Some(usage) = extract_usage_event(message_obj) {
                    events.push(SessionEvent {
                        timestamp: ts,
                        kind: usage,
                    });
                }
            }
            msg_type @ ("info" | "warning" | "error") => {
                // Preserve info/warning/error messages so they appear in
                // transcripts and are searchable. Cancellation info maps to
                // StopSignal; other info/warning/error become system messages
                // via AssistantMessage with a bracketed type prefix.
                let text = message_obj
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if text.is_empty() {
                    continue;
                }
                let is_cancellation = msg_type == "info"
                    && text.to_ascii_lowercase().contains("cancel");
                if is_cancellation || msg_type == "error" {
                    events.push(SessionEvent {
                        timestamp: ts,
                        kind: EventKind::StopSignal {
                            reason: text.to_string(),
                        },
                    });
                } else {
                    // info/warning that aren't cancellations: surface as a
                    // system note in the assistant role so they show in transcripts.
                    let label = match msg_type {
                        "warning" => "[Warning]",
                        _ => "[Info]",
                    };
                    events.push(SessionEvent {
                        timestamp: ts,
                        kind: EventKind::AssistantMessage {
                            content: vec![super::event::ContentBlock::Text(format!(
                                "{label} {text}"
                            ))],
                            model: None,
                            stop_reason: None,
                        },
                    });
                }
            }
            _ => {}
        }
    }

    Ok(events)
}

/// Parses Gemini events from an offset.
///
/// Gemini stores each session as a single JSON object, so incremental offsets
/// are not meaningful; re-parse the full file for API compatibility.
pub fn parse_events_from_offset(path: &Path, _offset: u64) -> Result<Vec<SessionEvent>> {
    parse_events(path)
}

fn first_gemini_model(root: &Value) -> Option<String> {
    root.get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find_map(|message| {
            let message_type = message.get("type").and_then(Value::as_str)?;
            if message_type != "gemini" {
                return None;
            }
            message
                .get("model")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
}

fn extract_user_content(message: &Map<String, Value>) -> Vec<ContentBlock> {
    let Some(items) = message.get("content").and_then(Value::as_array) else {
        return Vec::new();
    };

    items
        .iter()
        .filter_map(|item| item.get("text").and_then(Value::as_str))
        .filter(|text| !text.is_empty())
        .map(|text| ContentBlock::Text(text.to_string()))
        .collect()
}

fn extract_assistant_content(message: &Map<String, Value>) -> Vec<ContentBlock> {
    let mut combined = String::new();

    if let Some(thoughts) = message.get("thoughts").and_then(Value::as_array) {
        for thought in thoughts {
            let Some(subject) = thought.get("subject").and_then(Value::as_str) else {
                continue;
            };
            let Some(description) = thought.get("description").and_then(Value::as_str) else {
                continue;
            };
            combined.push_str(&format!("[Thought: {subject}] {description}\n"));
        }
    }

    if let Some(content) = message.get("content").and_then(Value::as_str) {
        combined.push_str(content);
    } else if let Some(content) = message.get("content") {
        if !content.is_null() {
            combined.push_str(&content.to_string());
        }
    }

    if combined.is_empty() {
        Vec::new()
    } else {
        vec![ContentBlock::Text(combined)]
    }
}

fn extract_tool_events(message: &Map<String, Value>) -> Vec<EventKind> {
    let Some(tool_calls) = message.get("toolCalls").and_then(Value::as_array) else {
        return Vec::new();
    };

    let mut events = Vec::new();
    for tool_call in tool_calls {
        let Some(tool_obj) = tool_call.as_object() else {
            continue;
        };

        let id = tool_obj
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if id.is_empty() {
            continue;
        }

        let raw_name = tool_obj
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("unknown_tool");
        let name = normalize_tool_name(raw_name);
        let input = tool_obj.get("args").cloned().unwrap_or(Value::Null);

        events.push(EventKind::ToolUse(ToolUseEvent {
            id: id.clone(),
            name: name.clone(),
            input: input.clone(),
        }));

        let is_error = tool_obj
            .get("status")
            .and_then(Value::as_str)
            .map(|status| status != "success")
            .unwrap_or(false);
        let content = extract_tool_result_content(tool_obj.get("result"));

        events.push(EventKind::ToolResult {
            tool_use_id: id,
            content,
            is_error,
            tool_name: Some(name),
            tool_input: Some(input),
        });
    }

    events
}

fn extract_tool_result_content(result: Option<&Value>) -> Option<String> {
    let items = result?.as_array()?;

    let mut outputs = Vec::new();
    for item in items {
        // Check output first, then fall back to error text.
        if let Some(text) = item
            .pointer("/functionResponse/response/output")
            .and_then(Value::as_str)
        {
            outputs.push(text.to_string());
        } else if let Some(text) = item
            .pointer("/functionResponse/response/error")
            .and_then(Value::as_str)
        {
            outputs.push(text.to_string());
        }
    }

    if outputs.is_empty() {
        None
    } else {
        Some(outputs.join("\n"))
    }
}

fn extract_usage_event(message: &Map<String, Value>) -> Option<EventKind> {
    let tokens = message.get("tokens")?;
    if tokens.is_null() {
        return None;
    }

    Some(EventKind::Usage {
        input_tokens: as_i64(tokens.get("input")),
        output_tokens: as_i64(tokens.get("output")),
        cache_read_input_tokens: as_i64(tokens.get("cached")),
        cache_creation_input_tokens: 0,
        reasoning_tokens: as_i64(tokens.get("thoughts")),
        dedup_key: message
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

fn normalize_tool_name(raw: &str) -> String {
    match raw {
        "read_file" => "Read",
        "read_many_files" => "Read",
        "write_file" => "Write",
        "write_todos" => "WriteTodos",
        "replace" | "edit_file" => "Edit",
        "run_shell_command" => "Bash",
        "list_directory" | "glob" => "Glob",
        "grep_search" => "Grep",
        "google_web_search" => "WebSearch",
        "web_fetch" => "WebFetch",
        "save_memory" => "SaveMemory",
        "get_internal_docs" => "GetInternalDocs",
        "update_topic" => "UpdateTopic",
        "complete_task" => "CompleteTask",
        "ask_user" => "AskUser",
        "cli_help" => "CliHelp",
        "codebase_investigator" => "CodebaseInvestigator",
        "activate_skill" => "ActivateSkill",
        "enter_plan_mode" => "EnterPlanMode",
        "exit_plan_mode" => "ExitPlanMode",
        other => other,
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::normalize_tool_name;

    #[test]
    fn normalizes_additional_gemini_tool_names() {
        assert_eq!(normalize_tool_name("read_many_files"), "Read");
        assert_eq!(normalize_tool_name("write_todos"), "WriteTodos");
        assert_eq!(normalize_tool_name("save_memory"), "SaveMemory");
        assert_eq!(normalize_tool_name("get_internal_docs"), "GetInternalDocs");
        assert_eq!(normalize_tool_name("update_topic"), "UpdateTopic");
        assert_eq!(normalize_tool_name("complete_task"), "CompleteTask");
    }
}
