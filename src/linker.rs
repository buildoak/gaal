use crate::parser::event::{EventKind, SessionEvent};
use serde_json::{de::Deserializer, Value};
use std::collections::HashSet;
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ChildLink {
    pub child_session_id: String,
    pub engine: String,
    pub model: Option<String>,
}
/// Extract child session links from agent-mux Bash tool results.
pub fn extract_child_links(events: &[SessionEvent]) -> Vec<ChildLink> {
    let mut tracked_tool_uses = HashSet::new();
    let mut links = Vec::new();
    for event in events {
        match &event.kind {
            EventKind::ToolUse(tool_use) => {
                let command = tool_use.input.get("command").and_then(Value::as_str);
                if tool_use.name.eq_ignore_ascii_case("Bash")
                    && command.is_some_and(|cmd| cmd.contains("agent-mux"))
                {
                    tracked_tool_uses.insert(tool_use.id.clone());
                }
            }
            EventKind::ToolResult {
                tool_use_id,
                content,
                ..
            } if tracked_tool_uses.contains(tool_use_id) => {
                let Some(raw) = content.as_deref() else {
                    continue;
                };
                let Some(json_start) = raw.find('{') else {
                    continue;
                };
                let mut stream = Deserializer::from_str(&raw[json_start..]).into_iter::<Value>();
                let Some(Ok(payload)) = stream.next() else {
                    continue;
                };
                if !payload
                    .get("success")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    continue;
                }
                let Some(engine) = payload.get("engine").and_then(Value::as_str) else {
                    continue;
                };
                let Some(metadata) = payload.get("metadata").and_then(Value::as_object) else {
                    continue;
                };
                let Some(child_session_id) = metadata
                    .get("session_id")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|sid| !sid.is_empty())
                else {
                    continue;
                };
                let model = metadata
                    .get("model")
                    .and_then(Value::as_str)
                    .map(ToString::to_string);
                links.push(ChildLink {
                    child_session_id: child_session_id.to_string(),
                    engine: engine.to_string(),
                    model,
                });
            }
            _ => {}
        }
    }
    links
}

#[cfg(test)]
mod tests {
    use super::extract_child_links;
    use crate::parser::event::{EventKind, SessionEvent, ToolUseEvent};
    use serde_json::json;
    fn bash_use(id: &str, cmd: &str) -> SessionEvent {
        SessionEvent {
            timestamp: None,
            kind: EventKind::ToolUse(ToolUseEvent {
                id: id.to_string(),
                name: "Bash".to_string(),
                input: json!({ "command": cmd }),
            }),
        }
    }
    fn tool_result(id: &str, content: &str) -> SessionEvent {
        SessionEvent {
            timestamp: None,
            kind: EventKind::ToolResult {
                tool_use_id: id.to_string(),
                content: Some(content.to_string()),
                is_error: false,
            },
        }
    }
    #[test]
    fn valid_json_extracts_link() {
        let events = vec![
            bash_use("1", "agent-mux run"),
            tool_result(
                "1",
                r#"prefix {"success":true,"engine":"codex","metadata":{"session_id":"abc123","model":"gpt-5"}} suffix"#,
            ),
        ];
        let links = extract_child_links(&events);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].child_session_id, "abc123");
    }
    #[test]
    fn malformed_json_returns_empty() {
        let events = vec![
            bash_use("1", "agent-mux run"),
            tool_result("1", "oops {bad"),
        ];
        assert!(extract_child_links(&events).is_empty());
    }
    #[test]
    fn missing_session_id_returns_empty() {
        let events = vec![
            bash_use("1", "agent-mux run"),
            tool_result(
                "1",
                r#"{"success":true,"engine":"codex","metadata":{"model":"gpt-5"}}"#,
            ),
        ];
        assert!(extract_child_links(&events).is_empty());
    }
    #[test]
    fn non_agent_mux_bash_commands_ignored() {
        let events = vec![
            bash_use("1", "echo hello"),
            tool_result(
                "1",
                r#"{"success":true,"engine":"codex","metadata":{"session_id":"abc"}}"#,
            ),
        ];
        assert!(extract_child_links(&events).is_empty());
    }
    #[test]
    fn multiple_calls_extract_multiple_links() {
        let events = vec![
            bash_use("1", "agent-mux run"),
            tool_result(
                "1",
                r#"{"success":true,"engine":"codex","metadata":{"session_id":"aaa"}}"#,
            ),
            bash_use("2", "agent-mux run"),
            tool_result(
                "2",
                r#"{"success":true,"engine":"claude","metadata":{"session_id":"bbb","model":"sonnet"}}"#,
            ),
        ];
        assert_eq!(extract_child_links(&events).len(), 2);
    }
}
