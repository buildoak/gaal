use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use anyhow::Result;
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct SubagentMeta {
    pub agent_id: String,
    pub prompt: String,
    pub status: String,
    pub total_tokens: i64,
    pub total_duration_ms: i64,
    pub total_tool_use_count: i64,
    pub description: String,
    /// The `subagent_type` from the Agent tool_use input (e.g. "gsd-heavy", "Explore").
    pub subagent_type: Option<String>,
}

pub fn extract_subagent_summaries(parent_jsonl: &Path) -> Result<Vec<SubagentMeta>> {
    let file = File::open(parent_jsonl)?;
    let reader = BufReader::new(file);
    let mut by_agent_id: HashMap<String, SubagentMeta> = HashMap::new();
    // Map prompt text (first 200 chars) → subagent_type from Agent tool_use inputs.
    // tool_use blocks appear before the corresponding toolUseResult in the JSONL,
    // so we can build this map incrementally during the single-pass scan.
    let mut prompt_to_subagent_type: HashMap<String, String> = HashMap::new();

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let record: serde_json::Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => continue,
        };

        // Phase 1: Collect subagent_type from Agent tool_use input blocks.
        // These appear as message.content[].type == "tool_use" with name == "Agent".
        if let Some(msg) = record.get("message") {
            if let Some(content) = msg.get("content").and_then(|c| c.as_array()) {
                for block in content {
                    if block.get("type").and_then(|v| v.as_str()) == Some("tool_use")
                        && block.get("name").and_then(|v| v.as_str()) == Some("Agent")
                    {
                        if let Some(input) = block.get("input") {
                            if let Some(st) = input.get("subagent_type").and_then(|v| v.as_str()) {
                                if !st.is_empty() {
                                    let prompt_key =
                                        input.get("prompt").and_then(|v| v.as_str()).unwrap_or("");
                                    let key = prompt_match_key(prompt_key);
                                    prompt_to_subagent_type.insert(key, st.to_string());
                                }
                            }
                        }
                    }
                }
            }
        }

        // Phase 2: Process toolUseResult blocks (existing logic).
        let Some(tool_result) = record.get("toolUseResult") else {
            continue;
        };
        let Some(agent_id) = tool_result.get("agentId").and_then(|v| v.as_str()) else {
            continue;
        };

        let prompt = tool_result
            .get("prompt")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let status = tool_result
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        let total_tokens = tool_result
            .get("totalTokens")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let total_duration_ms = tool_result
            .get("totalDurationMs")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let total_tool_use_count = tool_result
            .get("totalToolUseCount")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);

        let description = tool_result
            .get("description")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(|text| truncate_description(text, 80))
            .unwrap_or_else(|| derive_description(&prompt));

        // Correlate subagent_type by matching the prompt text.
        let subagent_type = prompt_to_subagent_type
            .get(&prompt_match_key(&prompt))
            .cloned();

        by_agent_id.insert(
            agent_id.to_string(),
            SubagentMeta {
                agent_id: agent_id.to_string(),
                prompt,
                status,
                total_tokens,
                total_duration_ms,
                total_tool_use_count,
                description,
                subagent_type,
            },
        );
    }

    Ok(by_agent_id.into_values().collect())
}

pub fn extract_codex_spawn_summaries(parent_jsonl: &Path) -> Result<Vec<SubagentMeta>> {
    let file = File::open(parent_jsonl)?;
    let reader = BufReader::new(file);
    let mut pending_spawns: HashMap<String, (Option<String>, String)> = HashMap::new();
    let mut pending_closes: HashMap<String, String> = HashMap::new();
    let mut by_agent_id: HashMap<String, SubagentMeta> = HashMap::new();
    let mut order: Vec<String> = Vec::new();

    for line in reader.lines() {
        let line = match line {
            Ok(line) => line,
            Err(_) => continue,
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let record: Value = match serde_json::from_str(trimmed) {
            Ok(record) => record,
            Err(_) => continue,
        };
        if record.get("type").and_then(Value::as_str) != Some("response_item") {
            continue;
        }

        let payload_type = record
            .pointer("/payload/type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let call_id = record
            .pointer("/payload/call_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();

        match payload_type {
            "function_call" => {
                let name = record
                    .pointer("/payload/name")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let arguments = parse_json_or_value(record.pointer("/payload/arguments"));
                match name {
                    "spawn_agent" => {
                        let agent_type = arguments
                            .get("agent_type")
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                            .map(str::to_string);
                        let message = arguments
                            .get("message")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                            .unwrap_or_default();
                        if !call_id.is_empty() {
                            pending_spawns.insert(call_id, (agent_type, message));
                        }
                    }
                    "close_agent" => {
                        let target = arguments
                            .get("target")
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                            .map(str::to_string);
                        if let (false, Some(target)) = (call_id.is_empty(), target) {
                            pending_closes.insert(call_id, target);
                        }
                    }
                    _ => {}
                }
            }
            "function_call_output" => {
                let output = parse_json_or_value(record.pointer("/payload/output"));

                if let Some((agent_type, message)) = pending_spawns.remove(&call_id) {
                    let agent_id = output
                        .get("agent_id")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(str::to_string);
                    let Some(agent_id) = agent_id else {
                        continue;
                    };

                    let prompt = message;
                    let description = derive_description(&prompt);
                    let subagent_type = agent_type.filter(|value| !value.is_empty());

                    if !by_agent_id.contains_key(&agent_id) {
                        order.push(agent_id.clone());
                    }

                    by_agent_id.insert(
                        agent_id.clone(),
                        SubagentMeta {
                            agent_id,
                            prompt,
                            status: "unknown".to_string(),
                            total_tokens: 0,
                            total_duration_ms: 0,
                            total_tool_use_count: 0,
                            description,
                            subagent_type,
                        },
                    );
                    continue;
                }

                if let Some(target) = pending_closes.remove(&call_id) {
                    let status = decode_codex_close_status(&output);
                    if let Some(meta) = by_agent_id.get_mut(&target) {
                        meta.status = status;
                    }
                }
            }
            _ => {}
        }
    }

    Ok(order
        .into_iter()
        .filter_map(|agent_id| by_agent_id.remove(&agent_id))
        .collect())
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

fn decode_codex_close_status(output: &Value) -> String {
    match output.get("previous_status") {
        Some(Value::String(status)) => status.clone(),
        Some(Value::Object(map)) => map
            .keys()
            .next()
            .cloned()
            .unwrap_or_else(|| "unknown".to_string()),
        _ => "unknown".to_string(),
    }
}

/// Generate a stable key for prompt matching between tool_use input and toolUseResult.
/// Uses the first 200 characters to avoid false matches while being resilient to
/// minor trailing differences.
fn prompt_match_key(prompt: &str) -> String {
    prompt.chars().take(200).collect()
}

fn derive_description(prompt: &str) -> String {
    let prompt = prompt.trim();
    if prompt.is_empty() {
        return "subagent task".to_string();
    }

    let first_line = prompt
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("subagent task");
    truncate_description(first_line, 80)
}

fn truncate_description(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim().replace('\n', " ");
    if trimmed.chars().count() <= max_chars {
        return trimmed;
    }

    let keep = max_chars.saturating_sub(3);
    let mut truncated: String = trimmed.chars().take(keep).collect();
    truncated.push_str("...");
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn prefers_description_field_when_present() {
        let dir = std::env::temp_dir().join(format!(
            "gaal-parent-parser-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("parent.jsonl");
        fs::write(
            &path,
            r#"{"toolUseResult":{"agentId":"agent-1","description":"Short description","prompt":"prompt fallback","status":"completed","totalTokens":1,"totalDurationMs":2,"totalToolUseCount":3,"usage":{}}}
"#,
        )
        .expect("write jsonl");

        let summaries = extract_subagent_summaries(&path).expect("parse");
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].description, "Short description");
        assert_eq!(summaries[0].subagent_type, None);
    }

    #[test]
    fn falls_back_to_prompt_when_description_missing() {
        let dir = std::env::temp_dir().join(format!(
            "gaal-parent-parser-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("parent.jsonl");
        fs::write(
            &path,
            r#"{"toolUseResult":{"agentId":"agent-2","prompt":"Investigate the API failure in detail","status":"completed","totalTokens":1,"totalDurationMs":2,"totalToolUseCount":3,"usage":{}}}
"#,
        )
        .expect("write jsonl");

        let summaries = extract_subagent_summaries(&path).expect("parse");
        assert_eq!(summaries.len(), 1);
        assert_eq!(
            summaries[0].description,
            "Investigate the API failure in detail"
        );
        assert_eq!(summaries[0].subagent_type, None);
    }

    #[test]
    fn extracts_subagent_type_from_tool_use_input() {
        let dir = std::env::temp_dir().join(format!(
            "gaal-parent-parser-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("parent.jsonl");
        // Line 1: Agent tool_use with subagent_type in input
        // Line 2: toolUseResult with matching prompt
        fs::write(
            &path,
            r#"{"message":{"content":[{"type":"tool_use","name":"Agent","id":"toolu_1","input":{"prompt":"Build the auth module","subagent_type":"gsd-heavy"}}]}}
{"toolUseResult":{"agentId":"agent-gsd1","prompt":"Build the auth module","status":"completed","totalTokens":500,"totalDurationMs":1000,"totalToolUseCount":10,"usage":{}}}
"#,
        )
        .expect("write jsonl");

        let summaries = extract_subagent_summaries(&path).expect("parse");
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].subagent_type.as_deref(), Some("gsd-heavy"));
        assert_eq!(summaries[0].description, "Build the auth module");
    }

    #[test]
    fn extracts_codex_spawn_agent_summaries() {
        let dir = std::env::temp_dir().join(format!(
            "gaal-parent-parser-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("parent.jsonl");
        fs::write(
            &path,
            concat!(
                "{\"type\":\"response_item\",\"payload\":{\"type\":\"function_call\",\"name\":\"spawn_agent\",\"call_id\":\"call_spawn\",\"arguments\":\"{\\\"agent_type\\\":\\\"explorer\\\",\\\"message\\\":\\\"Investigate the failing index path\\\"}\"}}\n",
                "{\"type\":\"response_item\",\"payload\":{\"type\":\"function_call_output\",\"call_id\":\"call_spawn\",\"output\":\"{\\\"agent_id\\\":\\\"019d2e57-8e18-7851-bbc1-93c2458fb749\\\",\\\"nickname\\\":\\\"Herschel\\\"}\"}}\n",
                "{\"type\":\"response_item\",\"payload\":{\"type\":\"function_call\",\"name\":\"close_agent\",\"call_id\":\"call_close\",\"arguments\":\"{\\\"target\\\":\\\"019d2e57-8e18-7851-bbc1-93c2458fb749\\\"}\"}}\n",
                "{\"type\":\"response_item\",\"payload\":{\"type\":\"function_call_output\",\"call_id\":\"call_close\",\"output\":\"{\\\"previous_status\\\":{\\\"completed\\\":\\\"done\\\"}}\"}}\n"
            ),
        )
        .expect("write jsonl");

        let summaries = extract_codex_spawn_summaries(&path).expect("parse");
        assert_eq!(summaries.len(), 1);
        assert_eq!(
            summaries[0].agent_id,
            "019d2e57-8e18-7851-bbc1-93c2458fb749"
        );
        assert_eq!(summaries[0].prompt, "Investigate the failing index path");
        assert_eq!(
            summaries[0].description,
            "Investigate the failing index path"
        );
        assert_eq!(summaries[0].status, "completed");
        assert_eq!(summaries[0].subagent_type.as_deref(), Some("explorer"));
        assert_eq!(summaries[0].total_tokens, 0);
        assert_eq!(summaries[0].total_duration_ms, 0);
        assert_eq!(summaries[0].total_tool_use_count, 0);
    }
}
