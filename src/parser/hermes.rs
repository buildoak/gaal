use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{named_params, OptionalExtension};
use serde_json::{json, Value};

use super::event::{ContentBlock, EventKind, SessionEvent, ToolUseEvent};
use super::facts;
use super::types::{Engine, ParsedSession};
use crate::discovery::hermes::{open_readonly, unix_real_to_rfc3339};

#[derive(Debug)]
struct HermesSessionRow {
    id: String,
    source: String,
    model: Option<String>,
    parent_session_id: Option<String>,
    started_at: f64,
    ended_at: Option<f64>,
    end_reason: Option<String>,
    message_count: i64,
    tool_call_count: i64,
    input_tokens: i64,
    output_tokens: i64,
    cache_read_tokens: i64,
    cache_write_tokens: i64,
    reasoning_tokens: i64,
    title: Option<String>,
}

#[derive(Debug)]
struct HermesMessageRow {
    role: String,
    content: Option<String>,
    tool_call_id: Option<String>,
    tool_calls: Option<String>,
    tool_name: Option<String>,
    timestamp: f64,
    finish_reason: Option<String>,
}

pub fn parse_session(db_path: &Path, session_id: &str) -> Result<ParsedSession> {
    let events = parse_events(db_path, session_id)?;
    Ok(facts::extract_parsed_session(
        &events,
        Engine::Hermes,
        Path::new(session_id),
    ))
}

pub fn parse_events(db_path: &Path, session_id: &str) -> Result<Vec<SessionEvent>> {
    let conn = open_readonly(db_path)
        .with_context(|| format!("failed to open Hermes state DB: {}", db_path.display()))?;
    let session = load_session(&conn, session_id)?;
    let messages = load_messages(&conn, session_id)?;

    let mut events = Vec::new();
    let started_at = unix_real_to_rfc3339(session.started_at);
    events.push(SessionEvent {
        timestamp: Some(started_at.clone()),
        kind: EventKind::Meta {
            session_id: Some(session.id.clone()),
            model: session.model.clone(),
            cwd: Some(format!("hermes:{}", session.source)),
            version: None,
            forked_from_id: session.parent_session_id.clone(),
            agent_role: None,
            agent_nickname: None,
        },
    });
    events.push(SessionEvent {
        timestamp: Some(started_at.clone()),
        kind: EventKind::Usage {
            input_tokens: session.input_tokens,
            output_tokens: session.output_tokens,
            cache_read_input_tokens: session.cache_read_tokens,
            cache_creation_input_tokens: session.cache_write_tokens,
            reasoning_tokens: session.reasoning_tokens,
            dedup_key: Some(format!("hermes-session-usage:{}", session.id)),
        },
    });
    if let Some(title) = session
        .title
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        events.push(SessionEvent {
            timestamp: Some(started_at.clone()),
            kind: EventKind::Summary {
                text: title.to_string(),
            },
        });
    }

    for message in messages {
        let ts = Some(unix_real_to_rfc3339(message.timestamp));
        match message.role.as_str() {
            "user" => {
                events.push(SessionEvent {
                    timestamp: ts,
                    kind: EventKind::UserMessage {
                        content: message_text_blocks(message.content),
                    },
                });
            }
            "assistant" => {
                let stop_reason = message.finish_reason.clone();
                let tool_uses = parse_tool_uses(message.tool_calls.as_deref())?;
                events.push(SessionEvent {
                    timestamp: ts.clone(),
                    kind: EventKind::AssistantMessage {
                        content: message_text_blocks(message.content),
                        model: session.model.clone(),
                        stop_reason,
                    },
                });
                for tool_use in tool_uses {
                    events.push(SessionEvent {
                        timestamp: ts.clone(),
                        kind: EventKind::ToolUse(tool_use),
                    });
                }
            }
            "tool" => {
                events.push(SessionEvent {
                    timestamp: ts,
                    kind: EventKind::ToolResult {
                        tool_use_id: message.tool_call_id.unwrap_or_default(),
                        content: message.content,
                        is_error: false,
                        tool_name: message.tool_name.map(|name| normalize_tool_name(&name)),
                        tool_input: None,
                    },
                });
            }
            "session_meta" => {
                if let Some(content) = message.content {
                    events.push(SessionEvent {
                        timestamp: ts,
                        kind: EventKind::Summary {
                            text: summarize_session_meta(&content),
                        },
                    });
                }
            }
            _ => {}
        }
    }

    let end_ts = session
        .ended_at
        .map(unix_real_to_rfc3339)
        .or_else(|| {
            events
                .iter()
                .rev()
                .find_map(|event| event.timestamp.clone())
        })
        .unwrap_or(started_at);
    if let Some(reason) = session.end_reason {
        events.push(SessionEvent {
            timestamp: Some(end_ts.clone()),
            kind: EventKind::StopSignal { reason },
        });
    }

    if events
        .iter()
        .all(|event| !matches!(event.kind, EventKind::UserMessage { .. }))
        && (session.message_count > 0 || session.tool_call_count > 0)
    {
        events.push(SessionEvent {
            timestamp: Some(end_ts),
            kind: EventKind::Summary {
                text: format!(
                    "Hermes session contained {} messages and {} tool calls but no user turns.",
                    session.message_count, session.tool_call_count
                ),
            },
        });
    }

    Ok(events)
}

fn load_session(conn: &rusqlite::Connection, session_id: &str) -> Result<HermesSessionRow> {
    conn.query_row(
        r#"
        SELECT id, source, model, parent_session_id, started_at, ended_at, end_reason,
               message_count, tool_call_count, input_tokens, output_tokens,
               cache_read_tokens, cache_write_tokens, reasoning_tokens, title
        FROM sessions
        WHERE id = :id
        "#,
        named_params! { ":id": session_id },
        |row| {
            Ok(HermesSessionRow {
                id: row.get(0)?,
                source: row.get(1)?,
                model: row.get(2)?,
                parent_session_id: row.get(3)?,
                started_at: row.get(4)?,
                ended_at: row.get(5)?,
                end_reason: row.get(6)?,
                message_count: row.get(7)?,
                tool_call_count: row.get(8)?,
                input_tokens: row.get(9)?,
                output_tokens: row.get(10)?,
                cache_read_tokens: row.get(11)?,
                cache_write_tokens: row.get(12)?,
                reasoning_tokens: row.get(13)?,
                title: row.get(14)?,
            })
        },
    )
    .optional()?
    .with_context(|| format!("Hermes session not found: {session_id}"))
}

fn load_messages(conn: &rusqlite::Connection, session_id: &str) -> Result<Vec<HermesMessageRow>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT role, content, tool_call_id, tool_calls, tool_name, timestamp, finish_reason
        FROM messages
        WHERE session_id = :session_id
        ORDER BY timestamp ASC, id ASC
        "#,
    )?;
    let rows = stmt.query_map(named_params! { ":session_id": session_id }, |row| {
        Ok(HermesMessageRow {
            role: row.get(0)?,
            content: row.get(1)?,
            tool_call_id: row.get(2)?,
            tool_calls: row.get(3)?,
            tool_name: row.get(4)?,
            timestamp: row.get(5)?,
            finish_reason: row.get(6)?,
        })
    })?;

    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

fn message_text_blocks(content: Option<String>) -> Vec<ContentBlock> {
    content
        .map(|text| vec![ContentBlock::Text(text)])
        .unwrap_or_default()
}

fn parse_tool_uses(raw: Option<&str>) -> Result<Vec<ToolUseEvent>> {
    let Some(raw) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(Vec::new());
    };
    let value: Value = serde_json::from_str(raw)
        .with_context(|| "failed to parse Hermes assistant tool_calls JSON")?;
    let Some(items) = value.as_array() else {
        return Ok(Vec::new());
    };

    let mut out = Vec::new();
    for item in items {
        let Some(function) = item.get("function").and_then(Value::as_object) else {
            continue;
        };
        let Some(raw_name) = function.get("name").and_then(Value::as_str) else {
            continue;
        };
        let id = item
            .get("call_id")
            .or_else(|| item.get("id"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let input = parse_tool_arguments(function.get("arguments"));
        out.push(ToolUseEvent {
            id,
            name: normalize_tool_name(raw_name),
            input,
        });
    }
    Ok(out)
}

fn parse_tool_arguments(value: Option<&Value>) -> Value {
    match value {
        Some(Value::String(raw)) => serde_json::from_str::<Value>(raw).unwrap_or_else(|_| {
            json!({
                "raw_arguments": raw
            })
        }),
        Some(value) => value.clone(),
        None => json!({}),
    }
}

fn normalize_tool_name(name: &str) -> String {
    match name {
        "terminal" | "execute_code" => "Bash",
        "read_file" => "Read",
        "write_file" => "Write",
        "patch" => "apply_patch",
        "search_files" => "Grep",
        "browser_navigate" | "browser_snapshot" | "browser_click" | "browser_type"
        | "browser_console" | "browser_vision" => "web_fetch",
        "session_search" => "web_search",
        other => other,
    }
    .to_string()
}

fn summarize_session_meta(content: &str) -> String {
    match serde_json::from_str::<Value>(content) {
        Ok(value) => {
            let keys = value
                .as_object()
                .map(|obj| obj.keys().cloned().collect::<Vec<_>>().join(", "))
                .unwrap_or_else(|| "non-object".to_string());
            format!("Hermes session metadata keys: {keys}")
        }
        Err(_) => "Hermes session metadata record".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn fixture_db() -> std::path::PathBuf {
        let db_path = std::env::temp_dir().join(format!(
            "gaal-hermes-parser-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time")
                .as_nanos()
        ));
        let conn = Connection::open(&db_path).expect("open fixture db");
        conn.execute_batch(include_str!("../../tests/fixtures/hermes/state.sql"))
            .expect("load fixture sql");
        db_path
    }

    #[test]
    fn parses_cli_session_with_tool_call_and_usage() {
        let db_path = fixture_db();
        let parsed = parse_session(&db_path, "20260504_101010_a1b2c3").expect("parse session");
        std::fs::remove_file(&db_path).ok();

        assert_eq!(parsed.meta.id, "20260504_101010_a1b2c3");
        assert_eq!(parsed.meta.engine, Engine::Hermes);
        assert_eq!(parsed.meta.model.as_deref(), Some("hermes-test-model"));
        assert_eq!(parsed.meta.cwd.as_deref(), Some("hermes:cli"));
        assert_eq!(parsed.total_input_tokens, 120);
        assert_eq!(parsed.total_output_tokens, 45);
        assert_eq!(parsed.cache_read_tokens, 10);
        assert_eq!(parsed.cache_creation_tokens, 5);
        assert_eq!(parsed.reasoning_tokens, 7);
        assert_eq!(parsed.total_tools, 1);
        assert!(parsed
            .facts
            .iter()
            .any(|fact| fact.fact_type.as_str() == "command"
                && fact.subject.as_deref() == Some("printf hermes_fixture_alpha")));
        assert!(parsed
            .facts
            .iter()
            .any(|fact| fact.fact_type.as_str() == "user_prompt"
                && fact
                    .detail
                    .as_deref()
                    .is_some_and(|detail| detail.contains("hermes_fixture_alpha"))));
    }

    #[test]
    fn preserves_parent_session_id_for_continuations() {
        let db_path = fixture_db();
        let parsed = parse_session(&db_path, "20260504_141414_childbb").expect("parse session");
        std::fs::remove_file(&db_path).ok();

        assert_eq!(
            parsed.meta.forked_from_id.as_deref(),
            Some("20260504_131313_parentaa")
        );
        assert_eq!(parsed.total_turns, 1);
    }
}
