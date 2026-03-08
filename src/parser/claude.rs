use std::collections::{HashMap, HashSet};
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

/// Parses a full Claude JSONL session file.
pub fn parse(path: &Path) -> Result<ParsedSession> {
    parse_from_offset(path, 0)
}

/// Parses Claude JSONL content starting from a byte offset.
pub(crate) fn parse_from_offset(path: &Path, offset: u64) -> Result<ParsedSession> {
    let file = File::open(path)
        .with_context(|| format!("failed to open Claude session file: {}", path.display()))?;
    let mut reader = BufReader::new(file);
    if offset > 0 {
        reader
            .seek(SeekFrom::Start(offset))
            .with_context(|| format!("failed to seek Claude session file: {}", path.display()))?;
    }

    let mut session_id: Option<String> = None;
    let mut model: Option<String> = None;
    let mut cwd: Option<String> = None;
    let mut version: Option<String> = None;
    let mut started_at: Option<String> = None;
    let mut last_event_at: Option<String> = None;
    let mut last_stop_reason: Option<String> = None;

    let mut facts: Vec<Fact> = Vec::new();
    let mut tool_state_by_id: HashMap<String, ToolCallState> = HashMap::new();
    let mut usage_keys_seen: HashSet<String> = HashSet::new();

    let mut total_input_tokens = 0i64;
    let mut total_output_tokens = 0i64;
    let mut total_tools = 0i32;
    let mut total_turns = 0i32;

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
        if started_at.is_none() {
            started_at = ts.clone();
        }
        if ts.is_some() {
            last_event_at = ts.clone();
        }

        if session_id.is_none() {
            session_id = record
                .get("sessionId")
                .and_then(Value::as_str)
                .map(str::to_string);
        }
        if cwd.is_none() {
            cwd = record
                .get("cwd")
                .and_then(Value::as_str)
                .map(str::to_string);
        }
        if version.is_none() {
            version = record
                .get("version")
                .and_then(Value::as_str)
                .map(str::to_string);
        }
        if model.is_none() {
            model = record
                .pointer("/message/model")
                .and_then(Value::as_str)
                .map(str::to_string);
        }
        if let Some(stop_reason) = record
            .pointer("/message/stop_reason")
            .and_then(Value::as_str)
            .map(str::to_string)
        {
            last_stop_reason = Some(stop_reason);
        }

        let (in_tokens, out_tokens, usage_key) = extract_claude_usage(&record);
        if in_tokens != 0 || out_tokens != 0 {
            let should_count = usage_key
                .as_ref()
                .map(|key| usage_keys_seen.insert(key.clone()))
                .unwrap_or(true);
            if should_count {
                total_input_tokens += in_tokens;
                total_output_tokens += out_tokens;
            }
        }

        let record_type = record
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if is_noise_type(record_type) {
            continue;
        }

        if record_type == "user" {
            total_turns += 1;
            let turn_number = Some(total_turns);
            let content = record.pointer("/message/content").unwrap_or(&Value::Null);

            if let Some(prompt) = first_user_text(content) {
                facts.push(Fact {
                    id: None,
                    session_id: String::new(),
                    ts: ts.clone().unwrap_or_default(),
                    turn_number,
                    fact_type: FactType::UserPrompt,
                    subject: None,
                    detail: Some(prompt),
                    exit_code: None,
                    success: None,
                });
            }

            if let Some(blocks) = content.as_array() {
                for block in blocks {
                    if block.get("type").and_then(Value::as_str) != Some("tool_result") {
                        continue;
                    }

                    let call_id = block
                        .get("tool_use_id")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    let output_text = value_to_text(block.get("content").unwrap_or(&Value::Null));
                    let is_error = block
                        .get("is_error")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    let exit_code = parse_exit_code(output_text.as_deref().unwrap_or_default());

                    if let Some(id) = call_id.as_ref() {
                        if let Some(state) = tool_state_by_id.get(id) {
                            if let Some(fact_idx) = state.fact_index {
                                if let Some(fact) = facts.get_mut(fact_idx) {
                                    if state.tool_name.eq_ignore_ascii_case("bash") {
                                        fact.exit_code = exit_code;
                                        fact.success = Some(exit_code.unwrap_or(0) == 0);
                                    }
                                }
                            }
                        }
                    }

                    let output_has_error = output_text
                        .as_ref()
                        .map(|text| contains_error(text))
                        .unwrap_or(false);
                    if is_error || output_has_error {
                        let state = call_id
                            .as_ref()
                            .and_then(|id| tool_state_by_id.get(id))
                            .cloned();
                        facts.push(Fact {
                            id: None,
                            session_id: String::new(),
                            ts: ts.clone().unwrap_or_default(),
                            turn_number,
                            fact_type: FactType::Error,
                            subject: state.as_ref().and_then(|s| s.subject.clone()),
                            detail: output_text
                                .clone()
                                .or_else(|| state.as_ref().and_then(|s| s.detail.clone())),
                            exit_code,
                            success: Some(false),
                        });
                    }
                }
            }
            continue;
        }

        if record_type != "assistant" {
            continue;
        }

        let turn_number = if total_turns > 0 {
            Some(total_turns)
        } else {
            None
        };
        let content = record.pointer("/message/content").unwrap_or(&Value::Null);
        let Some(blocks) = content.as_array() else {
            continue;
        };

        for block in blocks {
            let block_type = block
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if block_type == "text" {
                if let Some(text) = block.get("text").and_then(Value::as_str) {
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        facts.push(Fact {
                            id: None,
                            session_id: String::new(),
                            ts: ts.clone().unwrap_or_default(),
                            turn_number,
                            fact_type: FactType::AssistantReply,
                            subject: None,
                            detail: Some(truncate(trimmed, 500)),
                            exit_code: None,
                            success: None,
                        });
                    }
                }
                continue;
            }

            if block_type != "tool_use" {
                continue;
            }

            total_tools += 1;
            let tool_name = block
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let input = block.get("input").cloned().unwrap_or(Value::Null);
            let fact = tool_call_fact(
                &tool_name,
                &input,
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

            if let Some(id) = block.get("id").and_then(Value::as_str).map(str::to_string) {
                tool_state_by_id.insert(id, state);
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
            engine: Engine::Claude,
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
        exit_signal: last_stop_reason,
        last_event_at,
    })
}

fn extract_claude_usage(record: &Value) -> (i64, i64, Option<String>) {
    let input = as_i64(record.pointer("/message/usage/input_tokens"));
    let output = as_i64(record.pointer("/message/usage/output_tokens"));
    let key = record
        .pointer("/message/id")
        .and_then(Value::as_str)
        .map(str::to_string);
    (input, output, key)
}

fn first_user_text(content: &Value) -> Option<String> {
    if let Some(text) = content.as_str() {
        let trimmed = text.trim();
        return (!trimmed.is_empty()).then(|| trimmed.to_string());
    }
    content.as_array().and_then(|blocks| {
        blocks.iter().find_map(|block| {
            let is_text = block.get("type").and_then(Value::as_str) == Some("text");
            let text = block
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let trimmed = text.trim();
            (is_text && !trimmed.is_empty()).then(|| trimmed.to_string())
        })
    })
}

fn value_to_text(value: &Value) -> Option<String> {
    if let Some(text) = value.as_str() {
        return Some(text.to_string());
    }
    if let Some(items) = value.as_array() {
        let joined = items
            .iter()
            .filter_map(|item| item.get("text").and_then(Value::as_str))
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        return (!joined.is_empty()).then_some(joined);
    }
    serde_json::to_string(value).ok()
}

fn is_noise_type(record_type: &str) -> bool {
    matches!(
        record_type,
        "queue-operation" | "progress" | "file-history-snapshot" | "system"
    )
}
