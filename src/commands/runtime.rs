use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde_json::Value;

use crate::model::{Fact, FactType};
use crate::parser::types::Engine;

// ---------------------------------------------------------------------------
// Runtime probe types — shared by inspect, show, ls
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct RuntimeProbe {
    pub session_id: Option<String>,
    pub last_event_ts: Option<String>,
    pub last_action: Option<ActionEvent>,
    pub usage_samples: Vec<UsageSample>,
}

#[derive(Debug, Clone)]
pub struct UsageSample {
    pub ts: String,
    pub tokens: i64,
    pub input_tokens: i64,
}

#[derive(Debug, Clone, Default)]
pub struct ActionEvent {
    pub ts: Option<String>,
    pub kind: String,
    pub summary: String,
}

// ---------------------------------------------------------------------------
// Core probe function
// ---------------------------------------------------------------------------

pub fn probe_runtime(path: &Path, engine: Engine, max_lines: usize) -> RuntimeProbe {
    let lines = read_tail_lines(path, max_lines);
    if lines.is_empty() {
        return RuntimeProbe::default();
    }

    let mut session_id: Option<String> = None;
    let mut last_event_ts: Option<String> = None;
    let mut last_action: Option<ActionEvent> = None;
    let mut usage_samples: Vec<UsageSample> = Vec::new();

    for line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(record) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };

        let ts = record_timestamp(&record);
        if ts.is_some() {
            last_event_ts = ts.clone();
        }
        if session_id.is_none() {
            session_id = extract_session_id(&record, engine);
        }

        if let Some(sample) = extract_usage_sample(&record, engine) {
            usage_samples.push(sample);
        }

        let actions = extract_actions(&record, engine, ts.clone());
        for action in actions {
            if !action.kind.is_empty() {
                last_action = Some(action.clone());
            }
        }
    }

    RuntimeProbe {
        session_id,
        last_event_ts,
        last_action,
        usage_samples,
    }
}

// ---------------------------------------------------------------------------
// Timestamp / age utilities
// ---------------------------------------------------------------------------

pub fn parse_ts(ts: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(ts)
        .map(|dt| dt.with_timezone(&Utc))
        .ok()
}

pub fn age_from_ts(ts: &str, now: DateTime<Utc>) -> Option<u64> {
    let ts = parse_ts(ts)?;
    if ts > now {
        return Some(0);
    }
    let delta = now.signed_duration_since(ts);
    u64::try_from(delta.num_seconds()).ok()
}

// ---------------------------------------------------------------------------
// Fact-based action helpers
// ---------------------------------------------------------------------------

pub fn latest_action_from_facts(facts: &[Fact]) -> Option<ActionEvent> {
    facts.iter().rev().find_map(fact_to_action)
}

fn fact_to_action(fact: &Fact) -> Option<ActionEvent> {
    let kind = match fact.fact_type {
        FactType::FileRead => "Read",
        FactType::FileWrite => "Write",
        FactType::Command => "Bash",
        FactType::TaskSpawn => "Task",
        FactType::GitOp => "Git",
        _ => return None,
    }
    .to_string();

    let summary = fact
        .detail
        .clone()
        .or_else(|| fact.subject.clone())
        .unwrap_or_else(|| "action".to_string());

    Some(ActionEvent {
        ts: Some(fact.ts.clone()),
        kind,
        summary: truncate(summary.as_str(), 120),
    })
}

pub fn format_action(kind: &str, summary: &str) -> String {
    format!("{kind}: {summary}")
}

pub fn count_actions_in_window(facts: &[Fact], anchor: DateTime<Utc>, minutes: i64) -> usize {
    let start = anchor - ChronoDuration::minutes(minutes.max(1));
    facts
        .iter()
        .filter(|fact| is_action_fact(&fact.fact_type))
        .filter_map(|fact| parse_ts(&fact.ts))
        .filter(|ts| *ts >= start && *ts <= anchor)
        .count()
}

pub fn tokens_per_minute_from_samples(
    samples: &[UsageSample],
    anchor: DateTime<Utc>,
    minutes: i64,
) -> f64 {
    let start = anchor - ChronoDuration::minutes(minutes.max(1));
    let total = samples
        .iter()
        .filter_map(|sample| parse_ts(&sample.ts).map(|ts| (ts, sample.tokens)))
        .filter(|(ts, _)| *ts >= start && *ts <= anchor)
        .map(|(_, tokens)| tokens.max(0))
        .sum::<i64>();
    round1(total as f64 / minutes.max(1) as f64)
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

fn extract_session_id(record: &Value, engine: Engine) -> Option<String> {
    match engine {
        Engine::Claude => record
            .get("sessionId")
            .and_then(Value::as_str)
            .map(str::to_string),
        Engine::Codex => record
            .pointer("/payload/id")
            .or_else(|| record.get("session_id"))
            .and_then(Value::as_str)
            .map(str::to_string),
    }
}

fn record_timestamp(record: &Value) -> Option<String> {
    record
        .get("timestamp")
        .and_then(Value::as_str)
        .or_else(|| record.pointer("/payload/timestamp").and_then(Value::as_str))
        .map(str::to_string)
}

fn extract_usage_sample(record: &Value, engine: Engine) -> Option<UsageSample> {
    let ts = record_timestamp(record)?;
    let (input_tokens, tokens) = match engine {
        Engine::Claude => {
            // I33: Claude API splits tokens into input_tokens (non-cached),
            // cache_read_input_tokens, and cache_creation_input_tokens.
            // Total context usage = sum of all three.
            let input = as_i64(record.pointer("/message/usage/input_tokens"));
            let cache_read = as_i64(record.pointer("/message/usage/cache_read_input_tokens"));
            let cache_creation =
                as_i64(record.pointer("/message/usage/cache_creation_input_tokens"));
            let total_input = input + cache_read + cache_creation;
            let output = as_i64(record.pointer("/message/usage/output_tokens"));
            (total_input, total_input + output)
        }
        Engine::Codex => {
            // I32: Focus on token_count events for accurate current context usage
            let payload_type = record.pointer("/payload/type").and_then(Value::as_str);
            if payload_type == Some("token_count") {
                // Use last_token_usage which represents the most recent turn's context usage
                let input =
                    as_i64(record.pointer("/payload/info/last_token_usage/input_tokens"));
                let cached =
                    as_i64(record.pointer("/payload/info/last_token_usage/cached_input_tokens"));
                let total_input = input + cached;
                let output =
                    as_i64(record.pointer("/payload/info/last_token_usage/output_tokens"));
                (total_input, total_input + output)
            } else {
                // Skip non-token_count records to avoid cumulative values
                return None;
            }
        }
    };

    (tokens > 0).then_some(UsageSample {
        ts,
        tokens,
        input_tokens,
    })
}

fn as_i64(value: Option<&Value>) -> i64 {
    value
        .and_then(|v| {
            v.as_i64()
                .or_else(|| v.as_u64().and_then(|n| i64::try_from(n).ok()))
        })
        .unwrap_or(0)
}

fn extract_actions(record: &Value, engine: Engine, ts: Option<String>) -> Vec<ActionEvent> {
    match engine {
        Engine::Claude => extract_claude_actions(record, ts),
        Engine::Codex => extract_codex_actions(record, ts),
    }
}

fn extract_claude_actions(record: &Value, ts: Option<String>) -> Vec<ActionEvent> {
    if record.get("type").and_then(Value::as_str) != Some("assistant") {
        return Vec::new();
    }

    let Some(blocks) = record.pointer("/message/content").and_then(Value::as_array) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for block in blocks {
        if block.get("type").and_then(Value::as_str) != Some("tool_use") {
            continue;
        }

        let kind = block
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("tool")
            .to_string();
        let input = block.get("input").cloned().unwrap_or(Value::Null);
        let subject = extract_action_subject(&input);
        let summary = extract_action_summary(&kind, &input, subject.as_deref());

        out.push(ActionEvent {
            ts: ts.clone(),
            kind,
            summary,
        });
    }

    out
}

fn extract_codex_actions(record: &Value, ts: Option<String>) -> Vec<ActionEvent> {
    if record.get("type").and_then(Value::as_str) != Some("response_item") {
        return Vec::new();
    }

    let payload_type = record
        .pointer("/payload/type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !matches!(payload_type, "function_call" | "custom_tool_call") {
        return Vec::new();
    }

    let kind = record
        .pointer("/payload/name")
        .and_then(Value::as_str)
        .unwrap_or("tool")
        .to_string();

    let input = if payload_type == "function_call" {
        parse_json_or_null(record.pointer("/payload/arguments").and_then(Value::as_str))
    } else {
        parse_json_or_null(record.pointer("/payload/input").and_then(Value::as_str))
    };

    let subject = extract_action_subject(&input);
    let summary = extract_action_summary(&kind, &input, subject.as_deref());

    vec![ActionEvent {
        ts,
        kind,
        summary,
    }]
}

fn parse_json_or_null(raw: Option<&str>) -> Value {
    raw.and_then(|s| serde_json::from_str::<Value>(s).ok())
        .unwrap_or(Value::Null)
}

fn extract_action_subject(input: &Value) -> Option<String> {
    [
        "file_path",
        "path",
        "file",
        "command",
        "cmd",
        "url",
        "query",
        "directory",
        "dir",
        "cwd",
    ]
    .iter()
    .find_map(|key| input.get(*key).and_then(Value::as_str).map(str::to_string))
}

fn extract_action_summary(kind: &str, input: &Value, subject: Option<&str>) -> String {
    if matches!(kind, "Bash" | "exec_command") {
        if let Some(cmd) = input
            .get("command")
            .or_else(|| input.get("cmd"))
            .and_then(Value::as_str)
        {
            return truncate(cmd, 120);
        }
    }

    if let Some(subject) = subject {
        return truncate(subject, 120);
    }

    if input.is_null() {
        return "tool call".to_string();
    }

    let raw = serde_json::to_string(input).unwrap_or_else(|_| "tool call".to_string());
    truncate(raw.as_str(), 120)
}

fn is_action_fact(fact_type: &FactType) -> bool {
    matches!(
        fact_type,
        FactType::FileRead
            | FactType::FileWrite
            | FactType::Command
            | FactType::TaskSpawn
            | FactType::GitOp
    )
}

fn read_tail_lines(path: &Path, max_lines: usize) -> Vec<String> {
    if max_lines == 0 {
        return Vec::new();
    }

    let Ok(mut file) = File::open(path) else {
        return Vec::new();
    };

    let Ok(file_len) = file.metadata().map(|meta| meta.len()) else {
        return Vec::new();
    };
    if file_len == 0 {
        return Vec::new();
    }

    let mut ends_with_newline = false;
    if file.seek(SeekFrom::End(-1)).is_ok() {
        let mut last_byte = [0_u8; 1];
        if file.read_exact(&mut last_byte).is_ok() {
            ends_with_newline = last_byte[0] == b'\n';
        }
    }

    let target_newlines = max_lines.saturating_add(usize::from(ends_with_newline));
    let mut start = 0_u64;
    let mut pos = file_len;
    let mut seen_newlines = 0_usize;
    let mut found = false;
    let mut chunk = vec![0_u8; 64 * 1024];

    while pos > 0 {
        let read_size_u64 = pos.min(chunk.len() as u64);
        let read_size = read_size_u64 as usize;
        let new_pos = pos - read_size_u64;

        if file.seek(SeekFrom::Start(new_pos)).is_err() {
            return Vec::new();
        }
        if file.read_exact(&mut chunk[..read_size]).is_err() {
            return Vec::new();
        }

        for idx in (0..read_size).rev() {
            if chunk[idx] == b'\n' {
                seen_newlines += 1;
                if seen_newlines == target_newlines {
                    let Ok(idx_u64) = u64::try_from(idx) else {
                        return Vec::new();
                    };
                    start = new_pos + idx_u64 + 1;
                    found = true;
                    break;
                }
            }
        }

        if found {
            break;
        }
        pos = new_pos;
    }

    if file.seek(SeekFrom::Start(start)).is_err() {
        return Vec::new();
    }

    let reader = BufReader::new(file);
    let mut lines = Vec::with_capacity(max_lines);
    for line in reader.lines().map_while(Result::ok) {
        if lines.len() == max_lines {
            break;
        }
        lines.push(line);
    }

    lines
}

fn round1(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

fn truncate(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    value.chars().take(max_chars).collect()
}
