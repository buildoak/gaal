use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use anyhow::{Context, Result};
use serde_json::{Map, Value};

use super::event::{ContentBlock, EventKind, SessionEvent, ToolUseEvent};

const RESULT_EXIT_CODE_KEY: &str = "__gaal_explicit_exit_code";
const RESULT_SUCCESS_KEY: &str = "__gaal_explicit_success";

/// Parses a full Antigravity CLI JSONL transcript into canonical events.
pub fn parse_events(path: &Path) -> Result<Vec<SessionEvent>> {
    parse_events_from_offset(path, 0)
}

/// Parses Antigravity CLI JSONL events starting from a byte offset.
pub fn parse_events_from_offset(path: &Path, offset: u64) -> Result<Vec<SessionEvent>> {
    let file = File::open(path).with_context(|| {
        format!(
            "failed to open Antigravity session file: {}",
            path.display()
        )
    })?;
    let mut reader = BufReader::new(file);

    let mut body_events = Vec::new();
    let mut pending_tools: VecDeque<PendingTool> = VecDeque::new();
    let mut first_ts: Option<String> = None;
    let mut line_offset = 0_u64;

    loop {
        let mut line = String::new();
        let bytes_read = reader
            .read_line(&mut line)
            .context("failed to read Antigravity JSONL line")?;
        if bytes_read == 0 {
            break;
        }

        let line_start = line_offset;
        line_offset = line_offset.saturating_add(bytes_read as u64);
        let trimmed = line.trim();
        if trimmed.is_empty() || line_start < offset {
            continue;
        }

        let record: Value = match serde_json::from_str(trimmed) {
            Ok(value) => value,
            Err(_) => continue,
        };
        let Some(record_obj) = record.as_object() else {
            continue;
        };

        let ts = record_obj
            .get("created_at")
            .and_then(Value::as_str)
            .map(str::to_string);
        if first_ts.is_none() {
            first_ts = ts.clone();
        }

        let record_type = record_obj
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();

        match record_type {
            "USER_INPUT" => {
                if let Some(text) = text_content(record_obj.get("content")) {
                    body_events.push(SessionEvent {
                        timestamp: ts,
                        kind: EventKind::UserMessage {
                            content: vec![ContentBlock::Text(text)],
                        },
                    });
                }
            }
            "PLANNER_RESPONSE" => {
                if let Some(text) = text_content(record_obj.get("content")) {
                    body_events.push(SessionEvent {
                        timestamp: ts.clone(),
                        kind: EventKind::AssistantMessage {
                            content: vec![ContentBlock::Text(text)],
                            model: None,
                            stop_reason: None,
                        },
                    });
                }

                for (idx, tool_call) in extract_tool_calls(record_obj).into_iter().enumerate() {
                    let id = tool_id(record_obj, idx);
                    let name = normalize_tool_name(&tool_call.name);
                    let input = normalize_args(tool_call.args);
                    pending_tools.push_back(PendingTool {
                        id: id.clone(),
                        name: name.clone(),
                        input: input.clone(),
                    });
                }
            }
            "RUN_COMMAND" | "VIEW_FILE" | "LIST_DIRECTORY" | "GREP_SEARCH" | "SEARCH_WEB"
            | "GENERATE_IMAGE" => {
                emit_tool_execution(
                    record_obj,
                    record_type,
                    ts,
                    &mut pending_tools,
                    &mut body_events,
                );
            }
            "CHECKPOINT" | "EPHEMERAL_MESSAGE" | "SYSTEM_MESSAGE" | "GENERIC"
            | "list_permissions" => {}
            "ERROR_MESSAGE" => {
                if let Some(reason) = text_content(record_obj.get("content")) {
                    let id = record_obj
                        .get("step_index")
                        .and_then(Value::as_i64)
                        .map(|idx| format!("agy_error_{idx}"))
                        .unwrap_or_else(|| format!("agy_error_{}", body_events.len()));
                    body_events.push(SessionEvent {
                        timestamp: ts.clone(),
                        kind: EventKind::ToolResult {
                            tool_use_id: id,
                            content: Some(reason.clone()),
                            is_error: true,
                            tool_name: None,
                            tool_input: None,
                        },
                    });
                    body_events.push(SessionEvent {
                        timestamp: ts,
                        kind: EventKind::StopSignal { reason },
                    });
                }
            }
            _ => {}
        }
    }

    if offset == 0 {
        let mut events = vec![SessionEvent {
            timestamp: first_ts,
            kind: EventKind::Meta {
                session_id: session_id_from_path(path),
                model: None,
                cwd: None,
                version: None,
                forked_from_id: None,
                agent_role: None,
                agent_nickname: None,
            },
        }];
        events.extend(body_events);
        Ok(events)
    } else {
        Ok(body_events)
    }
}

fn emit_tool_execution(
    record: &Map<String, Value>,
    record_type: &str,
    ts: Option<String>,
    pending_tools: &mut VecDeque<PendingTool>,
    events: &mut Vec<SessionEvent>,
) {
    let canonical_name = tool_name_for_record_type(record_type);
    let pending_idx = pending_tools
        .iter()
        .position(|tool| tool.name == canonical_name);
    let pending = pending_idx.and_then(|idx| pending_tools.remove(idx));

    let (id, input) = if let Some(pending) = pending {
        let input = merge_tool_inputs(pending.input, derive_tool_input(record, record_type));
        events.push(SessionEvent {
            timestamp: ts.clone(),
            kind: EventKind::ToolUse(ToolUseEvent {
                id: pending.id.clone(),
                name: canonical_name.to_string(),
                input: input.clone(),
            }),
        });
        (pending.id, input)
    } else {
        let input = derive_tool_input(record, record_type).unwrap_or(Value::Null);
        let id = record
            .get("step_index")
            .and_then(Value::as_i64)
            .map(|idx| format!("agy_{idx}"))
            .unwrap_or_else(|| format!("agy_{}", events.len()));
        events.push(SessionEvent {
            timestamp: ts.clone(),
            kind: EventKind::ToolUse(ToolUseEvent {
                id: id.clone(),
                name: canonical_name.to_string(),
                input: input.clone(),
            }),
        });
        (id, input)
    };
    let result_input = add_result_status_metadata(input.clone(), record);

    if let Some(content) = tool_result_content(record) {
        events.push(SessionEvent {
            timestamp: ts,
            kind: EventKind::ToolResult {
                tool_use_id: id,
                content: Some(content),
                is_error: is_error_record(record),
                tool_name: Some(canonical_name.to_string()),
                tool_input: Some(result_input),
            },
        });
    } else if is_error_record(record) {
        events.push(SessionEvent {
            timestamp: ts,
            kind: EventKind::ToolResult {
                tool_use_id: id,
                content: None,
                is_error: true,
                tool_name: Some(canonical_name.to_string()),
                tool_input: Some(result_input),
            },
        });
    }
}

fn extract_tool_calls(record: &Map<String, Value>) -> Vec<AgyToolCall> {
    record
        .get("tool_calls")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|call| {
            let obj = call.as_object()?;
            let name = obj.get("name").and_then(Value::as_str)?.to_string();
            if normalize_tool_name(&name) == "GenerateImage" {
                return None;
            }
            let args = obj.get("args").cloned().unwrap_or(Value::Null);
            Some(AgyToolCall { name, args })
        })
        .collect()
}

fn tool_id(record: &Map<String, Value>, idx: usize) -> String {
    match record.get("step_index").and_then(Value::as_i64) {
        Some(step) => format!("agy_{step}_{idx}"),
        None => format!("agy_unknown_{idx}"),
    }
}

fn text_content(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(text) if !text.trim().is_empty() => Some(text.to_string()),
        Value::Array(items) => {
            let text = items
                .iter()
                .filter_map(|item| {
                    item.as_str()
                        .or_else(|| item.get("text").and_then(Value::as_str))
                })
                .filter(|text| !text.trim().is_empty())
                .collect::<Vec<_>>()
                .join("\n");
            if text.is_empty() {
                None
            } else {
                Some(text)
            }
        }
        Value::Object(_) | Value::Number(_) | Value::Bool(_) => Some(value?.to_string()),
        _ => None,
    }
}

fn tool_result_content(record: &Map<String, Value>) -> Option<String> {
    if record.get("type").and_then(Value::as_str) == Some("RUN_COMMAND") {
        let content = text_content(record.get("content")).unwrap_or_default();
        let content = append_exit_metadata(extract_command_result(&content), record);
        return (!content.trim().is_empty()).then_some(content);
    }

    let content = text_content(record.get("content"))?;
    if content.trim().is_empty() {
        None
    } else {
        Some(content)
    }
}

fn extract_command_result(content: &str) -> String {
    if let Some((_, output)) = content.split_once("Output:") {
        let trimmed = output.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    content.trim().to_string()
}

fn is_error_record(record: &Map<String, Value>) -> bool {
    if record.get("success").and_then(Value::as_bool) == Some(true) {
        return false;
    }
    if record.get("exit_code").and_then(Value::as_i64) == Some(0) {
        return false;
    }
    if record.get("success").and_then(Value::as_bool) == Some(false) {
        return true;
    }
    if record
        .get("exit_code")
        .and_then(Value::as_i64)
        .is_some_and(|code| code != 0)
    {
        return true;
    }

    let status_is_error = record
        .get("status")
        .and_then(Value::as_str)
        .map(|status| {
            matches!(
                status,
                "ERROR" | "FAILED" | "FAILURE" | "CANCELED" | "CANCELLED" | "TIMEOUT" | "TIMED_OUT"
            )
        })
        .unwrap_or(false);
    if status_is_error {
        return true;
    }

    text_content(record.get("content"))
        .map(|content| {
            let lower = content.to_ascii_lowercase();
            lower.contains("error:") || lower.contains("failed") || lower.contains("command failed")
        })
        .unwrap_or(false)
}

fn derive_tool_input(record: &Map<String, Value>, record_type: &str) -> Option<Value> {
    let mut input = Map::new();
    if let Some(args) = record.get("args") {
        if let Value::Object(map) = normalize_args(args.clone()) {
            input.extend(map);
        }
    }
    if let Some(raw_input) = record.get("input") {
        if let Value::Object(map) = normalize_args(raw_input.clone()) {
            input.extend(map);
        }
    }
    for key in [
        "CommandLine",
        "command",
        "Cwd",
        "cwd",
        "AbsolutePath",
        "file_path",
        "DirectoryPath",
        "directory",
        "SearchPath",
        "path",
        "Query",
        "query",
        "Prompt",
        "artifact_path",
    ] {
        if let Some(value) = record.get(key).and_then(Value::as_str) {
            input.insert(
                normalize_arg_key(key).to_string(),
                Value::String(value.to_string()),
            );
        }
    }
    let content = record
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match record_type {
        "VIEW_FILE" => {
            if !input.contains_key("file_path") {
                if let Some(path) = extract_backticked_file_uri(content)
                    .or_else(|| extract_prefixed_line(content, "File Path:"))
                {
                    input.insert("file_path".to_string(), Value::String(path));
                }
            }
        }
        "GENERATE_IMAGE" => {
            if !input.contains_key("query") {
                if let Some(prompt) = extract_prefixed_line(content, "Using prompt:") {
                    input.insert("query".to_string(), Value::String(prompt));
                }
            }
            if !input.contains_key("file_path") {
                if let Some(path) = record
                    .get("artifact_path")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| extract_generated_image_path(content))
                {
                    input.insert("file_path".to_string(), Value::String(path));
                }
            }
        }
        _ => {}
    }

    if input.is_empty() {
        None
    } else {
        Some(Value::Object(input))
    }
}

fn extract_backticked_file_uri(content: &str) -> Option<String> {
    let start = content.find("`file://")? + "`file://".len();
    let rest = &content[start..];
    let end = rest.find('`')?;
    Some(rest[..end].to_string())
}

fn extract_prefixed_line(content: &str, prefix: &str) -> Option<String> {
    content.lines().find_map(|line| {
        let rest = line.trim().strip_prefix(prefix)?;
        let trimmed = rest.trim().trim_matches('`');
        if trimmed.is_empty() {
            None
        } else {
            Some(
                trimmed
                    .strip_prefix("file://")
                    .unwrap_or(trimmed)
                    .to_string(),
            )
        }
    })
}

fn extract_generated_image_path(content: &str) -> Option<String> {
    let (_, rest) = content.split_once("Generated image is saved at ")?;
    let path = rest
        .split_whitespace()
        .next()?
        .trim_matches('`')
        .trim_end_matches(['.', ',', ';', ':']);
    (!path.is_empty()).then(|| path.to_string())
}

fn normalize_tool_name(raw: &str) -> String {
    match raw {
        "run_command" => "Bash",
        "view_file" => "Read",
        "list_dir" | "list_directory" => "Glob",
        "grep_search" => "Grep",
        "search_web" => "WebSearch",
        "generate_image" => "GenerateImage",
        other => other,
    }
    .to_string()
}

fn tool_name_for_record_type(record_type: &str) -> &'static str {
    match record_type {
        "RUN_COMMAND" => "Bash",
        "VIEW_FILE" => "Read",
        "LIST_DIRECTORY" => "Glob",
        "GREP_SEARCH" => "Grep",
        "SEARCH_WEB" => "WebSearch",
        "GENERATE_IMAGE" => "GenerateImage",
        _ => "Unknown",
    }
}

fn normalize_args(value: Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(key, value)| (normalize_arg_key(&key).to_string(), normalize_args(value)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.into_iter().map(normalize_args).collect()),
        other => other,
    }
}

fn normalize_arg_key(key: &str) -> &str {
    match key {
        "CommandLine" => "command",
        "Cwd" => "cwd",
        "AbsolutePath" => "file_path",
        "DirectoryPath" => "directory",
        "SearchPath" => "path",
        "Query" => "query",
        "Prompt" => "query",
        "ArtifactPath" => "file_path",
        "artifact_path" => "file_path",
        other => other,
    }
}

fn merge_tool_inputs(base: Value, extra: Option<Value>) -> Value {
    let Some(extra) = extra else {
        return base;
    };
    match (base, extra) {
        (Value::Object(mut base), Value::Object(extra)) => {
            for (key, value) in extra {
                base.insert(key, value);
            }
            Value::Object(base)
        }
        (base, _) => base,
    }
}

fn append_exit_metadata(mut content: String, record: &Map<String, Value>) -> String {
    if let Some(code) = record.get("exit_code").and_then(Value::as_i64) {
        if content.trim().is_empty() {
            return format!("Process exited with code {code}");
        }
        content = format!("Process exited with code {code}\n{content}");
    }
    content
}

fn add_result_status_metadata(mut input: Value, record: &Map<String, Value>) -> Value {
    let Some(input_obj) = input.as_object_mut() else {
        return input;
    };
    if let Some(code) = record.get("exit_code").and_then(Value::as_i64) {
        input_obj.insert(RESULT_EXIT_CODE_KEY.to_string(), Value::Number(code.into()));
    }
    if let Some(success) = record.get("success").and_then(Value::as_bool) {
        input_obj.insert(RESULT_SUCCESS_KEY.to_string(), Value::Bool(success));
    }
    input
}

fn session_id_from_path(path: &Path) -> Option<String> {
    let mut previous_was_brain = false;
    for component in path.components() {
        let value = component.as_os_str().to_string_lossy();
        if previous_was_brain {
            return Some(value.chars().take(8).collect());
        }
        previous_was_brain = value == "brain";
    }

    path.parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .map(|value| value.chars().take(8).collect())
}

struct AgyToolCall {
    name: String,
    args: Value,
}

struct PendingTool {
    id: String,
    name: String,
    input: Value,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::facts;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_transcript(name: &str, body: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "gaal-agy-parser-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time")
                .as_nanos()
        ));
        let logs = root.join("brain/12345678-aaaa-bbbb-cccc-123456789abc/.system_generated/logs");
        fs::create_dir_all(&logs).unwrap();
        let path = logs.join("transcript_full.jsonl");
        fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn maps_user_assistant_and_command_events() {
        let path = temp_transcript(
            "command",
            r#"{"step_index":0,"source":"USER_EXPLICIT","type":"USER_INPUT","status":"DONE","created_at":"2026-06-22T18:46:54Z","content":"run pwd"}
{"step_index":1,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","created_at":"2026-06-22T18:46:55Z","content":"I will run it.","tool_calls":[{"name":"run_command","args":{"CommandLine":"pwd","Cwd":"/tmp","toolSummary":"Run pwd"}}]}
{"step_index":2,"source":"MODEL","type":"RUN_COMMAND","status":"DONE","created_at":"2026-06-22T18:46:56Z","content":"Created At: 2026-06-22T18:46:56Z\nCompleted At: 2026-06-22T18:46:56Z\n\nThe command completed successfully.\nOutput:\n/tmp\n"}
"#,
        );

        let events = parse_events(&path).unwrap();
        fs::remove_dir_all(path.ancestors().nth(5).unwrap()).ok();

        assert!(matches!(
            events.first().map(|event| &event.kind),
            Some(EventKind::Meta {
                session_id: Some(id),
                ..
            }) if id == "12345678"
        ));
        assert!(events
            .iter()
            .any(|event| matches!(event.kind, EventKind::UserMessage { .. })));
        assert!(events.iter().any(|event| matches!(
            &event.kind,
            EventKind::AssistantMessage { content, .. }
                if matches!(content.first(), Some(ContentBlock::Text(text)) if text == "I will run it.")
        )));
        assert!(events.iter().any(|event| matches!(
            &event.kind,
            EventKind::ToolUse(ToolUseEvent { name, input, .. })
                if name == "Bash"
                    && input.get("command").and_then(Value::as_str) == Some("pwd")
                    && input.get("cwd").and_then(Value::as_str) == Some("/tmp")
        )));
        assert!(events.iter().any(|event| matches!(
            &event.kind,
            EventKind::ToolResult {
                content: Some(content),
                is_error: false,
                tool_name: Some(name),
                ..
            } if name == "Bash" && content == "/tmp"
        )));
    }

    #[test]
    fn maps_file_search_web_image_and_error_records() {
        let path = temp_transcript(
            "tools",
            r#"{"step_index":1,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","created_at":"2026-06-22T18:46:55Z","tool_calls":[{"name":"view_file","args":{"AbsolutePath":"/tmp/a.txt"}},{"name":"list_dir","args":{"DirectoryPath":"/tmp"}},{"name":"grep_search","args":{"SearchPath":"/tmp","Query":"needle"}},{"name":"search_web","args":{"Query":"rust jsonl"}},{"name":"generate_image","args":{"Prompt":"blue square"}}]}
{"step_index":2,"source":"MODEL","type":"VIEW_FILE","status":"DONE","created_at":"2026-06-22T18:46:56Z","content":"File Path: `file:///tmp/a.txt`\ncontents"}
{"step_index":3,"source":"MODEL","type":"LIST_DIRECTORY","status":"DONE","created_at":"2026-06-22T18:46:57Z","content":"file list"}
{"step_index":4,"source":"MODEL","type":"GREP_SEARCH","status":"DONE","created_at":"2026-06-22T18:46:58Z","content":"match"}
{"step_index":5,"source":"MODEL","type":"SEARCH_WEB","status":"DONE","created_at":"2026-06-22T18:46:59Z","content":"result"}
{"step_index":6,"source":"MODEL","type":"GENERATE_IMAGE","status":"DONE","created_at":"2026-06-22T18:47:00Z","content":"Using prompt: blue square\nGenerated image is saved at /tmp/x.jpg."}
{"step_index":7,"source":"SYSTEM","type":"ERROR_MESSAGE","status":"DONE","created_at":"2026-06-22T18:47:01Z","content":"Error: model output error"}
"#,
        );

        let events = parse_events(&path).unwrap();
        fs::remove_dir_all(path.ancestors().nth(5).unwrap()).ok();

        for name in ["Read", "Glob", "Grep", "WebSearch", "GenerateImage"] {
            assert!(events.iter().any(|event| matches!(
                &event.kind,
                EventKind::ToolUse(ToolUseEvent { name: actual, .. }) if actual == name
            )));
        }
        assert!(events.iter().any(|event| matches!(
            &event.kind,
            EventKind::ToolUse(ToolUseEvent { name, input, .. })
                if name == "Read"
                    && input.get("file_path").and_then(Value::as_str) == Some("/tmp/a.txt")
        )));
        assert!(events.iter().any(|event| matches!(
            &event.kind,
            EventKind::ToolUse(ToolUseEvent { name, input, .. })
                if name == "Glob"
                    && input.get("directory").and_then(Value::as_str) == Some("/tmp")
        )));
        assert!(events.iter().any(|event| matches!(
            &event.kind,
            EventKind::ToolUse(ToolUseEvent { name, input, .. })
                if name == "Grep"
                    && input.get("path").and_then(Value::as_str) == Some("/tmp")
                    && input.get("query").and_then(Value::as_str) == Some("needle")
        )));
        assert!(events.iter().any(|event| matches!(
            &event.kind,
            EventKind::ToolUse(ToolUseEvent { name, input, .. })
                if name == "GenerateImage"
                    && input.get("file_path").and_then(Value::as_str) == Some("/tmp/x.jpg")
                    && input.get("query").and_then(Value::as_str) == Some("blue square")
        )));
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(
                    &event.kind,
                    EventKind::ToolUse(ToolUseEvent { name, .. }) if name == "GenerateImage"
                ))
                .count(),
            1
        );
        assert!(events
            .iter()
            .any(|event| matches!(&event.kind, EventKind::StopSignal { reason } if reason.contains("model output error"))));
        assert!(events.iter().any(|event| matches!(
            &event.kind,
            EventKind::ToolResult {
                is_error: true,
                content: Some(content),
                ..
            } if content.contains("model output error")
        )));
    }

    #[test]
    fn generated_image_path_strips_sentence_punctuation() {
        let path = extract_generated_image_path(
            "Using prompt: banana\nGenerated image is saved at /tmp/banana_1782230390990.jpg. The file is ready.",
        );
        assert_eq!(path.as_deref(), Some("/tmp/banana_1782230390990.jpg"));
    }

    #[test]
    fn successful_command_with_failed_text_is_not_error() {
        let path = temp_transcript(
            "success-failed-text",
            r#"{"step_index":1,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","created_at":"2026-06-22T18:46:55Z","tool_calls":[{"name":"run_command","args":{"CommandLine":"cargo test","Cwd":"/tmp"}}]}
{"step_index":2,"source":"MODEL","type":"RUN_COMMAND","status":"DONE","created_at":"2026-06-22T18:46:56Z","exit_code":0,"success":true,"content":"Output:\n0 failed; 12 passed\n"}
"#,
        );

        let events = parse_events(&path).unwrap();
        fs::remove_dir_all(path.ancestors().nth(5).unwrap()).ok();

        assert!(events.iter().any(|event| matches!(
            &event.kind,
            EventKind::ToolResult {
                is_error: false,
                content: Some(content),
                ..
            } if content.starts_with("Process exited with code 0") && content.contains("0 failed")
        )));
    }

    #[test]
    fn explicit_success_status_wins_over_exit_like_output_text_in_facts() {
        let path = temp_transcript(
            "success-exit-like-output",
            r#"{"step_index":1,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","created_at":"2026-06-22T18:46:55Z","tool_calls":[{"name":"run_command","args":{"CommandLine":"printf weird","Cwd":"/tmp"}}]}
{"step_index":2,"source":"MODEL","type":"RUN_COMMAND","status":"DONE","created_at":"2026-06-22T18:46:56Z","exit_code":0,"success":true,"content":"Output:\nProcess exited with code 1\n"}
"#,
        );

        let events = parse_events(&path).unwrap();
        let parsed = facts::extract_parsed_session(&events, crate::parser::Engine::Agy, &path);
        fs::remove_dir_all(path.ancestors().nth(5).unwrap()).ok();

        let command = parsed
            .facts
            .iter()
            .find(|fact| fact.fact_type.as_str() == "command")
            .expect("command fact");
        assert_eq!(command.exit_code, Some(0));
        assert_eq!(command.success, Some(true));
        assert!(
            parsed
                .facts
                .iter()
                .all(|fact| fact.fact_type.as_str() != "error"),
            "explicit successful agy status must not create error facts: {:?}",
            parsed.facts
        );
    }

    #[test]
    fn failed_command_without_content_gets_exit_metadata() {
        let path = temp_transcript(
            "empty-failure",
            r#"{"step_index":1,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","created_at":"2026-06-22T18:46:55Z","tool_calls":[{"name":"run_command","args":{"CommandLine":"false","Cwd":"/tmp"}}]}
{"step_index":2,"source":"MODEL","type":"RUN_COMMAND","status":"DONE","created_at":"2026-06-22T18:46:56Z","exit_code":2,"success":false}
"#,
        );

        let events = parse_events(&path).unwrap();
        fs::remove_dir_all(path.ancestors().nth(5).unwrap()).ok();

        assert!(events.iter().any(|event| matches!(
            &event.kind,
            EventKind::ToolResult {
                is_error: true,
                content: Some(content),
                ..
            } if content.contains("Process exited with code 2")
        )));
    }

    #[test]
    fn planner_tool_calls_do_not_create_tool_events_or_facts() {
        let path = temp_transcript(
            "plan-only",
            r#"{"step_index":1,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","created_at":"2026-06-22T18:46:55Z","tool_calls":[{"name":"run_command","args":{"CommandLine":"pwd","Cwd":"/tmp"}},{"name":"view_file","args":{"AbsolutePath":"/tmp/a.txt"}}]}
"#,
        );

        let events = parse_events(&path).unwrap();
        let parsed = facts::extract_parsed_session(&events, crate::parser::Engine::Agy, &path);
        fs::remove_dir_all(path.ancestors().nth(5).unwrap()).ok();

        assert!(!events
            .iter()
            .any(|event| matches!(event.kind, EventKind::ToolUse(_))));
        assert_eq!(parsed.total_tools, 0);
        assert!(
            parsed.facts.is_empty(),
            "plan-only tool calls must not create facts: {:?}",
            parsed.facts
        );
    }

    #[test]
    fn planned_then_executed_command_creates_exactly_one_command_fact() {
        let path = temp_transcript(
            "planned-executed-once",
            r#"{"step_index":1,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","created_at":"2026-06-22T18:46:55Z","tool_calls":[{"name":"run_command","args":{"CommandLine":"pwd","Cwd":"/tmp"}}]}
{"step_index":2,"source":"MODEL","type":"RUN_COMMAND","status":"DONE","created_at":"2026-06-22T18:46:56Z","exit_code":0,"success":true,"content":"Output:\n/tmp\n"}
"#,
        );

        let events = parse_events(&path).unwrap();
        let parsed = facts::extract_parsed_session(&events, crate::parser::Engine::Agy, &path);
        fs::remove_dir_all(path.ancestors().nth(5).unwrap()).ok();

        let tool_uses = events
            .iter()
            .filter(|event| matches!(&event.kind, EventKind::ToolUse(ToolUseEvent { name, .. }) if name == "Bash"))
            .count();
        let command_facts = parsed
            .facts
            .iter()
            .filter(|fact| fact.fact_type.as_str() == "command")
            .count();
        assert_eq!(tool_uses, 1);
        assert_eq!(parsed.total_tools, 1);
        assert_eq!(command_facts, 1);
    }

    #[test]
    fn executed_tool_input_overrides_stale_planned_input() {
        let path = temp_transcript(
            "planned-executed-override",
            r#"{"step_index":1,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","created_at":"2026-06-22T18:46:55Z","tool_calls":[{"name":"run_command","args":{"CommandLine":"planned command","Cwd":"/planned"}}]}
{"step_index":2,"source":"MODEL","type":"RUN_COMMAND","status":"DONE","created_at":"2026-06-22T18:46:56Z","exit_code":0,"success":true,"command":"executed command","cwd":"/executed","content":"Output:\nok\n"}
"#,
        );

        let events = parse_events(&path).unwrap();
        let parsed = facts::extract_parsed_session(&events, crate::parser::Engine::Agy, &path);
        fs::remove_dir_all(path.ancestors().nth(5).unwrap()).ok();

        let command = parsed
            .facts
            .iter()
            .find(|fact| fact.fact_type.as_str() == "command")
            .expect("command fact");
        assert_eq!(command.detail.as_deref(), Some("executed command"));
    }

    #[test]
    fn unknown_command_status_without_explicit_result_stays_unknown() {
        let path = temp_transcript(
            "unknown-status",
            r#"{"step_index":1,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","created_at":"2026-06-22T18:46:55Z","tool_calls":[{"name":"run_command","args":{"CommandLine":"echo hi","Cwd":"/tmp"}}]}
{"step_index":2,"source":"MODEL","type":"RUN_COMMAND","status":"MYSTERY","created_at":"2026-06-22T18:46:56Z","content":"Output:\nhi\n"}
"#,
        );

        let events = parse_events(&path).unwrap();
        let parsed = facts::extract_parsed_session(&events, crate::parser::Engine::Agy, &path);
        fs::remove_dir_all(path.ancestors().nth(5).unwrap()).ok();

        let command = parsed
            .facts
            .iter()
            .find(|fact| fact.fact_type.as_str() == "command")
            .expect("command fact");
        assert_eq!(command.exit_code, None);
        assert_eq!(command.success, None);
        assert!(parsed
            .facts
            .iter()
            .all(|fact| fact.fact_type.as_str() != "error"));
    }

    #[test]
    fn explicit_failed_command_without_exit_code_does_not_fabricate_exit_code() {
        let path = temp_transcript(
            "explicit-failure-no-exit",
            r#"{"step_index":1,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","created_at":"2026-06-22T18:46:55Z","tool_calls":[{"name":"run_command","args":{"CommandLine":"false","Cwd":"/tmp"}}]}
{"step_index":2,"source":"MODEL","type":"RUN_COMMAND","status":"DONE","created_at":"2026-06-22T18:46:56Z","success":false}
"#,
        );

        let events = parse_events(&path).unwrap();
        let parsed = facts::extract_parsed_session(&events, crate::parser::Engine::Agy, &path);
        fs::remove_dir_all(path.ancestors().nth(5).unwrap()).ok();

        let command = parsed
            .facts
            .iter()
            .find(|fact| fact.fact_type.as_str() == "command")
            .expect("command fact");
        assert_eq!(command.exit_code, None);
        assert_eq!(command.success, Some(false));
        let errors: Vec<_> = parsed
            .facts
            .iter()
            .filter(|fact| fact.fact_type.as_str() == "error")
            .collect();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].exit_code, None);
        assert_eq!(errors[0].success, Some(false));
        assert!(!events.iter().any(|event| matches!(
            &event.kind,
            EventKind::ToolResult {
                content: Some(content),
                ..
            } if content.contains("Process exited with code 1")
        )));
    }

    #[test]
    fn lifecycle_system_and_permission_noise_is_ignored() {
        let path = temp_transcript(
            "ignored-events",
            r#"{"step_index":1,"source":"SYSTEM","type":"CHECKPOINT","status":"DONE","created_at":"2026-06-22T18:46:55Z","content":"checkpoint"}
{"step_index":2,"source":"SYSTEM","type":"EPHEMERAL_MESSAGE","status":"DONE","created_at":"2026-06-22T18:46:56Z","content":"typing"}
{"step_index":3,"source":"SYSTEM","type":"SYSTEM_MESSAGE","status":"DONE","created_at":"2026-06-22T18:46:57Z","content":"system"}
{"step_index":4,"source":"SYSTEM","type":"GENERIC","status":"DONE","created_at":"2026-06-22T18:46:58Z","content":"generic"}
{"step_index":5,"source":"MODEL","type":"list_permissions","status":"DONE","created_at":"2026-06-22T18:46:59Z","content":"permissions","args":{"CommandLine":"not a command","AbsolutePath":"/tmp/nope"}}
"#,
        );

        let events = parse_events(&path).unwrap();
        let parsed = facts::extract_parsed_session(&events, crate::parser::Engine::Agy, &path);
        fs::remove_dir_all(path.ancestors().nth(5).unwrap()).ok();

        assert_eq!(
            events.len(),
            1,
            "only the synthetic meta event should remain"
        );
        assert!(parsed.facts.is_empty());
        assert_eq!(parsed.total_tools, 0);
    }
}
