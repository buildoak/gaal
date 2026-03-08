use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::Path;

use anyhow::{Context, Result};
use serde_json::Value;

use crate::model::fact::FactType;
use crate::model::Fact;

use super::common::{
    as_i64, contains_error, is_git_command, parse_exit_code, resolve_started_at, tool_call_fact,
    truncate,
};
use super::types::{Engine, ParsedSession, SessionMeta};

#[derive(Debug, Clone)]
struct ToolCallState {
    tool_name: String,
    fact_index: Option<usize>,
    subject: Option<String>,
    detail: Option<String>,
}

/// Parses a full Codex JSONL session file.
pub fn parse(path: &Path) -> Result<ParsedSession> {
    parse_from_offset(path, 0)
}

/// Parses Codex JSONL content starting from a byte offset.
pub(crate) fn parse_from_offset(path: &Path, offset: u64) -> Result<ParsedSession> {
    let file = File::open(path)
        .with_context(|| format!("failed to open Codex session file: {}", path.display()))?;
    let mut reader = BufReader::new(file);
    if offset > 0 {
        reader
            .seek(SeekFrom::Start(offset))
            .with_context(|| format!("failed to seek Codex session file: {}", path.display()))?;
    }

    let mut session_id: Option<String> = None;
    let mut model: Option<String> = None;
    let mut cwd: Option<String> = None;
    let mut version: Option<String> = None;
    let mut started_at: Option<String> = None;
    let mut last_event_at: Option<String> = None;
    let mut last_event_kind: Option<String> = None;

    let mut facts: Vec<Fact> = Vec::new();
    let mut call_state_by_id: HashMap<String, ToolCallState> = HashMap::new();

    let mut total_input_tokens = 0i64;
    let mut total_output_tokens = 0i64;
    let mut total_tools = 0i32;
    let mut total_turns = 0i32;

    for line_result in reader.lines() {
        let line = line_result.context("failed to read Codex JSONL line")?;
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
        if started_at.is_none() {
            started_at = ts.clone();
        }
        if ts.is_some() {
            last_event_at = ts.clone();
        }

        let record_type = record
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();

        if record_type == "turn_context" {
            if session_id.is_none() {
                session_id = record
                    .pointer("/payload/id")
                    .and_then(Value::as_str)
                    .map(str::to_string);
            }
            if cwd.is_none() {
                cwd = record
                    .pointer("/payload/cwd")
                    .and_then(Value::as_str)
                    .map(str::to_string);
            }
            if model.is_none() {
                model = record
                    .pointer("/payload/model")
                    .and_then(Value::as_str)
                    .map(str::to_string);
            }
        }

        if record_type == "session_meta" {
            if session_id.is_none() {
                session_id = record
                    .pointer("/payload/id")
                    .and_then(Value::as_str)
                    .map(str::to_string);
            }
            if cwd.is_none() {
                cwd = record
                    .pointer("/payload/cwd")
                    .and_then(Value::as_str)
                    .map(str::to_string);
            }
            if version.is_none() {
                version = record
                    .pointer("/payload/cli_version")
                    .and_then(Value::as_str)
                    .map(str::to_string);
            }
        }

        let (in_tokens, out_tokens) = extract_codex_usage(&record);
        total_input_tokens += in_tokens;
        total_output_tokens += out_tokens;

        if record_type == "event_msg" {
            let event_type = record
                .pointer("/payload/type")
                .and_then(Value::as_str)
                .unwrap_or_default();
            last_event_kind = Some(event_type.to_string());
            let turn_number = if total_turns > 0 {
                Some(total_turns)
            } else {
                None
            };

            if event_type == "user_message" {
                total_turns += 1;
                let text = record
                    .pointer("/payload/message")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .unwrap_or_default()
                    .to_string();
                if !text.is_empty() {
                    facts.push(Fact {
                        id: None,
                        session_id: String::new(),
                        ts: ts.clone().unwrap_or_default(),
                        turn_number: Some(total_turns),
                        fact_type: FactType::UserPrompt,
                        subject: None,
                        detail: Some(text),
                        exit_code: None,
                        success: None,
                    });
                }
            }

            if matches!(event_type, "task_complete" | "agent_message") {
                let text = extract_assistant_text(&record, event_type);
                if let Some(detail) = text {
                    facts.push(Fact {
                        id: None,
                        session_id: String::new(),
                        ts: ts.clone().unwrap_or_default(),
                        turn_number,
                        fact_type: FactType::AssistantReply,
                        subject: None,
                        detail: Some(truncate(&detail, 500)),
                        exit_code: None,
                        success: None,
                    });
                }
            }
            continue;
        }

        if record_type != "response_item" {
            continue;
        }

        let payload_type = record
            .pointer("/payload/type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let turn_number = if total_turns > 0 {
            Some(total_turns)
        } else {
            None
        };
        last_event_kind = Some(payload_type.to_string());

        if matches!(payload_type, "function_call" | "custom_tool_call") {
            total_tools += 1;
            let tool_name = extract_tool_name(&record, payload_type);
            let tool_input = extract_tool_input(&record, payload_type);
            let fact = tool_call_fact(
                &tool_name,
                &tool_input,
                ts.clone().unwrap_or_default(),
                turn_number,
            );

            let mut state = ToolCallState {
                tool_name: tool_name.clone(),
                fact_index: None,
                subject: None,
                detail: None,
            };

            if let Some(mut call_fact) = fact {
                if matches!(&call_fact.fact_type, FactType::Command) {
                    if let Some(cmd) = call_fact.detail.clone() {
                        if is_git_command(&cmd) {
                            facts.push(Fact {
                                id: None,
                                session_id: String::new(),
                                ts: ts.clone().unwrap_or_default(),
                                turn_number,
                                fact_type: FactType::GitOp,
                                subject: Some(truncate(&cmd, 100)),
                                detail: Some(cmd),
                                exit_code: None,
                                success: None,
                            });
                        }
                    }
                }
                state.subject = call_fact.subject.clone();
                state.detail = call_fact.detail.clone();
                state.fact_index = Some(facts.len());
                call_fact.session_id = String::new();
                facts.push(call_fact);
            }

            if let Some(call_id) = record
                .pointer("/payload/call_id")
                .and_then(Value::as_str)
                .map(str::to_string)
            {
                call_state_by_id.insert(call_id, state);
            }
            continue;
        }

        if matches!(
            payload_type,
            "function_call_output" | "custom_tool_call_output"
        ) {
            let call_id = record
                .pointer("/payload/call_id")
                .and_then(Value::as_str)
                .map(str::to_string);
            let output = value_to_text(record.pointer("/payload/output").unwrap_or(&Value::Null));
            let is_error = record
                .pointer("/payload/is_error")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let exit_code = parse_exit_code(output.as_deref().unwrap_or_default());

            if let Some(id) = call_id.as_ref() {
                if let Some(state) = call_state_by_id.get(id) {
                    if let Some(fact_idx) = state.fact_index {
                        if let Some(fact) = facts.get_mut(fact_idx) {
                            if state.tool_name.eq_ignore_ascii_case("bash")
                                || state.tool_name.eq_ignore_ascii_case("exec_command")
                            {
                                fact.exit_code = exit_code;
                                fact.success = Some(exit_code.unwrap_or(0) == 0);
                            }
                        }
                    }
                }
            }

            let output_has_error = output
                .as_ref()
                .map(|text| contains_error(text))
                .unwrap_or(false);
            if is_error || output_has_error {
                let state = call_id
                    .as_ref()
                    .and_then(|id| call_state_by_id.get(id))
                    .cloned();
                facts.push(Fact {
                    id: None,
                    session_id: String::new(),
                    ts: ts.clone().unwrap_or_default(),
                    turn_number,
                    fact_type: FactType::Error,
                    subject: state.as_ref().and_then(|s| s.subject.clone()),
                    detail: output
                        .clone()
                        .or_else(|| state.as_ref().and_then(|s| s.detail.clone())),
                    exit_code,
                    success: Some(false),
                });
            }
        }
    }

    let fallback_id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();
    let resolved_id = session_id.unwrap_or(fallback_id);
    for fact in &mut facts {
        fact.session_id = resolved_id.clone();
    }

    let resolved_start = resolve_started_at(started_at, last_event_at.clone());
    Ok(ParsedSession {
        meta: SessionMeta {
            id: resolved_id,
            engine: Engine::Codex,
            model,
            cwd,
            started_at: resolved_start,
            version,
        },
        facts,
        total_input_tokens,
        total_output_tokens,
        total_tools,
        total_turns,
        ended_at: last_event_at.clone(),
        exit_signal: resolve_exit_signal(last_event_kind),
        last_event_at,
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

fn extract_tool_input(record: &Value, payload_type: &str) -> Value {
    if payload_type == "function_call" {
        return parse_json_string(record.pointer("/payload/arguments").and_then(Value::as_str));
    }
    parse_json_string(record.pointer("/payload/input").and_then(Value::as_str))
}

fn parse_json_string(value: Option<&str>) -> Value {
    value
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        .unwrap_or(Value::Null)
}

fn extract_codex_usage(record: &Value) -> (i64, i64) {
    let event_type = record.pointer("/payload/type").and_then(Value::as_str);
    if event_type == Some("token_count") {
        let input = as_i64(record.pointer("/payload/info/last_token_usage/input_tokens"));
        let output = as_i64(record.pointer("/payload/info/last_token_usage/output_tokens"));
        return (input, output);
    }
    let input = as_i64(record.pointer("/payload/usage/input_tokens"));
    let output = as_i64(record.pointer("/payload/usage/output_tokens"));
    (input, output)
}

fn extract_assistant_text(record: &Value, event_type: &str) -> Option<String> {
    let key = if event_type == "task_complete" {
        "/payload/last_agent_message"
    } else {
        "/payload/message"
    };
    let text = record
        .pointer(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    (!text.is_empty()).then(|| text.to_string())
}

fn value_to_text(value: &Value) -> Option<String> {
    if let Some(text) = value.as_str() {
        return Some(text.to_string());
    }
    serde_json::to_string(value).ok()
}

fn resolve_exit_signal(last_event_kind: Option<String>) -> Option<String> {
    let kind = last_event_kind.unwrap_or_default();
    if kind == "task_complete" {
        Some(kind)
    } else {
        None
    }
}
