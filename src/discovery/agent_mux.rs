use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct AgentMuxEnrichment {
    pub model: Option<String>,
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub(super) struct AgentMuxSidecars {
    by_session_key: HashMap<String, SidecarMatch>,
}

#[derive(Debug, Clone)]
enum SidecarMatch {
    Unique(AgentMuxEnrichment),
    Ambiguous,
}

impl AgentMuxSidecars {
    pub(super) fn from_home() -> Self {
        let Some(home) = dirs::home_dir() else {
            return Self::default();
        };
        Self::from_dispatches_dir(&home.join(".agent-mux").join("dispatches"))
    }

    pub(super) fn from_dispatches_dir(root: &Path) -> Self {
        let Ok(entries) = fs::read_dir(root) else {
            return Self::default();
        };

        let mut meta_paths = entries
            .flatten()
            .filter_map(|entry| {
                let Ok(file_type) = entry.file_type() else {
                    return None;
                };
                if file_type.is_dir() {
                    Some(entry.path().join("meta.json"))
                } else {
                    None
                }
            })
            .collect::<Vec<PathBuf>>();
        meta_paths.sort();

        let mut sidecars = Self::default();
        for path in meta_paths {
            sidecars.load_one(&path);
        }
        sidecars
    }

    pub(super) fn get(&self, agy_uuid: &str, short_id: &str) -> Option<&AgentMuxEnrichment> {
        for key in session_lookup_keys(agy_uuid, short_id) {
            match self.by_session_key.get(&key) {
                Some(SidecarMatch::Unique(enrichment)) => return Some(enrichment),
                Some(SidecarMatch::Ambiguous) => return None,
                None => {}
            }
        }
        None
    }

    fn load_one(&mut self, path: &Path) {
        let Ok(contents) = fs::read_to_string(path) else {
            return;
        };
        let Ok(meta) = serde_json::from_str::<Value>(&contents) else {
            return;
        };
        if meta.get("engine").and_then(Value::as_str) != Some("agy") {
            return;
        }

        let keys = meta_session_keys(&meta);
        if keys.is_empty() {
            return;
        }

        let enrichment = AgentMuxEnrichment {
            model: string_field(&meta, "model"),
            cwd: string_field(&meta, "cwd"),
        };
        if enrichment.model.is_none() && enrichment.cwd.is_none() {
            return;
        }

        for key in keys {
            self.insert_key(key, enrichment.clone());
        }
    }

    fn insert_key(&mut self, key: String, enrichment: AgentMuxEnrichment) {
        match self.by_session_key.get(&key) {
            None => {
                self.by_session_key
                    .insert(key, SidecarMatch::Unique(enrichment));
            }
            Some(SidecarMatch::Unique(existing)) if existing == &enrichment => {}
            Some(_) => {
                self.by_session_key.insert(key, SidecarMatch::Ambiguous);
            }
        }
    }
}

fn meta_session_keys(meta: &Value) -> Vec<String> {
    [
        "session_id",
        "agy_session_id",
        "agy_uuid",
        "native_session_id",
        "native_agy_session_id",
        "native_agy_uuid",
        "conversation_id",
        "conversation_uuid",
        "uuid",
        "short_id",
        "session_short_id",
    ]
    .iter()
    .filter_map(|key| string_field(meta, key))
    .flat_map(|value| key_variants(&value))
    .collect()
}

fn session_lookup_keys(agy_uuid: &str, short_id: &str) -> Vec<String> {
    let mut keys = key_variants(agy_uuid);
    keys.extend(key_variants(short_id));
    keys
}

fn key_variants(value: &str) -> Vec<String> {
    let normalized = normalize_session_key(value);
    if normalized.is_empty() {
        return Vec::new();
    }

    let mut keys = vec![normalized.clone()];
    let short_key = normalized.chars().take(8).collect::<String>();
    if normalized.len() >= 8 && short_key != normalized {
        keys.push(short_key);
    }
    keys
}

fn normalize_session_key(value: &str) -> String {
    value
        .trim()
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .map(|ch| ch.to_ascii_lowercase())
        .collect()
}

fn string_field(meta: &Value, key: &str) -> Option<String> {
    meta.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}
