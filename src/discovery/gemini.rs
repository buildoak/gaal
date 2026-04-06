use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde_json::Value;

use crate::discovery::discover::DiscoveredSession;
use crate::parser::types::Engine;

/// Discover Gemini session JSON files from `~/.gemini/tmp/*/chats/session-*.json`.
pub fn discover_gemini_sessions() -> Result<Vec<DiscoveredSession>> {
    let Some(home) = dirs::home_dir() else {
        return Ok(Vec::new());
    };
    let root = home.join(".gemini").join("tmp");
    if !root.exists() {
        return Ok(Vec::new());
    }

    let mut sessions = Vec::new();
    for path in collect_gemini_session_files(&root) {
        let Ok(meta) = fs::metadata(&path) else {
            continue;
        };
        if !meta.is_file() || meta.len() == 0 {
            continue;
        }

        let Ok(contents) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(record) = serde_json::from_str::<Value>(&contents) else {
            continue;
        };

        let raw_id = record
            .get("sessionId")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                path.file_stem()
                    .and_then(|s| s.to_str())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| "unknown".to_string());
        let short_id: String = raw_id.chars().take(8).collect();

        sessions.push(DiscoveredSession {
            id: short_id,
            engine: Engine::Gemini,
            path,
            model: extract_gemini_model(&record),
            cwd: None,
            started_at: record
                .get("startTime")
                .and_then(Value::as_str)
                .map(str::to_string),
            forked_from_id: None,
            file_size: meta.len(),
        });
    }

    Ok(sessions)
}

fn collect_gemini_session_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let Ok(project_dirs) = fs::read_dir(root) else {
        return files;
    };

    for project_entry in project_dirs.flatten() {
        let Ok(file_type) = project_entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }

        let chats_dir = project_entry.path().join("chats");
        let Ok(entries) = fs::read_dir(chats_dir) else {
            continue;
        };

        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if !file_type.is_file() {
                continue;
            }

            let path = entry.path();
            let is_session_json = path
                .file_name()
                .and_then(|name| name.to_str())
                .map(|name| name.starts_with("session-") && name.ends_with(".json"))
                .unwrap_or(false);

            if is_session_json {
                files.push(path);
            }
        }
    }

    files
}

fn extract_gemini_model(record: &Value) -> Option<String> {
    let messages = record.get("messages")?.as_array()?;

    for message in messages {
        let is_gemini = message
            .get("type")
            .and_then(Value::as_str)
            .map(|value| value == "gemini")
            .unwrap_or(false);
        if !is_gemini {
            continue;
        }

        if let Some(model) = message.get("model").and_then(Value::as_str) {
            return Some(model.to_string());
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn home_env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn discover_gemini_sessions_finds_sessions_on_disk() {
        let _guard = home_env_lock().lock().unwrap();

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_home = std::env::temp_dir().join(format!("gaal-gemini-discovery-{unique}"));
        let chats_dir = temp_home.join(".gemini/tmp/test-project/chats");
        fs::create_dir_all(&chats_dir).unwrap();

        let session_path = chats_dir.join("session-2026-04-05T14-03-7739a8b6.json");
        fs::write(
            &session_path,
            r#"{
  "sessionId": "7739a8b6-999b-4e76-b749-2533556ef47d",
  "projectHash": "hash",
  "startTime": "2026-04-05T14:03:58.671Z",
  "lastUpdated": "2026-04-05T14:04:16.105Z",
  "kind": "main",
  "messages": [
    {
      "id": "1",
      "timestamp": "2026-04-05T14:03:58.672Z",
      "type": "user",
      "content": []
    },
    {
      "id": "2",
      "timestamp": "2026-04-05T14:04:01.893Z",
      "type": "gemini",
      "content": "Working",
      "model": "gemini-3-flash-preview"
    }
  ]
}"#,
        )
        .unwrap();

        let previous_home = std::env::var_os("HOME");
        std::env::set_var("HOME", &temp_home);

        let sessions = discover_gemini_sessions().unwrap();
        eprintln!("discovered gemini sessions: {}", sessions.len());

        if let Some(home) = previous_home {
            std::env::set_var("HOME", home);
        } else {
            std::env::remove_var("HOME");
        }

        fs::remove_dir_all(&temp_home).unwrap();

        assert_eq!(sessions.len(), 1);
        let session = &sessions[0];
        assert_eq!(session.engine, Engine::Gemini);
        assert_eq!(session.id, "7739a8b6");
        assert_eq!(session.model.as_deref(), Some("gemini-3-flash-preview"));
        assert_eq!(
            session.started_at.as_deref(),
            Some("2026-04-05T14:03:58.671Z")
        );
        assert_eq!(session.cwd, None);
        assert_eq!(session.forked_from_id, None);
        assert_eq!(session.path, session_path);
        assert!(session.file_size > 0);
    }
}
