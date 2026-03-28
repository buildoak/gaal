//! Facts consumer: converts a canonical event stream into a `ParsedSession`.
//!
//! This is the single consumer that replaces the duplicated fact-construction
//! logic previously embedded in `claude::parse_from_offset` and
//! `codex::parse_from_offset`.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::model::fact::FactType;
use crate::model::Fact;

use super::common::{is_git_command, parse_exit_code, resolve_started_at, tool_call_fact, truncate};
use super::event::{ContentBlock, EventKind, SessionEvent};
use super::types::{Engine, ParsedSession, SessionMeta};

/// Intermediate state for a pending tool call awaiting its result.
#[derive(Debug, Clone)]
struct ToolCallState {
    tool_name: String,
    fact_index: Option<usize>,
    #[allow(dead_code)]
    subject: Option<String>,
    detail: Option<String>,
}

fn is_shell_tool(tool_name: &str) -> bool {
    tool_name.eq_ignore_ascii_case("bash") || tool_name.eq_ignore_ascii_case("exec_command")
}

fn is_non_error_tool(tool_name: &str) -> bool {
    [
        "read",
        "glob",
        "grep",
        "webfetch",
        "websearch",
        "write",
        "edit",
        "notebookedit",
    ]
    .iter()
    .any(|name| tool_name.eq_ignore_ascii_case(name))
}

/// Convert a canonical event stream into a `ParsedSession`.
///
/// Replicates all fact-construction behaviour previously spread across
/// `claude::parse_from_offset` and `codex::parse_from_offset`:
///
/// - Metadata extraction (session_id, model, cwd, version)
/// - Usage deduplication
/// - Turn counting on `UserMessage`
/// - Fact construction (user prompts, assistant replies, tool calls, errors)
/// - Exit-code backfill from `ToolResult` events
/// - Git-op detection
/// - Exit signal extraction
pub fn extract_parsed_session(
    events: &[SessionEvent],
    engine: Engine,
    path: &Path,
) -> ParsedSession {
    // -- Metadata accumulators --
    let mut session_id: Option<String> = None;
    let mut model: Option<String> = None;
    let mut cwd: Option<String> = None;
    let mut version: Option<String> = None;
    let mut started_at: Option<String> = None;
    let mut last_event_at: Option<String> = None;
    let mut last_stop_reason: Option<String> = None;

    // -- Counters --
    let mut total_input_tokens = 0i64;
    let mut total_output_tokens = 0i64;
    let mut cache_read_tokens = 0i64;
    let mut cache_creation_tokens = 0i64;
    let mut reasoning_tokens = 0i64;
    let mut peak_context = 0i64;
    let mut total_tools = 0i32;
    let mut total_turns = 0i32;
    let mut usage_keys_seen: HashSet<String> = HashSet::new();

    // -- Facts --
    let mut facts: Vec<Fact> = Vec::new();
    let mut tool_state_by_id: HashMap<String, ToolCallState> = HashMap::new();

    for event in events {
        // Track timestamps.
        if let Some(ts) = &event.timestamp {
            if started_at.is_none() {
                started_at = Some(ts.clone());
            }
            last_event_at = Some(ts.clone());
        }

        let ts_str = event.timestamp.clone().unwrap_or_default();

        match &event.kind {
            // ── Meta ──────────────────────────────────────────────
            EventKind::Meta {
                session_id: sid,
                model: m,
                cwd: c,
                version: v,
            } => {
                if session_id.is_none() {
                    session_id = sid.clone();
                }
                if model.is_none() {
                    model = m.clone();
                }
                if cwd.is_none() {
                    cwd = c.clone();
                }
                if version.is_none() {
                    version = v.clone();
                }
            }

            // ── Usage ─────────────────────────────────────────────
            EventKind::Usage {
                input_tokens,
                output_tokens,
                cache_read_input_tokens,
                cache_creation_input_tokens,
                reasoning_tokens: evt_reasoning,
                dedup_key,
            } => {
                let should_count = dedup_key
                    .as_ref()
                    .map(|key| usage_keys_seen.insert(key.clone()))
                    .unwrap_or(true);
                if should_count {
                    total_input_tokens += input_tokens;
                    total_output_tokens += output_tokens;
                    cache_read_tokens += cache_read_input_tokens;
                    cache_creation_tokens += cache_creation_input_tokens;
                    reasoning_tokens += evt_reasoning;
                    // Track peak context: full input for the turn (non-cached + cache_read + cache_creation).
                    let full_input = input_tokens + cache_read_input_tokens + cache_creation_input_tokens;
                    if full_input > peak_context {
                        peak_context = full_input;
                    }
                }
            }

            // ── User message ──────────────────────────────────────
            EventKind::UserMessage { content } => {
                total_turns += 1;
                let turn_number = Some(total_turns);

                // Extract first text block as user prompt.
                let prompt_text = content.iter().find_map(|block| match block {
                    ContentBlock::Text(text) => {
                        let trimmed = text.trim();
                        (!trimmed.is_empty()).then(|| trimmed.to_string())
                    }
                    _ => None,
                });

                if let Some(prompt) = prompt_text {
                    facts.push(Fact {
                        id: None,
                        session_id: String::new(),
                        ts: ts_str.clone(),
                        turn_number,
                        fact_type: FactType::UserPrompt,
                        subject: None,
                        detail: Some(prompt),
                        exit_code: None,
                        success: None,
                    });
                }
            }

            // ── Assistant message ─────────────────────────────────
            EventKind::AssistantMessage {
                content,
                model: msg_model,
                ..
            } => {
                let turn_number = if total_turns > 0 {
                    Some(total_turns)
                } else {
                    None
                };

                // Pick up model from assistant message if not yet known.
                if model.is_none() {
                    model = msg_model.clone();
                }

                // One AssistantReply fact per text block (matches old per-block behaviour).
                for block in content {
                    if let ContentBlock::Text(text) = block {
                        let trimmed = text.trim();
                        if !trimmed.is_empty() {
                            facts.push(Fact {
                                id: None,
                                session_id: String::new(),
                                ts: ts_str.clone(),
                                turn_number,
                                fact_type: FactType::AssistantReply,
                                subject: None,
                                detail: Some(trimmed.to_string()),
                                exit_code: None,
                                success: None,
                            });
                        }
                    }
                }
            }

            // ── Tool use ──────────────────────────────────────────
            EventKind::ToolUse(tool_use) => {
                total_tools += 1;
                let turn_number = if total_turns > 0 {
                    Some(total_turns)
                } else {
                    None
                };

                let fact =
                    tool_call_fact(&tool_use.name, &tool_use.input, ts_str.clone(), turn_number);

                let mut state = ToolCallState {
                    tool_name: tool_use.name.clone(),
                    fact_index: None,
                    subject: None,
                    detail: None,
                };

                if let Some(mut call_fact) = fact {
                    // Git-op detection.
                    if matches!(&call_fact.fact_type, FactType::Command) {
                        if let Some(cmd) = call_fact.detail.clone() {
                            if is_git_command(&cmd) {
                                facts.push(Fact {
                                    id: None,
                                    session_id: String::new(),
                                    ts: ts_str.clone(),
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

                if !tool_use.id.is_empty() {
                    tool_state_by_id.insert(tool_use.id.clone(), state);
                }
            }

            // ── Tool result ───────────────────────────────────────
            EventKind::ToolResult {
                tool_use_id,
                content: output_text,
                is_error,
            } => {
                let turn_number = if total_turns > 0 {
                    Some(total_turns)
                } else {
                    None
                };
                let exit_code = parse_exit_code(output_text.as_deref().unwrap_or_default());
                let state = tool_state_by_id.get(tool_use_id).cloned();
                let tool_name = state.as_ref().map(|s| s.tool_name.as_str()).unwrap_or("");
                let is_shell = is_shell_tool(tool_name);
                let is_blocked_tool = is_non_error_tool(tool_name);
                let shell_non_zero_exit = is_shell && exit_code.map(|code| code != 0).unwrap_or(false);

                // Backfill exit_code on the matching tool-call fact.
                if let Some(state) = state.as_ref() {
                    if let Some(fact_idx) = state.fact_index {
                        if let Some(fact) = facts.get_mut(fact_idx) {
                            // Both Claude (Bash) and Codex (Bash, exec_command).
                            if is_shell {
                                fact.exit_code = exit_code;
                                fact.success = Some(exit_code.unwrap_or(0) == 0);
                            }
                        }
                    }
                }

                // AF4: Error facts come only from explicit `is_error` or shell non-zero exits.
                // Certain non-shell tools are explicitly excluded from error classification.
                let should_create_error_fact = !is_blocked_tool && (*is_error || shell_non_zero_exit);
                if should_create_error_fact {
                    facts.push(Fact {
                        id: None,
                        session_id: String::new(),
                        ts: ts_str.clone(),
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

            // ── Stop signal ───────────────────────────────────────
            EventKind::StopSignal { reason } => {
                last_stop_reason = Some(reason.clone());
            }

            // ── Events not relevant to facts ──────────────────────
            EventKind::SubagentProgress { .. }
            | EventKind::SubagentCompletion { .. }
            | EventKind::Summary { .. } => {}
        }
    }

    // -- Resolve session ID --
    let fallback_id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();
    let resolved_id = session_id.unwrap_or(fallback_id);

    // Backfill session_id on all facts.
    for fact in &mut facts {
        fact.session_id = resolved_id.clone();
    }

    let resolved_start = resolve_started_at(started_at, last_event_at.clone());
    ParsedSession {
        meta: SessionMeta {
            id: resolved_id,
            engine,
            model,
            cwd,
            started_at: resolved_start,
            version,
        },
        facts,
        total_input_tokens,
        total_output_tokens,
        cache_read_tokens,
        cache_creation_tokens,
        reasoning_tokens,
        peak_context,
        total_tools,
        total_turns,
        ended_at: last_event_at.clone(),
        exit_signal: last_stop_reason,
        last_event_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::event::{ContentBlock, EventKind, SessionEvent, ToolUseEvent};
    use serde_json::{json, Value};
    use std::path::Path;

    fn meta_event(sid: &str, model: &str, cwd: &str) -> SessionEvent {
        SessionEvent {
            timestamp: Some("2026-03-07T10:00:00Z".to_string()),
            kind: EventKind::Meta {
                session_id: Some(sid.to_string()),
                model: Some(model.to_string()),
                cwd: Some(cwd.to_string()),
                version: Some("1.0.0".to_string()),
            },
        }
    }

    fn user_msg(ts: &str, text: &str) -> SessionEvent {
        SessionEvent {
            timestamp: Some(ts.to_string()),
            kind: EventKind::UserMessage {
                content: vec![ContentBlock::Text(text.to_string())],
            },
        }
    }

    fn assistant_msg(ts: &str, text: &str) -> SessionEvent {
        SessionEvent {
            timestamp: Some(ts.to_string()),
            kind: EventKind::AssistantMessage {
                content: vec![ContentBlock::Text(text.to_string())],
                model: None,
                stop_reason: None,
            },
        }
    }

    fn tool_use_event(ts: &str, id: &str, name: &str, input: Value) -> SessionEvent {
        SessionEvent {
            timestamp: Some(ts.to_string()),
            kind: EventKind::ToolUse(ToolUseEvent {
                id: id.to_string(),
                name: name.to_string(),
                input,
            }),
        }
    }

    fn tool_result_event(
        ts: &str,
        tool_use_id: &str,
        content: &str,
        is_error: bool,
    ) -> SessionEvent {
        SessionEvent {
            timestamp: Some(ts.to_string()),
            kind: EventKind::ToolResult {
                tool_use_id: tool_use_id.to_string(),
                content: Some(content.to_string()),
                is_error,
            },
        }
    }

    fn usage_event(ts: &str, input: i64, output: i64, key: &str) -> SessionEvent {
        SessionEvent {
            timestamp: Some(ts.to_string()),
            kind: EventKind::Usage {
                input_tokens: input,
                output_tokens: output,
                cache_read_input_tokens: 0,
                cache_creation_input_tokens: 0,
                reasoning_tokens: 0,
                dedup_key: Some(key.to_string()),
            },
        }
    }

    fn stop_event(ts: &str, reason: &str) -> SessionEvent {
        SessionEvent {
            timestamp: Some(ts.to_string()),
            kind: EventKind::StopSignal {
                reason: reason.to_string(),
            },
        }
    }

    #[test]
    fn empty_events_produce_minimal_session() {
        let result = extract_parsed_session(&[], Engine::Claude, Path::new("test.jsonl"));
        assert_eq!(result.meta.id, "test");
        assert_eq!(result.facts.len(), 0);
        assert_eq!(result.total_turns, 0);
        assert_eq!(result.total_tools, 0);
    }

    #[test]
    fn metadata_extraction() {
        let events = vec![meta_event("abc123", "opus", "/home/user")];
        let result = extract_parsed_session(&events, Engine::Claude, Path::new("test.jsonl"));
        assert_eq!(result.meta.id, "abc123");
        assert_eq!(result.meta.model, Some("opus".to_string()));
        assert_eq!(result.meta.cwd, Some("/home/user".to_string()));
        assert_eq!(result.meta.version, Some("1.0.0".to_string()));
    }

    #[test]
    fn first_meta_wins() {
        let events = vec![
            meta_event("first", "opus", "/a"),
            meta_event("second", "sonnet", "/b"),
        ];
        let result = extract_parsed_session(&events, Engine::Claude, Path::new("test.jsonl"));
        assert_eq!(result.meta.id, "first");
        assert_eq!(result.meta.model, Some("opus".to_string()));
    }

    #[test]
    fn fallback_session_id_from_path() {
        let events = vec![SessionEvent {
            timestamp: Some("2026-03-07T10:00:00Z".to_string()),
            kind: EventKind::Meta {
                session_id: None,
                model: None,
                cwd: None,
                version: None,
            },
        }];
        let result =
            extract_parsed_session(&events, Engine::Claude, Path::new("/path/to/abc123.jsonl"));
        assert_eq!(result.meta.id, "abc123");
    }

    #[test]
    fn user_message_creates_turn_and_prompt_fact() {
        let events = vec![user_msg("2026-03-07T10:00:00Z", "Hello world")];
        let result = extract_parsed_session(&events, Engine::Claude, Path::new("test.jsonl"));
        assert_eq!(result.total_turns, 1);
        assert_eq!(result.facts.len(), 1);
        assert_eq!(result.facts[0].fact_type.as_str(), "user_prompt");
        assert_eq!(result.facts[0].detail, Some("Hello world".to_string()));
    }

    #[test]
    fn empty_user_message_no_fact() {
        let events = vec![SessionEvent {
            timestamp: Some("2026-03-07T10:00:00Z".to_string()),
            kind: EventKind::UserMessage {
                content: vec![ContentBlock::Text("   ".to_string())],
            },
        }];
        let result = extract_parsed_session(&events, Engine::Claude, Path::new("test.jsonl"));
        assert_eq!(result.total_turns, 1);
        assert_eq!(result.facts.len(), 0); // Whitespace-only should not create a fact
    }

    #[test]
    fn assistant_message_creates_reply_fact() {
        let events = vec![
            user_msg("2026-03-07T10:00:00Z", "question"),
            assistant_msg("2026-03-07T10:01:00Z", "answer"),
        ];
        let result = extract_parsed_session(&events, Engine::Claude, Path::new("test.jsonl"));
        let reply_facts: Vec<_> = result
            .facts
            .iter()
            .filter(|f| f.fact_type.as_str() == "assistant_reply")
            .collect();
        assert_eq!(reply_facts.len(), 1);
        assert_eq!(reply_facts[0].detail, Some("answer".to_string()));
    }

    #[test]
    fn assistant_reply_preserves_full_content() {
        let long_text = "x".repeat(5000);
        let events = vec![assistant_msg("2026-03-07T10:00:00Z", &long_text)];
        let result = extract_parsed_session(&events, Engine::Claude, Path::new("test.jsonl"));
        let detail = result.facts[0].detail.as_ref().unwrap();
        assert_eq!(detail.len(), 5000, "full assistant reply content should be preserved");
    }

    #[test]
    fn tool_use_creates_fact_and_counts() {
        let events = vec![tool_use_event(
            "2026-03-07T10:00:00Z",
            "call_1",
            "Read",
            json!({"file_path": "/src/main.rs"}),
        )];
        let result = extract_parsed_session(&events, Engine::Claude, Path::new("test.jsonl"));
        assert_eq!(result.total_tools, 1);
        let read_facts: Vec<_> = result
            .facts
            .iter()
            .filter(|f| f.fact_type.as_str() == "file_read")
            .collect();
        assert_eq!(read_facts.len(), 1);
        assert_eq!(read_facts[0].subject, Some("/src/main.rs".to_string()));
    }

    #[test]
    fn bash_tool_creates_command_fact() {
        let events = vec![tool_use_event(
            "2026-03-07T10:00:00Z",
            "call_1",
            "Bash",
            json!({"command": "ls -la"}),
        )];
        let result = extract_parsed_session(&events, Engine::Claude, Path::new("test.jsonl"));
        let cmd_facts: Vec<_> = result
            .facts
            .iter()
            .filter(|f| f.fact_type.as_str() == "command")
            .collect();
        assert_eq!(cmd_facts.len(), 1);
        assert_eq!(cmd_facts[0].detail, Some("ls -la".to_string()));
    }

    #[test]
    fn git_command_creates_git_op_fact() {
        let events = vec![tool_use_event(
            "2026-03-07T10:00:00Z",
            "call_1",
            "Bash",
            json!({"command": "git commit -m 'test'"}),
        )];
        let result = extract_parsed_session(&events, Engine::Claude, Path::new("test.jsonl"));
        let git_facts: Vec<_> = result
            .facts
            .iter()
            .filter(|f| f.fact_type.as_str() == "git_op")
            .collect();
        assert_eq!(git_facts.len(), 1);
    }

    #[test]
    fn write_tool_creates_file_write_fact() {
        let events = vec![tool_use_event(
            "2026-03-07T10:00:00Z",
            "call_1",
            "Write",
            json!({"file_path": "/src/new.rs", "content": "fn main() {}"}),
        )];
        let result = extract_parsed_session(&events, Engine::Claude, Path::new("test.jsonl"));
        let write_facts: Vec<_> = result
            .facts
            .iter()
            .filter(|f| f.fact_type.as_str() == "file_write")
            .collect();
        assert_eq!(write_facts.len(), 1);
        assert_eq!(write_facts[0].subject, Some("/src/new.rs".to_string()));
    }

    #[test]
    fn tool_result_backfills_exit_code_on_bash() {
        let events = vec![
            tool_use_event(
                "2026-03-07T10:00:00Z",
                "call_1",
                "Bash",
                json!({"command": "echo hi"}),
            ),
            tool_result_event(
                "2026-03-07T10:01:00Z",
                "call_1",
                "hi\nProcess exited with code 0",
                false,
            ),
        ];
        let result = extract_parsed_session(&events, Engine::Claude, Path::new("test.jsonl"));
        let cmd_fact = result
            .facts
            .iter()
            .find(|f| f.fact_type.as_str() == "command")
            .unwrap();
        assert_eq!(cmd_fact.exit_code, Some(0));
        assert_eq!(cmd_fact.success, Some(true));
    }

    #[test]
    fn tool_result_backfills_exit_code_on_exec_command() {
        let events = vec![
            tool_use_event(
                "2026-03-07T10:00:00Z",
                "call_1",
                "exec_command",
                json!({"command": "ls"}),
            ),
            tool_result_event("2026-03-07T10:01:00Z", "call_1", "Exit code 1", false),
        ];
        let result = extract_parsed_session(&events, Engine::Codex, Path::new("test.jsonl"));
        let cmd_fact = result
            .facts
            .iter()
            .find(|f| f.fact_type.as_str() == "command")
            .unwrap();
        assert_eq!(cmd_fact.exit_code, Some(1));
        assert_eq!(cmd_fact.success, Some(false));
    }

    #[test]
    fn error_tool_result_creates_error_fact() {
        let events = vec![
            tool_use_event(
                "2026-03-07T10:00:00Z",
                "call_1",
                "Bash",
                json!({"command": "bad"}),
            ),
            tool_result_event("2026-03-07T10:01:00Z", "call_1", "error: not found", true),
        ];
        let result = extract_parsed_session(&events, Engine::Claude, Path::new("test.jsonl"));
        let error_facts: Vec<_> = result
            .facts
            .iter()
            .filter(|f| f.fact_type.as_str() == "error")
            .collect();
        assert_eq!(error_facts.len(), 1);
        assert_eq!(error_facts[0].success, Some(false));
    }

    #[test]
    fn output_containing_error_creates_error_fact() {
        let events = vec![
            tool_use_event(
                "2026-03-07T10:00:00Z",
                "call_1",
                "Bash",
                json!({"command": "compile"}),
            ),
            tool_result_event(
                "2026-03-07T10:01:00Z",
                "call_1",
                "compilation failed with error\nProcess exited with code 1",
                false,
            ),
        ];
        let result = extract_parsed_session(&events, Engine::Claude, Path::new("test.jsonl"));
        let error_facts: Vec<_> = result
            .facts
            .iter()
            .filter(|f| f.fact_type.as_str() == "error")
            .collect();
        assert_eq!(error_facts.len(), 1);
    }

    #[test]
    fn non_shell_error_text_does_not_create_error_fact() {
        let events = vec![
            tool_use_event(
                "2026-03-07T10:00:00Z",
                "call_1",
                "Read",
                json!({"file_path": "/tmp/data.txt"}),
            ),
            tool_result_event(
                "2026-03-07T10:01:00Z",
                "call_1",
                "The file contains the phrase: failed to compile",
                false,
            ),
        ];
        let result = extract_parsed_session(&events, Engine::Claude, Path::new("test.jsonl"));
        let error_facts: Vec<_> = result
            .facts
            .iter()
            .filter(|f| f.fact_type.as_str() == "error")
            .collect();
        assert_eq!(error_facts.len(), 0);
    }

    #[test]
    fn excluded_tool_never_creates_error_fact_even_with_is_error_true() {
        let events = vec![
            tool_use_event(
                "2026-03-07T10:00:00Z",
                "call_1",
                "WebSearch",
                json!({"query": "rust error handling"}),
            ),
            tool_result_event(
                "2026-03-07T10:01:00Z",
                "call_1",
                "request failed",
                true,
            ),
        ];
        let result = extract_parsed_session(&events, Engine::Claude, Path::new("test.jsonl"));
        let error_facts: Vec<_> = result
            .facts
            .iter()
            .filter(|f| f.fact_type.as_str() == "error")
            .collect();
        assert_eq!(error_facts.len(), 0);
    }

    #[test]
    fn usage_deduplication() {
        let events = vec![
            usage_event("2026-03-07T10:00:00Z", 100, 200, "msg_1"),
            usage_event("2026-03-07T10:01:00Z", 100, 200, "msg_1"), // duplicate
            usage_event("2026-03-07T10:02:00Z", 50, 100, "msg_2"),
        ];
        let result = extract_parsed_session(&events, Engine::Claude, Path::new("test.jsonl"));
        assert_eq!(result.total_input_tokens, 150); // 100 + 50, not 250
        assert_eq!(result.total_output_tokens, 300); // 200 + 100, not 500
    }

    #[test]
    fn usage_without_dedup_key_always_counted() {
        let events = vec![
            SessionEvent {
                timestamp: Some("2026-03-07T10:00:00Z".to_string()),
                kind: EventKind::Usage {
                    input_tokens: 100,
                    output_tokens: 200,
                    cache_read_input_tokens: 0,
                    cache_creation_input_tokens: 0,
                    reasoning_tokens: 0,
                    dedup_key: None,
                },
            },
            SessionEvent {
                timestamp: Some("2026-03-07T10:01:00Z".to_string()),
                kind: EventKind::Usage {
                    input_tokens: 50,
                    output_tokens: 100,
                    cache_read_input_tokens: 0,
                    cache_creation_input_tokens: 0,
                    reasoning_tokens: 0,
                    dedup_key: None,
                },
            },
        ];
        let result = extract_parsed_session(&events, Engine::Claude, Path::new("test.jsonl"));
        assert_eq!(result.total_input_tokens, 150);
        assert_eq!(result.total_output_tokens, 300);
    }

    #[test]
    fn stop_signal_captured() {
        let events = vec![stop_event("2026-03-07T10:00:00Z", "end_turn")];
        let result = extract_parsed_session(&events, Engine::Claude, Path::new("test.jsonl"));
        assert_eq!(result.exit_signal, Some("end_turn".to_string()));
    }

    #[test]
    fn last_stop_signal_wins() {
        let events = vec![
            stop_event("2026-03-07T10:00:00Z", "first"),
            stop_event("2026-03-07T10:01:00Z", "last"),
        ];
        let result = extract_parsed_session(&events, Engine::Claude, Path::new("test.jsonl"));
        assert_eq!(result.exit_signal, Some("last".to_string()));
    }

    #[test]
    fn timestamps_tracked_correctly() {
        let events = vec![
            user_msg("2026-03-07T10:00:00Z", "first"),
            assistant_msg("2026-03-07T10:30:00Z", "reply"),
            user_msg("2026-03-07T11:00:00Z", "second"),
        ];
        let result = extract_parsed_session(&events, Engine::Claude, Path::new("test.jsonl"));
        assert_eq!(result.meta.started_at, "2026-03-07T10:00:00Z");
        assert_eq!(
            result.last_event_at,
            Some("2026-03-07T11:00:00Z".to_string())
        );
        assert_eq!(result.ended_at, Some("2026-03-07T11:00:00Z".to_string()));
    }

    #[test]
    fn session_id_backfilled_on_all_facts() {
        let events = vec![
            meta_event("session_xyz", "opus", "/home"),
            user_msg("2026-03-07T10:00:00Z", "hello"),
            assistant_msg("2026-03-07T10:01:00Z", "world"),
        ];
        let result = extract_parsed_session(&events, Engine::Claude, Path::new("test.jsonl"));
        for fact in &result.facts {
            assert_eq!(fact.session_id, "session_xyz");
        }
    }

    #[test]
    fn turn_numbers_assigned_correctly() {
        let events = vec![
            user_msg("2026-03-07T10:00:00Z", "turn 1"),
            assistant_msg("2026-03-07T10:01:00Z", "reply 1"),
            user_msg("2026-03-07T10:02:00Z", "turn 2"),
            assistant_msg("2026-03-07T10:03:00Z", "reply 2"),
        ];
        let result = extract_parsed_session(&events, Engine::Claude, Path::new("test.jsonl"));
        assert_eq!(result.total_turns, 2);

        let prompts: Vec<_> = result
            .facts
            .iter()
            .filter(|f| f.fact_type.as_str() == "user_prompt")
            .collect();
        assert_eq!(prompts[0].turn_number, Some(1));
        assert_eq!(prompts[1].turn_number, Some(2));
    }

    #[test]
    fn task_tool_creates_task_spawn_fact() {
        let events = vec![tool_use_event(
            "2026-03-07T10:00:00Z",
            "call_1",
            "Task",
            json!({"prompt": "do something", "description": "a task"}),
        )];
        let result = extract_parsed_session(&events, Engine::Claude, Path::new("test.jsonl"));
        let task_facts: Vec<_> = result
            .facts
            .iter()
            .filter(|f| f.fact_type.as_str() == "task_spawn")
            .collect();
        assert_eq!(task_facts.len(), 1);
    }

    #[test]
    fn engine_preserved() {
        let result = extract_parsed_session(&[], Engine::Codex, Path::new("test.jsonl"));
        assert!(matches!(result.meta.engine, Engine::Codex));

        let result2 = extract_parsed_session(&[], Engine::Claude, Path::new("test.jsonl"));
        assert!(matches!(result2.meta.engine, Engine::Claude));
    }

    #[test]
    fn full_conversation_pipeline() {
        let events = vec![
            meta_event("sess_1", "claude-opus-4", "/project"),
            usage_event("2026-03-07T10:00:00Z", 500, 1000, "msg_1"),
            user_msg("2026-03-07T10:00:00Z", "Build a web server"),
            assistant_msg("2026-03-07T10:01:00Z", "I'll create a web server for you."),
            tool_use_event(
                "2026-03-07T10:02:00Z",
                "call_1",
                "Write",
                json!({"file_path": "/src/main.rs", "content": "fn main() {}"}),
            ),
            tool_result_event("2026-03-07T10:02:00Z", "call_1", "File written", false),
            tool_use_event(
                "2026-03-07T10:03:00Z",
                "call_2",
                "Bash",
                json!({"command": "cargo build"}),
            ),
            tool_result_event(
                "2026-03-07T10:03:00Z",
                "call_2",
                "Compiling...\nProcess exited with code 0",
                false,
            ),
            tool_use_event(
                "2026-03-07T10:04:00Z",
                "call_3",
                "Bash",
                json!({"command": "git commit -m 'init'"}),
            ),
            tool_result_event(
                "2026-03-07T10:04:00Z",
                "call_3",
                "Process exited with code 0",
                false,
            ),
            stop_event("2026-03-07T10:05:00Z", "end_turn"),
        ];
        let result = extract_parsed_session(&events, Engine::Claude, Path::new("test.jsonl"));

        assert_eq!(result.meta.id, "sess_1");
        assert_eq!(result.total_turns, 1);
        assert_eq!(result.total_tools, 3);
        assert_eq!(result.total_input_tokens, 500);
        assert_eq!(result.total_output_tokens, 1000);
        assert_eq!(result.exit_signal, Some("end_turn".to_string()));

        // Should have: user_prompt, assistant_reply, file_write, command, git_op, command
        assert!(result
            .facts
            .iter()
            .any(|f| f.fact_type.as_str() == "user_prompt"));
        assert!(result
            .facts
            .iter()
            .any(|f| f.fact_type.as_str() == "assistant_reply"));
        assert!(result
            .facts
            .iter()
            .any(|f| f.fact_type.as_str() == "file_write"));
        assert!(result
            .facts
            .iter()
            .any(|f| f.fact_type.as_str() == "git_op"));
        assert!(result
            .facts
            .iter()
            .any(|f| f.fact_type.as_str() == "command"));
    }

    #[test]
    fn subagent_events_ignored_for_facts() {
        let events = vec![SessionEvent {
            timestamp: Some("2026-03-07T10:00:00Z".to_string()),
            kind: EventKind::SubagentProgress {
                agent_id: "agent_1".to_string(),
                prompt: "do stuff".to_string(),
                message: None,
                timestamp: None,
                total_tokens: Some(1000),
                total_duration_ms: Some(5000),
                total_tool_use_count: Some(3),
            },
        }];
        let result = extract_parsed_session(&events, Engine::Claude, Path::new("test.jsonl"));
        assert_eq!(result.facts.len(), 0);
    }

    #[test]
    fn summary_events_ignored_for_facts() {
        let events = vec![SessionEvent {
            timestamp: Some("2026-03-07T10:00:00Z".to_string()),
            kind: EventKind::Summary {
                text: "Session summary here".to_string(),
            },
        }];
        let result = extract_parsed_session(&events, Engine::Claude, Path::new("test.jsonl"));
        assert_eq!(result.facts.len(), 0);
    }

    #[test]
    fn no_read_fact_for_unrecognized_tool() {
        let events = vec![tool_use_event(
            "2026-03-07T10:00:00Z",
            "call_1",
            "UnknownTool",
            json!({"arg": "value"}),
        )];
        let result = extract_parsed_session(&events, Engine::Claude, Path::new("test.jsonl"));
        assert_eq!(result.total_tools, 1); // Still counted
        assert_eq!(result.facts.len(), 0); // But no fact created for unknown tools
    }
}
