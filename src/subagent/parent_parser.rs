use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use anyhow::Result;

#[derive(Debug, Clone)]
pub struct SubagentMeta {
    pub agent_id: String,
    pub prompt: String,
    pub status: String,
    pub total_tokens: i64,
    pub total_duration_ms: i64,
    pub total_tool_use_count: i64,
    pub description: String,
}

pub fn extract_subagent_summaries(parent_jsonl: &Path) -> Result<Vec<SubagentMeta>> {
    let file = File::open(parent_jsonl)?;
    let reader = BufReader::new(file);
    let mut by_agent_id: HashMap<String, SubagentMeta> = HashMap::new();

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
            },
        );
    }

    Ok(by_agent_id.into_values().collect())
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
    }
}
