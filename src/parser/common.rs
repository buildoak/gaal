use serde_json::Value;

use crate::model::fact::FactType;
use crate::model::Fact;

pub(crate) fn tool_call_fact(
    name: &str,
    input: &Value,
    ts: String,
    turn_number: Option<i32>,
) -> Option<Fact> {
    if matches!(name, "Read" | "Glob" | "Grep") {
        return Some(file_read_fact(input, ts, turn_number));
    }
    if matches!(name, "Write" | "Edit" | "apply_patch") {
        return Some(file_write_fact(input, ts, turn_number));
    }
    if matches!(name, "Bash" | "exec_command") {
        return Some(bash_fact(input, ts, turn_number));
    }
    if matches!(name, "Task" | "TaskCreate" | "Agent") {
        return Some(task_fact(input, ts, turn_number));
    }
    if matches!(name, "WebSearch" | "WebFetch" | "web_search" | "web_fetch") {
        return Some(web_fact(input, ts, turn_number));
    }
    None
}

pub(crate) fn file_read_fact(input: &Value, ts: String, turn_number: Option<i32>) -> Fact {
    Fact {
        id: None,
        session_id: String::new(),
        ts,
        turn_number,
        fact_type: FactType::FileRead,
        subject: extract_file_path(input),
        detail: serde_json::to_string(input).ok(),
        exit_code: None,
        success: None,
    }
}

pub(crate) fn file_write_fact(input: &Value, ts: String, turn_number: Option<i32>) -> Fact {
    Fact {
        id: None,
        session_id: String::new(),
        ts,
        turn_number,
        fact_type: FactType::FileWrite,
        subject: extract_file_path(input),
        detail: serde_json::to_string(input).ok(),
        exit_code: None,
        success: None,
    }
}

pub(crate) fn bash_fact(input: &Value, ts: String, turn_number: Option<i32>) -> Fact {
    let cmd = extract_command(input);
    Fact {
        id: None,
        session_id: String::new(),
        ts,
        turn_number,
        fact_type: FactType::Command,
        subject: cmd.as_ref().map(|c| truncate(c, 100)),
        detail: cmd.or_else(|| serde_json::to_string(input).ok()),
        exit_code: None,
        success: None,
    }
}

pub(crate) fn task_fact(input: &Value, ts: String, turn_number: Option<i32>) -> Fact {
    Fact {
        id: None,
        session_id: String::new(),
        ts,
        turn_number,
        fact_type: FactType::TaskSpawn,
        subject: None,
        detail: serde_json::to_string(input).ok(),
        exit_code: None,
        success: None,
    }
}

pub(crate) fn web_fact(input: &Value, ts: String, turn_number: Option<i32>) -> Fact {
    let detail = extract_web_detail(input).or_else(|| serde_json::to_string(input).ok());
    Fact {
        id: None,
        session_id: String::new(),
        ts,
        turn_number,
        fact_type: FactType::Command,
        subject: detail.as_ref().map(|v| truncate(v, 100)),
        detail,
        exit_code: None,
        success: None,
    }
}

pub(crate) fn as_i64(value: Option<&Value>) -> i64 {
    value
        .and_then(|v| {
            v.as_i64()
                .or_else(|| v.as_u64().and_then(|u| i64::try_from(u).ok()))
        })
        .unwrap_or(0)
}

pub(crate) fn extract_file_path(input: &Value) -> Option<String> {
    ["file_path", "path", "file", "dir", "directory", "cwd"]
        .iter()
        .find_map(|key| input.get(*key).and_then(Value::as_str).map(str::to_string))
}

pub(crate) fn extract_command(input: &Value) -> Option<String> {
    ["command", "cmd"]
        .iter()
        .find_map(|key| input.get(*key).and_then(Value::as_str).map(str::to_string))
}

pub(crate) fn extract_web_detail(input: &Value) -> Option<String> {
    ["url", "query"]
        .iter()
        .find_map(|key| input.get(*key).and_then(Value::as_str).map(str::to_string))
}

pub(crate) fn parse_exit_code(output: &str) -> Option<i32> {
    if let Ok(value) = serde_json::from_str::<Value>(output) {
        if let Some(code) = value.pointer("/metadata/exit_code").and_then(Value::as_i64) {
            return i32::try_from(code).ok();
        }
    }
    output.lines().find_map(parse_exit_line)
}

pub(crate) fn parse_exit_line(line: &str) -> Option<i32> {
    for prefix in ["Process exited with code ", "Exit code ", "exit code: "] {
        if let Some(rest) = line.trim().strip_prefix(prefix) {
            return rest
                .split_whitespace()
                .next()
                .and_then(|v| v.parse::<i32>().ok());
        }
    }
    None
}

pub(crate) fn is_git_command(command: &str) -> bool {
    let trimmed = command.trim_start();
    [
        "git commit",
        "git push",
        "git checkout",
        "git rebase",
        "git merge",
        "git pull",
        "git cherry-pick",
        "git reset",
        "git add",
    ]
    .iter()
    .any(|prefix| trimmed.starts_with(prefix))
}

pub(crate) fn truncate(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    value.chars().take(max_chars).collect()
}

pub(crate) fn resolve_started_at(
    started_at: Option<String>,
    last_event_at: Option<String>,
) -> String {
    started_at
        .or(last_event_at)
        .unwrap_or_else(|| "1970-01-01T00:00:00Z".to_string())
}
