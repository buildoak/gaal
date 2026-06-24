use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::Result;
use serde_json::Value;

use crate::discovery::agent_mux::AgentMuxSidecars;
use crate::discovery::discover::{read_head_lines, DiscoveredSession};
use crate::parser::types::Engine;

const HEAD_LINES: usize = 20;

/// Discover Antigravity CLI sessions from `~/.gemini/antigravity-cli/brain/<uuid>/`.
///
/// Prefer the complete generated transcript when it exists and is non-empty,
/// falling back to the user-visible transcript. The mtime cutoff is applied to
/// the selected transcript before any head read or JSON parsing.
pub fn discover_agy_sessions(newer_than: Option<SystemTime>) -> Result<Vec<DiscoveredSession>> {
    let Some(home) = dirs::home_dir() else {
        return Ok(Vec::new());
    };
    let root = home.join(".gemini").join("antigravity-cli");
    discover_agy_sessions_from_root_and_sidecars(&root, AgentMuxSidecars::from_home(), newer_than)
}

#[cfg(test)]
fn discover_agy_sessions_from_root(
    root: &Path,
    newer_than: Option<SystemTime>,
) -> Result<Vec<DiscoveredSession>> {
    discover_agy_sessions_from_root_and_sidecars(root, AgentMuxSidecars::default(), newer_than)
}

fn discover_agy_sessions_from_root_and_sidecars(
    root: &Path,
    agent_mux_sidecars: AgentMuxSidecars,
    newer_than: Option<SystemTime>,
) -> Result<Vec<DiscoveredSession>> {
    let brain_root = root.join("brain");
    if !brain_root.exists() {
        return Ok(Vec::new());
    }

    let cwd_by_uuid = load_last_conversations(&root.join("cache").join("last_conversations.json"));
    let mut sessions = Vec::new();
    let Ok(entries) = fs::read_dir(&brain_root) else {
        return Ok(sessions);
    };

    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }

        let uuid = entry.file_name().to_string_lossy().to_string();
        let Some((path, meta)) = selected_transcript(&entry.path()) else {
            continue;
        };

        if let Some(cutoff) = newer_than {
            if meta.modified().ok().is_some_and(|mtime| mtime < cutoff) {
                continue;
            }
        }

        let started_at = first_created_at(&path);
        let short_id = uuid.chars().take(8).collect::<String>();
        let native_cwd = cwd_by_uuid.get(&uuid).cloned();
        let sidecar = agent_mux_sidecars.get(&uuid, &short_id);
        sessions.push(DiscoveredSession {
            id: short_id,
            engine: Engine::Agy,
            path,
            model: sidecar.and_then(|meta| meta.model.clone()),
            cwd: native_cwd.or_else(|| sidecar.and_then(|meta| meta.cwd.clone())),
            started_at,
            forked_from_id: None,
            file_size: meta.len(),
        });
    }

    Ok(sessions)
}

fn selected_transcript(brain_dir: &Path) -> Option<(PathBuf, fs::Metadata)> {
    let logs_dir = brain_dir.join(".system_generated").join("logs");
    let preferred = logs_dir.join("transcript_full.jsonl");
    if let Some(meta) = non_empty_file_metadata(&preferred) {
        return Some((preferred, meta));
    }

    let fallback = logs_dir.join("transcript.jsonl");
    if let Some(meta) = non_empty_file_metadata(&fallback) {
        return Some((fallback, meta));
    }

    None
}

fn non_empty_file_metadata(path: &Path) -> Option<fs::Metadata> {
    let meta = fs::metadata(path).ok()?;
    if meta.is_file() && meta.len() > 0 {
        Some(meta)
    } else {
        None
    }
}

fn first_created_at(path: &Path) -> Option<String> {
    for line in read_head_lines(path, HEAD_LINES) {
        let Ok(record) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        if let Some(created_at) = record.get("created_at").and_then(Value::as_str) {
            return Some(created_at.to_string());
        }
    }
    None
}

fn load_last_conversations(path: &Path) -> HashMap<String, String> {
    let Ok(contents) = fs::read_to_string(path) else {
        return HashMap::new();
    };
    let Ok(root) = serde_json::from_str::<Value>(&contents) else {
        return HashMap::new();
    };
    let Some(map) = root.as_object() else {
        return HashMap::new();
    };

    let mut cwd_by_uuid = HashMap::new();
    for (cwd, value) in map {
        if let Some(uuid) = conversation_uuid(value) {
            cwd_by_uuid.insert(uuid.to_string(), cwd.to_string());
        }
    }
    cwd_by_uuid
}

fn conversation_uuid(value: &Value) -> Option<&str> {
    if let Some(uuid) = value.as_str() {
        return Some(uuid);
    }

    [
        "uuid",
        "id",
        "conversation_id",
        "conversationId",
        "brain_id",
        "brainId",
    ]
    .iter()
    .find_map(|key| value.get(*key).and_then(Value::as_str))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    fn temp_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "gaal-agy-discovery-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time")
                .as_nanos()
        ))
    }

    #[test]
    fn prefers_full_transcript_and_maps_cwd() {
        let root = temp_root("prefer-full");
        let uuid = "12345678-aaaa-bbbb-cccc-123456789abc";
        let logs = root.join("brain").join(uuid).join(".system_generated/logs");
        fs::create_dir_all(&logs).unwrap();
        fs::create_dir_all(root.join("cache")).unwrap();

        let full = logs.join("transcript_full.jsonl");
        let fallback = logs.join("transcript.jsonl");
        fs::write(
            &full,
            r#"{"step_index":0,"source":"USER_EXPLICIT","type":"USER_INPUT","status":"DONE","created_at":"2026-06-22T18:46:54Z","content":"hello"}"#,
        )
        .unwrap();
        fs::write(&fallback, "{}\n").unwrap();
        fs::write(
            root.join("cache/last_conversations.json"),
            format!(r#"{{"/work/project":"{uuid}"}}"#),
        )
        .unwrap();

        let sessions = discover_agy_sessions_from_root(&root, None).unwrap();
        fs::remove_dir_all(&root).ok();

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].engine, Engine::Agy);
        assert_eq!(sessions[0].id, "12345678");
        assert_eq!(sessions[0].path, full);
        assert_eq!(sessions[0].cwd.as_deref(), Some("/work/project"));
        assert_eq!(
            sessions[0].started_at.as_deref(),
            Some("2026-06-22T18:46:54Z")
        );
    }

    #[test]
    fn falls_back_when_full_transcript_is_empty_and_respects_cutoff() {
        let root = temp_root("fallback-cutoff");
        let uuid = "abcdef12-aaaa-bbbb-cccc-123456789abc";
        let logs = root.join("brain").join(uuid).join(".system_generated/logs");
        fs::create_dir_all(&logs).unwrap();

        fs::write(logs.join("transcript_full.jsonl"), "").unwrap();
        let fallback = logs.join("transcript.jsonl");
        fs::write(
            &fallback,
            r#"{"step_index":0,"source":"USER_EXPLICIT","type":"USER_INPUT","status":"DONE","created_at":"2026-06-22T18:46:54Z","content":"hello"}"#,
        )
        .unwrap();

        let sessions = discover_agy_sessions_from_root(&root, None).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].path, fallback);

        let future = SystemTime::now() + Duration::from_secs(60);
        let sessions = discover_agy_sessions_from_root(&root, Some(future)).unwrap();
        fs::remove_dir_all(&root).ok();

        assert!(sessions.is_empty());
    }

    #[test]
    fn missing_agent_mux_sidecars_are_harmless() {
        let root = temp_root("missing-sidecar");
        let sidecars = temp_root("missing-sidecar-agent-mux").join("dispatches");
        let uuid = "23456789-aaaa-bbbb-cccc-123456789abc";
        let logs = root.join("brain").join(uuid).join(".system_generated/logs");
        fs::create_dir_all(&logs).unwrap();
        fs::write(
            logs.join("transcript_full.jsonl"),
            r#"{"step_index":0,"source":"USER_EXPLICIT","type":"USER_INPUT","status":"DONE","created_at":"2026-06-22T18:46:54Z","content":"hello"}"#,
        )
        .unwrap();

        let sessions = discover_agy_sessions_from_root_and_sidecars(
            &root,
            AgentMuxSidecars::from_dispatches_dir(&sidecars),
            None,
        )
        .unwrap();
        fs::remove_dir_all(&root).ok();

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, "23456789");
        assert_eq!(sessions[0].model, None);
        assert_eq!(sessions[0].cwd, None);
    }

    #[test]
    fn malformed_agent_mux_sidecars_are_ignored() {
        let root = temp_root("malformed-sidecar");
        let sidecars = temp_root("malformed-sidecar-agent-mux").join("dispatches");
        let uuid = "3456789a-aaaa-bbbb-cccc-123456789abc";
        let logs = root.join("brain").join(uuid).join(".system_generated/logs");
        fs::create_dir_all(&logs).unwrap();
        fs::create_dir_all(sidecars.join("01BROKEN")).unwrap();
        fs::write(
            logs.join("transcript_full.jsonl"),
            r#"{"step_index":0,"source":"USER_EXPLICIT","type":"USER_INPUT","status":"DONE","created_at":"2026-06-22T18:46:54Z","content":"hello"}"#,
        )
        .unwrap();
        fs::write(sidecars.join("01BROKEN/meta.json"), "{not json").unwrap();

        let sessions = discover_agy_sessions_from_root_and_sidecars(
            &root,
            AgentMuxSidecars::from_dispatches_dir(&sidecars),
            None,
        )
        .unwrap();
        fs::remove_dir_all(&root).ok();
        fs::remove_dir_all(sidecars.ancestors().nth(1).unwrap()).ok();

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].model, None);
        assert_eq!(sessions[0].cwd, None);
    }

    #[test]
    fn matching_agent_mux_sidecar_enriches_model_and_cwd() {
        let root = temp_root("matching-sidecar");
        let sidecars = temp_root("matching-sidecar-agent-mux").join("dispatches");
        let uuid = "456789ab-aaaa-bbbb-cccc-123456789abc";
        let logs = root.join("brain").join(uuid).join(".system_generated/logs");
        fs::create_dir_all(&logs).unwrap();
        fs::create_dir_all(sidecars.join("01MATCH")).unwrap();
        fs::write(
            logs.join("transcript_full.jsonl"),
            r#"{"step_index":0,"source":"USER_EXPLICIT","type":"USER_INPUT","status":"DONE","created_at":"2026-06-22T18:46:54Z","content":"hello"}"#,
        )
        .unwrap();
        fs::write(
            sidecars.join("01MATCH/meta.json"),
            format!(
                r#"{{
  "dispatch_id": "01MATCH",
  "session_id": "{uuid}",
  "engine": "agy",
  "model": "Gemini 3.5 Flash (Low)",
  "effort": "high",
  "profile": "scout",
  "cwd": "/work/sidecar",
  "started_at": "2026-06-22T18:46:50Z"
}}"#
            ),
        )
        .unwrap();

        let sessions = discover_agy_sessions_from_root_and_sidecars(
            &root,
            AgentMuxSidecars::from_dispatches_dir(&sidecars),
            None,
        )
        .unwrap();
        fs::remove_dir_all(&root).ok();
        fs::remove_dir_all(sidecars.ancestors().nth(1).unwrap()).ok();

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, "456789ab");
        assert_eq!(sessions[0].model.as_deref(), Some("Gemini 3.5 Flash (Low)"));
        assert_eq!(sessions[0].cwd.as_deref(), Some("/work/sidecar"));
    }
}
