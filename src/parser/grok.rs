use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::sync::OnceLock;
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result};
use chrono::{DateTime, TimeZone, Utc};
use regex::Regex;
use serde_json::{json, Map, Value};

use crate::db::queries::{
    GrokSessionMetaRecord, GrokSourceState, ParserObservationRecord, SourceArtifactRecord,
};

use super::event::{ContentBlock, EventKind, SessionEvent, ToolUseEvent};
use super::facts;
use super::types::{Engine, ParsedSession};

const RESULT_EXIT_CODE_KEY: &str = "__gaal_explicit_exit_code";
const RESULT_SUCCESS_KEY: &str = "__gaal_explicit_success";
const SOURCE_SCHEMA_VERSION: &str = "grok-0.2.93-bundle-v1";
const VISIBILITY_POLICY_VERSION: &str = "grok-visibility-v1";
const TOOL_OUTPUT_LIMIT_CHARS: usize = 20_000;
const TOOL_OUTPUT_PREVIEW_CHARS: usize = 16_000;

/// Parse one native Grok Build session directory.
///
/// Grok 0.2.x stores one logical session as a directory. `summary.json` is
/// metadata; visible conversational facts come from `updates.jsonl` when it has
/// recoverable visible events, with `chat_history.jsonl` as the fallback only.
pub fn parse_session(session_dir: &Path, session_id: &str) -> Result<ParsedSession> {
    let events = parse_events(session_dir, session_id)?;
    Ok(facts::extract_parsed_session(
        &events,
        Engine::Grok,
        session_dir,
    ))
}

pub fn parse_events(session_dir: &Path, session_id: &str) -> Result<Vec<SessionEvent>> {
    let summary = read_json(session_dir.join("summary.json").as_path());
    let fallback_timestamp = summary_timestamp(summary.as_ref());
    let mut events = vec![summary_meta_event(session_id, summary.as_ref())];

    let updates_path = session_dir.join("updates.jsonl");
    let (mut visible_events, visible_count) =
        parse_updates_jsonl(&updates_path).unwrap_or_else(|_| (Vec::new(), 0));

    if visible_count == 0 {
        let chat_path = session_dir.join("chat_history.jsonl");
        visible_events =
            parse_chat_history_jsonl(&chat_path, fallback_timestamp.as_deref()).unwrap_or_default();
    }

    events.append(&mut visible_events);
    Ok(events)
}

pub fn source_state(session_dir: &Path, session_id: &str) -> GrokSourceState {
    let summary = read_json(session_dir.join("summary.json").as_path());
    let meta = Some(GrokSessionMetaRecord {
        session_id: session_id.to_string(),
        agent_name: summary
            .as_ref()
            .and_then(|value| value.get("agent_name"))
            .and_then(Value::as_str)
            .map(str::to_string),
        chat_format_version: summary
            .as_ref()
            .and_then(|value| value.get("chat_format_version"))
            .and_then(Value::as_i64),
        current_model_id: summary
            .as_ref()
            .and_then(|value| value.get("current_model_id"))
            .and_then(Value::as_str)
            .map(str::to_string),
        reasoning_effort: summary
            .as_ref()
            .and_then(|value| value.get("reasoning_effort"))
            .and_then(Value::as_str)
            .map(str::to_string),
        sandbox_profile: summary
            .as_ref()
            .and_then(|value| value.get("sandbox_profile"))
            .and_then(Value::as_str)
            .map(str::to_string),
        source_schema_version: SOURCE_SCHEMA_VERSION.to_string(),
        visibility_policy_version: VISIBILITY_POLICY_VERSION.to_string(),
    });

    let mut artifacts = Vec::new();
    let mut observations = Vec::new();
    let mut updates_visible = 0_i64;
    let mut updates_present = false;
    let mut chat_visible = 0_i64;

    let Ok(entries) = std::fs::read_dir(session_dir) else {
        observations.push(observation(
            session_id,
            "error",
            "unreadable_session_dir",
            "Grok session directory could not be read",
            None,
            Some(session_dir.display().to_string()),
            1,
        ));
        return GrokSourceState {
            meta,
            artifacts,
            observations,
        };
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_file() {
            continue;
        }
        let rel_path = entry.file_name().to_string_lossy().to_string();
        let role = artifact_role(&rel_path);
        let mut artifact = base_artifact(session_id, &role, &rel_path, &path);
        match role.as_str() {
            "summary" => {
                artifact.visibility = "metadata_only".to_string();
                artifact.parse_status =
                    if summary.is_some() { "ok" } else { "malformed" }.to_string();
                artifact.private_count = summary
                    .as_ref()
                    .map(|value| {
                        i64::from(value.get("generated_title").is_some())
                            + i64::from(value.get("session_summary").is_some())
                    })
                    .unwrap_or(0);
            }
            "updates" => {
                updates_present = true;
                classify_updates_file(&path, &mut artifact);
                updates_visible = artifact.visible_count;
            }
            "chat_history" => {
                classify_chat_file(&path, &mut artifact);
                chat_visible = artifact.visible_count;
            }
            "prompt_context" | "system_prompt" | "rewind_points" => {
                artifact.visibility = "private_not_indexed".to_string();
                artifact.parse_status = "skipped_private".to_string();
                artifact.private_count = 1;
            }
            "events" | "signals" => {
                artifact.visibility = "metadata_only".to_string();
                artifact.parse_status = "metadata_only".to_string();
                artifact.redacted_count = 1;
            }
            _ => {
                artifact.visibility = "unknown_redacted".to_string();
                artifact.parse_status = "unknown_redacted".to_string();
                artifact.unknown_count = 1;
            }
        }
        artifacts.push(artifact);
    }

    let (source_selected, fallback_reason) = if updates_visible > 0 {
        ("updates", None)
    } else if chat_visible > 0 {
        (
            "chat_history",
            Some(if updates_present {
                "updates had no recoverable visible events"
            } else {
                "updates missing"
            }),
        )
    } else {
        ("none", Some("no visible source"))
    };
    observations.push(observation(
        session_id,
        "info",
        "source_selected",
        source_selected,
        Some(source_selected.to_string()),
        None,
        1,
    ));
    if let Some(reason) = fallback_reason {
        observations.push(observation(
            session_id,
            "warn",
            "fallback_reason",
            reason,
            Some(source_selected.to_string()),
            None,
            1,
        ));
    }

    let unknown_total: i64 = artifacts
        .iter()
        .map(|artifact| artifact.unknown_count)
        .sum();
    if unknown_total > 0 {
        observations.push(observation(
            session_id,
            "warn",
            "unknown_records",
            "Unknown Grok records/files were redacted",
            None,
            None,
            unknown_total,
        ));
    }
    let malformed_total: i64 = artifacts
        .iter()
        .map(|artifact| artifact.malformed_count)
        .sum();
    if malformed_total > 0 {
        observations.push(observation(
            session_id,
            "warn",
            "malformed_records",
            "Malformed Grok JSONL lines were skipped",
            None,
            None,
            malformed_total,
        ));
    }

    GrokSourceState {
        meta,
        artifacts,
        observations,
    }
}

fn summary_meta_event(session_id: &str, summary: Option<&Value>) -> SessionEvent {
    SessionEvent {
        timestamp: summary_timestamp(summary),
        kind: EventKind::Meta {
            session_id: Some(session_id.to_string()),
            model: summary
                .and_then(|value| value.get("current_model_id"))
                .and_then(Value::as_str)
                .map(str::to_string),
            cwd: summary
                .and_then(|value| value.get("git_root_dir"))
                .and_then(Value::as_str)
                .map(str::to_string),
            version: summary
                .and_then(|value| value.get("agent_name"))
                .and_then(Value::as_str)
                .map(str::to_string),
            forked_from_id: None,
            agent_role: None,
            agent_nickname: None,
        },
    }
}

fn summary_timestamp(summary: Option<&Value>) -> Option<String> {
    summary
        .and_then(|value| value.get("created_at"))
        .and_then(Value::as_str)
        .map(normalize_timestamp)
}

fn artifact_role(file_name: &str) -> String {
    match file_name {
        "summary.json" => "summary",
        "updates.jsonl" => "updates",
        "chat_history.jsonl" => "chat_history",
        "events.jsonl" => "events",
        "signals.json" => "signals",
        "prompt_context.json" => "prompt_context",
        "system_prompt.txt" => "system_prompt",
        "rewind_points.jsonl" => "rewind_points",
        _ => "unknown_file",
    }
    .to_string()
}

fn base_artifact(
    session_id: &str,
    role: &str,
    rel_path: &str,
    path: &Path,
) -> SourceArtifactRecord {
    let meta = std::fs::metadata(path).ok();
    let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
    let mtime_unix = meta
        .as_ref()
        .and_then(|m| m.modified().ok())
        .and_then(|mtime| mtime.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs() as i64);
    let fingerprint = artifact_fingerprint(size, mtime_unix);
    SourceArtifactRecord {
        session_id: session_id.to_string(),
        role: role.to_string(),
        rel_path: rel_path.to_string(),
        visibility: "metadata_only".to_string(),
        size_bytes: i64::try_from(size).unwrap_or(i64::MAX),
        mtime_unix,
        fingerprint,
        parse_status: "ok".to_string(),
        visible_count: 0,
        private_count: 0,
        redacted_count: 0,
        unknown_count: 0,
        malformed_count: 0,
    }
}

fn artifact_fingerprint(size: u64, mtime_unix: Option<i64>) -> i64 {
    let mut state = 0xcbf29ce484222325_u64;
    for byte in size.to_le_bytes() {
        state ^= u64::from(byte);
        state = state.wrapping_mul(0x100000001b3);
    }
    for byte in mtime_unix.unwrap_or(0).to_le_bytes() {
        state ^= u64::from(byte);
        state = state.wrapping_mul(0x100000001b3);
    }
    (state & i64::MAX as u64) as i64
}

fn classify_updates_file(path: &Path, artifact: &mut SourceArtifactRecord) {
    artifact.visibility = "visible_body".to_string();
    let Ok(file) = File::open(path) else {
        artifact.parse_status = "unreadable".to_string();
        return;
    };
    for (idx, line_result) in BufReader::new(file).lines().enumerate() {
        let Ok(line) = line_result else {
            artifact.malformed_count += 1;
            continue;
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(record) = serde_json::from_str::<Value>(trimmed) else {
            artifact.malformed_count += 1;
            continue;
        };
        let method = record
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !matches!(method, "session/update" | "_x.ai/session/update") {
            artifact.unknown_count += 1;
            continue;
        }
        let update_kind = record
            .pointer("/params/update/sessionUpdate")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match update_kind {
            "user_message_chunk" | "agent_message_chunk" | "tool_call" | "tool_call_update" => {
                artifact.visible_count += 1;
            }
            "agent_thought_chunk" => artifact.private_count += 1,
            "turn_completed" | "hook_execution" => artifact.redacted_count += 1,
            "" => {
                artifact.unknown_count += 1;
                artifact.parse_status = format!("unknown_record_at_line_{}", idx + 1);
            }
            _ => artifact.unknown_count += 1,
        }
    }
    if artifact.malformed_count > 0 {
        artifact.parse_status = "partial_malformed".to_string();
    } else if artifact.visible_count == 0 && artifact.unknown_count > 0 {
        artifact.parse_status = "unknown_only".to_string();
    } else {
        artifact.parse_status = "ok".to_string();
    }
}

fn classify_chat_file(path: &Path, artifact: &mut SourceArtifactRecord) {
    artifact.visibility = "visible_body".to_string();
    let Ok(file) = File::open(path) else {
        artifact.parse_status = "unreadable".to_string();
        return;
    };
    for line_result in BufReader::new(file).lines() {
        let Ok(line) = line_result else {
            artifact.malformed_count += 1;
            continue;
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(record) = serde_json::from_str::<Value>(trimmed) else {
            artifact.malformed_count += 1;
            continue;
        };
        match record
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            "user" | "assistant" | "tool_result" => artifact.visible_count += 1,
            "reasoning" | "system" => artifact.private_count += 1,
            _ => artifact.unknown_count += 1,
        }
    }
    artifact.parse_status = if artifact.malformed_count > 0 {
        "partial_malformed"
    } else {
        "ok"
    }
    .to_string();
}

fn observation(
    session_id: &str,
    severity: &str,
    kind: &str,
    message: &str,
    source_role: Option<String>,
    source_ref: Option<String>,
    count: i64,
) -> ParserObservationRecord {
    ParserObservationRecord {
        session_id: session_id.to_string(),
        parser: "grok".to_string(),
        severity: severity.to_string(),
        kind: kind.to_string(),
        message: message.to_string(),
        source_role,
        source_ref,
        count,
    }
}

fn parse_updates_jsonl(path: &Path) -> Result<(Vec<SessionEvent>, usize)> {
    let file = File::open(path)
        .with_context(|| format!("failed to open Grok updates file: {}", path.display()))?;
    let reader = BufReader::new(file);
    let mut builder = UpdateEventBuilder::default();

    for line_result in reader.lines() {
        let line = line_result.context("failed to read Grok updates JSONL line")?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(record) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };
        builder.consume_update_record(&record);
    }

    Ok(builder.finish())
}

fn parse_chat_history_jsonl(
    path: &Path,
    fallback_timestamp: Option<&str>,
) -> Result<Vec<SessionEvent>> {
    let file = File::open(path)
        .with_context(|| format!("failed to open Grok chat history file: {}", path.display()))?;
    let reader = BufReader::new(file);
    let mut events = Vec::new();
    let mut tool_meta_by_id: HashMap<String, (String, Value)> = HashMap::new();
    let mut emitted_result_ids = HashSet::new();

    for line_result in reader.lines() {
        let line = line_result.context("failed to read Grok chat history JSONL line")?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(record) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };
        let timestamp =
            grok_record_timestamp(&record).or_else(|| fallback_timestamp.map(str::to_string));
        match record
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            "user" => {
                if let Some(text) =
                    extract_chat_text(&record).filter(|text| !text.trim().is_empty())
                {
                    events.push(SessionEvent {
                        timestamp,
                        kind: EventKind::UserMessage {
                            content: vec![ContentBlock::Text(text)],
                        },
                    });
                }
            }
            "assistant" => {
                if let Some(text) =
                    extract_chat_text(&record).filter(|text| !text.trim().is_empty())
                {
                    events.push(SessionEvent {
                        timestamp: timestamp.clone(),
                        kind: EventKind::AssistantMessage {
                            content: vec![ContentBlock::Text(text)],
                            model: record
                                .get("model")
                                .or_else(|| record.get("model_id"))
                                .and_then(Value::as_str)
                                .map(str::to_string),
                            stop_reason: None,
                        },
                    });
                }
                for tool_use in chat_tool_uses(&record) {
                    tool_meta_by_id.insert(
                        tool_use.id.clone(),
                        (tool_use.name.clone(), tool_use.input.clone()),
                    );
                    events.push(SessionEvent {
                        timestamp: timestamp.clone(),
                        kind: EventKind::ToolUse(tool_use),
                    });
                }
            }
            "tool_result" => {
                let id = record
                    .get("tool_call_id")
                    .or_else(|| record.get("toolCallId"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if id.is_empty() || !emitted_result_ids.insert(id.clone()) {
                    continue;
                }
                let (tool_name, tool_input) = tool_meta_by_id
                    .get(&id)
                    .cloned()
                    .map(|(name, input)| (Some(name), Some(input)))
                    .unwrap_or((None, None));
                events.push(SessionEvent {
                    timestamp,
                    kind: EventKind::ToolResult {
                        tool_use_id: id,
                        content: safe_chat_tool_output(&record),
                        // Native chat_history records do not expose a reliable error flag.
                        is_error: false,
                        tool_name,
                        tool_input,
                    },
                });
            }
            _ => {}
        }
    }

    Ok(events)
}

#[derive(Default)]
struct UpdateEventBuilder {
    events: Vec<SessionEvent>,
    visible_count: usize,
    pending_user_chunks: Vec<String>,
    pending_assistant_chunks: Vec<String>,
    tool_meta_by_id: HashMap<String, (String, Value)>,
    emitted_tool_use_ids: HashSet<String>,
    emitted_result_ids: HashSet<String>,
}

impl UpdateEventBuilder {
    fn consume_update_record(&mut self, record: &Value) {
        let method = record
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !matches!(method, "session/update" | "_x.ai/session/update") {
            return;
        }
        let Some(update) = record.pointer("/params/update") else {
            return;
        };
        let timestamp = grok_record_timestamp(record);
        match update
            .get("sessionUpdate")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            "user_message_chunk" => {
                self.flush_assistant(timestamp.clone());
                if let Some(text) = extract_text(update).filter(|text| !text.trim().is_empty()) {
                    self.pending_user_chunks.push(text);
                }
            }
            "agent_message_chunk" => {
                self.flush_user(timestamp.clone());
                if let Some(text) = extract_text(update).filter(|text| !text.trim().is_empty()) {
                    self.pending_assistant_chunks.push(text);
                }
            }
            "agent_thought_chunk" => {}
            "tool_call" => {
                self.flush_user(timestamp.clone());
                self.flush_assistant(timestamp.clone());
                let id = update
                    .get("toolCallId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if id.is_empty() {
                    return;
                }
                let name = normalize_tool_name(update);
                let input = safe_tool_input(update);
                self.tool_meta_by_id
                    .insert(id.clone(), (name.clone(), input.clone()));
                if !self.emitted_tool_use_ids.insert(id.clone()) {
                    return;
                }
                self.events.push(SessionEvent {
                    timestamp,
                    kind: EventKind::ToolUse(ToolUseEvent { id, name, input }),
                });
                self.visible_count += 1;
            }
            "tool_call_update" => {
                let status = update
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if !matches!(status, "completed" | "failed") {
                    return;
                }
                self.flush_user(timestamp.clone());
                self.flush_assistant(timestamp.clone());
                let id = update
                    .get("toolCallId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if id.is_empty() {
                    return;
                }
                let name_input = self
                    .tool_meta_by_id
                    .get(&id)
                    .cloned()
                    .unwrap_or_else(|| (normalize_tool_name(update), safe_tool_input(update)));
                // A tool-call ID identifies one logical result. Grok can replay
                // terminal lifecycle rows, and applying the ID gate uniformly
                // also keeps other completed tool updates idempotent.
                if !self.emitted_result_ids.insert(id.clone()) {
                    return;
                }
                let output = safe_tool_output(update);
                let mut result_input = name_input.1.clone();
                attach_result_status(&mut result_input, update, status);
                self.events.push(SessionEvent {
                    timestamp,
                    kind: EventKind::ToolResult {
                        tool_use_id: id,
                        content: output,
                        is_error: status == "failed",
                        tool_name: Some(name_input.0),
                        tool_input: Some(result_input),
                    },
                });
                self.visible_count += 1;
            }
            "turn_completed" => {
                self.flush_user(timestamp.clone());
                self.flush_assistant(timestamp.clone());
                self.events.push(SessionEvent {
                    timestamp,
                    kind: EventKind::StopSignal {
                        reason: "turn_completed".to_string(),
                    },
                });
            }
            _ => {}
        }
    }

    fn finish(mut self) -> (Vec<SessionEvent>, usize) {
        self.flush_user(None);
        self.flush_assistant(None);
        (self.events, self.visible_count)
    }

    fn flush_user(&mut self, timestamp: Option<String>) {
        if self.pending_user_chunks.is_empty() {
            return;
        }
        let text = join_chunks(&self.pending_user_chunks);
        self.pending_user_chunks.clear();
        if text.trim().is_empty() {
            return;
        }
        self.events.push(SessionEvent {
            timestamp,
            kind: EventKind::UserMessage {
                content: vec![ContentBlock::Text(text)],
            },
        });
        self.visible_count += 1;
    }

    fn flush_assistant(&mut self, timestamp: Option<String>) {
        if self.pending_assistant_chunks.is_empty() {
            return;
        }
        let text = join_chunks(&self.pending_assistant_chunks);
        self.pending_assistant_chunks.clear();
        if text.trim().is_empty() {
            return;
        }
        self.events.push(SessionEvent {
            timestamp,
            kind: EventKind::AssistantMessage {
                content: vec![ContentBlock::Text(text)],
                model: None,
                stop_reason: None,
            },
        });
        self.visible_count += 1;
    }
}

fn join_chunks(chunks: &[String]) -> String {
    chunks.join("")
}

fn extract_text(value: &Value) -> Option<String> {
    value
        .get("text")
        .and_then(Value::as_str)
        .or_else(|| value.pointer("/content/text").and_then(Value::as_str))
        .or_else(|| value.pointer("/delta/text").and_then(Value::as_str))
        .map(redact_visible_text)
}

fn extract_chat_text(value: &Value) -> Option<String> {
    if let Some(text) = value
        .get("text")
        .and_then(Value::as_str)
        .or_else(|| value.get("content").and_then(Value::as_str))
        .or_else(|| value.pointer("/content/text").and_then(Value::as_str))
    {
        return Some(redact_visible_text(text));
    }

    let chunks = value
        .get("content")
        .and_then(Value::as_array)?
        .iter()
        .filter_map(|item| {
            let item_type = item.get("type").and_then(Value::as_str);
            if item_type.is_some_and(|kind| kind != "text") {
                return None;
            }
            item.get("text").and_then(Value::as_str)
        })
        .collect::<Vec<_>>();
    (!chunks.is_empty()).then(|| redact_visible_text(&chunks.join("")))
}

fn chat_tool_uses(record: &Value) -> Vec<ToolUseEvent> {
    record
        .get("tool_calls")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|call| {
            let id = call
                .get("id")
                .or_else(|| call.get("tool_call_id"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if id.is_empty() {
                return None;
            }
            let name = normalize_tool_name(call);
            let raw_input = call
                .get("arguments")
                .and_then(|arguments| match arguments {
                    Value::String(encoded) => serde_json::from_str::<Value>(encoded).ok(),
                    other => Some(other.clone()),
                })
                .unwrap_or_else(|| json!({}));
            Some(ToolUseEvent {
                id,
                name,
                input: safe_tool_input_value(&raw_input),
            })
        })
        .collect()
}

fn normalize_tool_name(update: &Value) -> String {
    let raw = update
        .get("title")
        .or_else(|| update.get("name"))
        .or_else(|| update.get("kind"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let normalized = raw.trim().trim_end_matches(':').to_ascii_lowercase();
    match normalized.as_str() {
        "read" | "read_file" | "open_file" | "view_file" => "Read",
        "glob" => "Glob",
        "list_dir" | "list_directory" => "Read",
        "grep" | "search" | "search_file" => "Grep",
        "run_terminal_command" | "terminal" | "bash" | "shell" => "Bash",
        "write" | "write_file" | "create_file" => "Write",
        "edit" | "search_replace" | "edit_file" | "replace_file" => "Edit",
        "web search" | "x search" | "web_search" => "WebSearch",
        "web fetch" | "web_fetch" => "WebFetch",
        _ if !raw.is_empty() => raw,
        _ => "GrokTool",
    }
    .to_string()
}

fn safe_tool_input(update: &Value) -> Value {
    let raw = update
        .get("rawInput")
        .or_else(|| update.get("input"))
        .unwrap_or(update);
    let mut projected = safe_tool_input_value(raw);
    let Value::Object(out) = &mut projected else {
        return projected;
    };
    if !out.contains_key("command") {
        if let Some(value) = update.get("command").and_then(safe_scalar) {
            out.insert("command".to_string(), value);
        }
    }
    if !out.contains_key("file_path") {
        if let Some(value) = update.get("path").and_then(safe_scalar) {
            out.insert("file_path".to_string(), value);
        }
    }
    projected
}

fn safe_tool_input_value(raw: &Value) -> Value {
    let mut out = Map::new();
    for key in [
        "command",
        "cmd",
        "path",
        "file_path",
        "target_file",
        "target_directory",
        "directory",
        "dir",
        "cwd",
        "pattern",
        "query",
        "url",
    ] {
        if let Some(value) = raw.get(key).and_then(safe_scalar) {
            out.insert(normalize_input_key(key).to_string(), value);
        }
    }
    Value::Object(out)
}

fn safe_scalar(value: &Value) -> Option<Value> {
    match value {
        Value::String(text) if text.chars().count() <= 4_000 => {
            Some(Value::String(redact_visible_text(text)))
        }
        Value::Number(_) | Value::Bool(_) => Some(value.clone()),
        _ => None,
    }
}

fn normalize_input_key(key: &str) -> &str {
    match key {
        "target_file" | "path" => "file_path",
        "target_directory" | "directory" | "dir" => "path",
        other => other,
    }
}

fn safe_tool_output(update: &Value) -> Option<String> {
    let raw = update.get("rawOutput").or_else(|| update.get("output"))?;
    let candidates = [
        raw.get("content"),
        raw.get("text"),
        raw.get("output_for_prompt"),
        raw.pointer("/FileContent/content_concise"),
        raw.pointer("/FileContent/content"),
        raw.pointer("/EditsApplied/tool_output_for_prompt_concise"),
        raw.pointer("/EditsApplied/tool_output_for_prompt"),
        raw.pointer("/Content/content"),
        raw.pointer("/Result/output"),
        raw.pointer("/Result/message"),
        raw.get("file_matches"),
        raw.get("stdout"),
        raw.get("output"),
        Some(raw).filter(|value| value.is_string()),
    ];

    candidates
        .into_iter()
        .flatten()
        .find_map(output_value_to_text)
        .map(|text| project_tool_output(&text))
}

fn output_value_to_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) if !text.is_empty() => Some(text.clone()),
        Value::Array(items) if items.is_empty() => None,
        Value::Array(items) => {
            let bytes = items
                .iter()
                .map(Value::as_u64)
                .collect::<Option<Vec<_>>>()
                .filter(|values| values.iter().all(|value| *value <= u8::MAX.into()));
            if let Some(bytes) = bytes {
                let bytes = bytes
                    .into_iter()
                    .map(|value| value as u8)
                    .collect::<Vec<_>>();
                return Some(String::from_utf8_lossy(&bytes).into_owned());
            }
            serde_json::to_string(value).ok()
        }
        Value::Object(map) if map.is_empty() => None,
        Value::Object(_) | Value::Number(_) | Value::Bool(_) => serde_json::to_string(value).ok(),
        _ => None,
    }
}

fn safe_chat_tool_output(record: &Value) -> Option<String> {
    record
        .get("content")
        .and_then(Value::as_str)
        .or_else(|| record.pointer("/content/text").and_then(Value::as_str))
        .map(project_tool_output)
}

fn project_tool_output(text: &str) -> String {
    let redacted = redact_visible_text(text);
    let char_count = redacted.chars().count();
    if char_count <= TOOL_OUTPUT_LIMIT_CHARS {
        return redacted;
    }
    let preview: String = redacted.chars().take(TOOL_OUTPUT_PREVIEW_CHARS).collect();
    format!(
        "{preview}\n\n[GAAL_TRUNCATED_TOOL_OUTPUT original_chars={char_count} retained_chars={TOOL_OUTPUT_PREVIEW_CHARS}]"
    )
}

fn redact_visible_text(text: &str) -> String {
    static PATTERNS: OnceLock<Vec<(Regex, &'static str)>> = OnceLock::new();
    let patterns = PATTERNS.get_or_init(|| {
        vec![
            (
                Regex::new(r"\b\d{8,12}:[A-Za-z0-9_-]{30,}\b").expect("telegram token regex"),
                "[REDACTED_TELEGRAM_BOT_TOKEN]",
            ),
            (
                Regex::new(r"\bgh[pousr]_[A-Za-z0-9_]{20,}\b").expect("github token regex"),
                "[REDACTED_GITHUB_TOKEN]",
            ),
            (
                Regex::new(r"\bsk-[A-Za-z0-9_-]{20,}\b").expect("api token regex"),
                "[REDACTED_API_KEY]",
            ),
            (
                Regex::new(r"(?i)\bBearer\s+[A-Za-z0-9._~+/=-]{20,}").expect("bearer token regex"),
                "Bearer [REDACTED_BEARER_TOKEN]",
            ),
            (
                Regex::new(r"\bxox[baprs]-[A-Za-z0-9-]{20,}\b").expect("slack token regex"),
                "[REDACTED_SLACK_TOKEN]",
            ),
        ]
    });

    let mut redacted = text.to_string();
    for (pattern, replacement) in patterns {
        redacted = pattern.replace_all(&redacted, *replacement).into_owned();
    }
    redacted
}

fn attach_result_status(input: &mut Value, update: &Value, status: &str) {
    let Value::Object(map) = input else {
        return;
    };
    if let Some(exit_code) = update
        .pointer("/rawOutput/exit_code")
        .or_else(|| update.pointer("/rawOutput/exitCode"))
        .and_then(Value::as_i64)
    {
        map.insert(RESULT_EXIT_CODE_KEY.to_string(), json!(exit_code));
    }
    map.insert(RESULT_SUCCESS_KEY.to_string(), json!(status == "completed"));
}

fn grok_record_timestamp(record: &Value) -> Option<String> {
    record
        .pointer("/params/_meta/agentTimestampMs")
        .and_then(Value::as_i64)
        .or_else(|| record.get("timestamp_ms").and_then(Value::as_i64))
        .or_else(|| record.get("timestampMs").and_then(Value::as_i64))
        .and_then(timestamp_millis_to_rfc3339)
        .or_else(|| {
            record
                .get("timestamp")
                .and_then(Value::as_i64)
                .and_then(timestamp_auto_to_rfc3339)
        })
        .or_else(|| {
            record
                .get("created_at")
                .and_then(Value::as_str)
                .map(normalize_timestamp)
        })
        .or_else(|| {
            record
                .get("ts")
                .and_then(Value::as_str)
                .map(normalize_timestamp)
        })
}

fn timestamp_auto_to_rfc3339(value: i64) -> Option<String> {
    if value.abs() >= 100_000_000_000 {
        timestamp_millis_to_rfc3339(value)
    } else {
        Utc.timestamp_opt(value, 0)
            .single()
            .map(|dt| dt.to_rfc3339())
    }
}

fn timestamp_millis_to_rfc3339(value: i64) -> Option<String> {
    Utc.timestamp_millis_opt(value)
        .single()
        .map(|dt| dt.to_rfc3339())
}

fn normalize_timestamp(raw: &str) -> String {
    DateTime::parse_from_rfc3339(raw)
        .map(|dt| dt.with_timezone(&Utc).to_rfc3339())
        .unwrap_or_else(|_| raw.to_string())
}

fn read_json(path: &Path) -> Option<Value> {
    let data = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&data).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture_dir(id_suffix: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("grok")
            .join("home")
            .join(".grok")
            .join("sessions")
            .join("%2Ftmp%2Fgaal-grok-fixture")
            .join(format!("019f46d3-0000-7000-8000-0000000000{id_suffix}"))
    }

    fn parse_fixture(id_suffix: &str) -> ParsedSession {
        let id = format!("019f46d3-0000-7000-8000-0000000000{id_suffix}");
        parse_session(&fixture_dir(id_suffix), &id).expect("parse Grok fixture")
    }

    fn all_fact_text(parsed: &ParsedSession) -> String {
        parsed
            .facts
            .iter()
            .filter_map(|fact| fact.detail.as_deref())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn parses_updates_without_private_canaries() {
        let parsed = parse_fixture("01");
        let text = all_fact_text(&parsed);

        assert_eq!(parsed.meta.id, "019f46d3-0000-7000-8000-000000000001");
        assert_eq!(parsed.meta.engine, Engine::Grok);
        assert_eq!(parsed.meta.model.as_deref(), Some("grok-4.5"));
        assert_eq!(parsed.total_turns, 1);
        assert!(text.contains("GROK_VISIBLE_USER_PROMPT_SHOULD_INDEX"));
        assert!(text.contains("GROK_VISIBLE_ASSISTANT_REPLY_SHOULD_INDEX"));
        assert!(!text.contains("GROK_PRIV_THOUGHT_CHUNK_SHOULD_NOT_INDEX"));
        assert!(!text.contains("GROK_PRIV_SUMMARY_SHOULD_NOT_INDEX"));
        assert!(!text.contains("GROK_PRIV_TITLE_SHOULD_NOT_INDEX"));
        assert!(!text.contains("chat fallback should not merge with updates"));
    }

    #[test]
    fn projects_tool_events_safely() {
        let parsed = parse_fixture("02");
        let text = all_fact_text(&parsed);
        let command = parsed
            .facts
            .iter()
            .find(|fact| fact.fact_type.as_str() == "command")
            .expect("command fact");

        assert_eq!(parsed.total_tools, 6);
        assert_eq!(
            command.detail.as_deref(),
            Some("printf GROK_VISIBLE_TOOL_RESULT_SHOULD_INDEX")
        );
        assert_eq!(command.exit_code, Some(0));
        assert_eq!(command.success, Some(true));
        assert!(text.contains("GROK_VISIBLE_TOOL_RESULT_SHOULD_INDEX"));
        assert!(parsed.facts.iter().any(|fact| {
            fact.fact_type.as_str() == "file_read"
                && fact.subject.as_deref() == Some("/tmp/grok-readable.txt")
        }));
        assert!(parsed.facts.iter().any(|fact| {
            fact.fact_type.as_str() == "file_write"
                && fact.subject.as_deref() == Some("/tmp/grok-written.txt")
        }));
        assert!(parsed.facts.iter().any(|fact| {
            fact.fact_type.as_str() == "file_read"
                && fact.subject.as_deref() == Some("/tmp/grok-listable")
        }));
        assert!(parsed.facts.iter().any(|fact| {
            fact.fact_type.as_str() == "error"
                && fact.exit_code == Some(7)
                && fact.success == Some(false)
                && fact
                    .detail
                    .as_deref()
                    .is_some_and(|detail| detail.contains("GROK_VISIBLE_TOOL_FAILURE_SHOULD_INDEX"))
        }));
        assert!(text.contains("GAAL_TRUNCATED_TOOL_OUTPUT"));
        assert!(text.contains("GROK_VISIBLE_OVERSIZE_OUTPUT_SHOULD_INDEX"));
        assert!(!text.contains("GROK_OVERSIZE_TAIL_SHOULD_NOT_INDEX"));
        assert!(!text.contains("GROK_VISIBLE_DUPLICATE_TERMINAL_RESULT_SHOULD_NOT_INDEX"));
        assert!(!text.contains("GROK_VISIBLE_DUPLICATE_SHELL_RESULT_SHOULD_NOT_INDEX"));
        assert!(!text.contains("GROK_PRIV_RAWINPUT_BODY_SHOULD_NOT_INDEX"));
        assert!(!text.contains("GROK_PRIV_RAWOUTPUT_BODY_SHOULD_NOT_INDEX"));
        assert!(!text.contains("GROK_PRIV_READ_BODY_SHOULD_NOT_INDEX"));
        assert!(!text.contains("GROK_PRIV_WRITE_BODY_SHOULD_NOT_INDEX"));
        assert!(!text.contains("GROK_PRIV_LIST_BODY_SHOULD_NOT_INDEX"));
        assert!(!text.contains("do not double index this chat row"));

        let events = parse_events(&fixture_dir("02"), "019f46d3-0000-7000-8000-000000000002")
            .expect("parse tool events");
        let tool_outputs = events
            .iter()
            .filter_map(|event| match &event.kind {
                EventKind::ToolResult { content, .. } => content.as_deref(),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        for canary in [
            "GROK_VISIBLE_TOOL_RESULT_SHOULD_INDEX",
            "GROK_VISIBLE_READ_RESULT_SHOULD_INDEX",
            "GROK_VISIBLE_WRITE_RESULT_SHOULD_INDEX",
            "GROK_VISIBLE_LIST_RESULT_SHOULD_INDEX",
        ] {
            assert!(tool_outputs.contains(canary), "missing {canary}");
        }
        assert!(!tool_outputs.contains("GROK_VISIBLE_DUPLICATE_TERMINAL_RESULT_SHOULD_NOT_INDEX"));
        assert!(!tool_outputs.contains("GROK_VISIBLE_DUPLICATE_SHELL_RESULT_SHOULD_NOT_INDEX"));
    }

    #[test]
    fn falls_back_to_chat_history_when_updates_have_no_visible_events() {
        let parsed = parse_fixture("03");
        let text = all_fact_text(&parsed);

        assert_eq!(parsed.total_turns, 1);
        assert_eq!(parsed.total_tools, 2);
        assert!(text.contains("Fallback user message GROK_VISIBLE_USER_PROMPT_SHOULD_INDEX"));
        assert!(text.contains("Fallback assistant reply GROK_VISIBLE_ASSISTANT_REPLY_SHOULD_INDEX"));
        assert!(parsed.facts.iter().any(|fact| {
            fact.fact_type.as_str() == "file_read"
                && fact.subject.as_deref() == Some("/tmp/grok-chat-readable.txt")
        }));
        assert!(parsed.facts.iter().any(|fact| {
            fact.fact_type.as_str() == "file_write"
                && fact.subject.as_deref() == Some("/tmp/grok-chat-written.txt")
        }));
        assert!(!text.contains("GROK_PRIV_CHAT_READ_BODY_SHOULD_NOT_INDEX"));
        assert!(!text.contains("GROK_PRIV_CHAT_WRITE_BODY_SHOULD_NOT_INDEX"));
        assert!(!text.contains("GROK_PRIV_CHAT_IMAGE_SHOULD_NOT_INDEX"));

        let events = parse_events(&fixture_dir("03"), "019f46d3-0000-7000-8000-000000000003")
            .expect("parse fallback events");
        let tool_outputs = events
            .iter()
            .filter_map(|event| match &event.kind {
                EventKind::ToolResult { content, .. } => content.as_deref(),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(tool_outputs.contains("GROK_VISIBLE_CHAT_TOOL_RESULT_SHOULD_INDEX"));
        assert!(tool_outputs.contains("[REDACTED_TELEGRAM_BOT_TOKEN]"));
        assert!(!tool_outputs.contains("1234567890:ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghi"));
    }

    #[test]
    fn redacts_common_secret_patterns_from_visible_text() {
        let raw = "tg 1234567890:ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghi github ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ1234567890 api sk-ABCDEFGHIJKLMNOPQRSTUVWXYZ1234567890 bearer Bearer abcdefghijklmnopqrstuvwxyz123456 slack xoxb-123456789012345678901234";
        let redacted = redact_visible_text(raw);

        assert!(redacted.contains("[REDACTED_TELEGRAM_BOT_TOKEN]"));
        assert!(redacted.contains("[REDACTED_GITHUB_TOKEN]"));
        assert!(redacted.contains("[REDACTED_API_KEY]"));
        assert!(redacted.contains("Bearer [REDACTED_BEARER_TOKEN]"));
        assert!(redacted.contains("[REDACTED_SLACK_TOKEN]"));
        assert!(!redacted.contains("1234567890:ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghi"));
        assert!(!redacted.contains("ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ1234567890"));
        assert!(!redacted.contains("sk-ABCDEFGHIJKLMNOPQRSTUVWXYZ1234567890"));
        assert!(!redacted.contains("abcdefghijklmnopqrstuvwxyz123456"));
        assert!(!redacted.contains("xoxb-123456789012345678901234"));
    }

    #[test]
    fn caps_projected_tool_output_with_an_explicit_marker() {
        let raw = format!(
            "GROK_VISIBLE_OVERSIZE_OUTPUT_SHOULD_INDEX {} GROK_OVERSIZE_TAIL_SHOULD_NOT_INDEX",
            "x".repeat(TOOL_OUTPUT_LIMIT_CHARS + 1_000)
        );
        let projected = project_tool_output(&raw);

        assert!(projected.contains("GROK_VISIBLE_OVERSIZE_OUTPUT_SHOULD_INDEX"));
        assert!(projected.contains("[GAAL_TRUNCATED_TOOL_OUTPUT"));
        assert!(!projected.contains("GROK_OVERSIZE_TAIL_SHOULD_NOT_INDEX"));
        assert!(projected.chars().count() <= TOOL_OUTPUT_LIMIT_CHARS);
    }

    #[test]
    fn projects_observed_live_raw_output_shapes() {
        let cases = [
            (
                json!({"rawOutput": {"output_for_prompt": "terminal-visible"}}),
                "terminal-visible",
            ),
            (
                json!({"rawOutput": {"FileContent": {"content_concise": "file-visible"}}}),
                "file-visible",
            ),
            (
                json!({"rawOutput": {"EditsApplied": {"tool_output_for_prompt_concise": "edit-visible"}}}),
                "edit-visible",
            ),
            (
                json!({"rawOutput": {"Content": {"content": "directory-visible"}}}),
                "directory-visible",
            ),
            (
                json!({"rawOutput": {"Result": {"output": "background-visible"}}}),
                "background-visible",
            ),
            (
                json!({"rawOutput": {"file_matches": [{"path": "/tmp/a", "matches": ["grep-visible"]}]}}),
                "grep-visible",
            ),
            (
                json!({"rawOutput": {"stdout": [115, 116, 100, 111, 117, 116, 45, 118, 105, 115, 105, 98, 108, 101]}}),
                "stdout-visible",
            ),
            (
                json!({"rawOutput": {"output": [111, 117, 116, 112, 117, 116, 45, 118, 105, 115, 105, 98, 108, 101]}}),
                "output-visible",
            ),
        ];

        for (update, expected) in cases {
            let projected = safe_tool_output(&update).expect("project observed output");
            assert!(projected.contains(expected), "missing {expected}");
        }
    }

    #[test]
    fn normalizes_observed_tool_names_and_directory_input() {
        assert_eq!(normalize_tool_name(&json!({"title": "write"})), "Write");
        assert_eq!(normalize_tool_name(&json!({"title": "Shell"})), "Bash");
        assert_eq!(
            normalize_tool_name(&json!({"title": "Web search:"})),
            "WebSearch"
        );
        assert_eq!(
            safe_tool_input(&json!({
                "title": "list_dir",
                "rawInput": {"target_directory": "/tmp/listable"}
            }))["path"],
            "/tmp/listable"
        );
    }

    #[test]
    fn source_state_reports_malformed_update_and_fallback_records() {
        let updates = source_state(&fixture_dir("02"), "019f46d3-0000-7000-8000-000000000002");
        assert!(updates.artifacts.iter().any(|artifact| {
            artifact.role == "updates"
                && artifact.malformed_count == 1
                && artifact.parse_status == "partial_malformed"
        }));

        let fallback = source_state(&fixture_dir("03"), "019f46d3-0000-7000-8000-000000000003");
        assert!(fallback.artifacts.iter().any(|artifact| {
            artifact.role == "chat_history"
                && artifact.malformed_count == 1
                && artifact.parse_status == "partial_malformed"
        }));
        assert!(fallback.observations.iter().any(|observation| {
            observation.kind == "source_selected"
                && observation.source_role.as_deref() == Some("chat_history")
        }));
    }
}
