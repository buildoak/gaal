use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use serde_json::Value;

use crate::discovery::discover::DiscoveredSession;
use crate::parser::types::Engine;

const APPROVED_FILES: &[&str] = &[
    "summary.json",
    "updates.jsonl",
    "chat_history.jsonl",
    "events.jsonl",
    "signals.json",
];

/// Discover native Grok sessions from `${GROK_HOME:-~/.grok}/sessions`.
///
/// Discovery is intentionally independent of AgentMUX. The session directory is
/// the source path because Grok sessions are multi-file artifacts; parser code
/// chooses the visible event source inside the directory.
pub fn discover_grok_sessions(newer_than: Option<SystemTime>) -> Result<Vec<DiscoveredSession>> {
    let Some(root) = grok_sessions_root() else {
        return Ok(Vec::new());
    };
    discover_grok_sessions_from_root(&root, newer_than)
}

#[cfg(test)]
pub(crate) fn discover_grok_sessions_from_root(
    sessions_root: &Path,
    newer_than: Option<SystemTime>,
) -> Result<Vec<DiscoveredSession>> {
    discover_grok_sessions_from_root_inner(sessions_root, newer_than)
}

#[cfg(not(test))]
fn discover_grok_sessions_from_root(
    sessions_root: &Path,
    newer_than: Option<SystemTime>,
) -> Result<Vec<DiscoveredSession>> {
    discover_grok_sessions_from_root_inner(sessions_root, newer_than)
}

fn grok_sessions_root() -> Option<PathBuf> {
    if let Some(home) = std::env::var_os("GROK_HOME") {
        let root = PathBuf::from(home).join("sessions");
        if root.exists() {
            return Some(root);
        }
    }
    dirs::home_dir().map(|home| home.join(".grok").join("sessions"))
}

fn discover_grok_sessions_from_root_inner(
    sessions_root: &Path,
    newer_than: Option<SystemTime>,
) -> Result<Vec<DiscoveredSession>> {
    if !sessions_root.exists() {
        return Ok(Vec::new());
    }

    let mut sessions = Vec::new();
    let Ok(cwd_dirs) = fs::read_dir(sessions_root) else {
        return Ok(sessions);
    };

    for cwd_entry in cwd_dirs.flatten() {
        let Ok(cwd_type) = cwd_entry.file_type() else {
            continue;
        };
        if !cwd_type.is_dir() {
            continue;
        }
        let encoded_cwd = cwd_entry.file_name().to_string_lossy().to_string();
        let Ok(session_dirs) = fs::read_dir(cwd_entry.path()) else {
            continue;
        };
        for session_entry in session_dirs.flatten() {
            let Ok(session_type) = session_entry.file_type() else {
                continue;
            };
            if !session_type.is_dir() {
                continue;
            }

            let session_id = session_entry.file_name().to_string_lossy().to_string();
            if !looks_like_grok_uuid(&session_id) {
                continue;
            }
            let session_dir = session_entry.path();
            let summary_path = session_dir.join("summary.json");
            if !summary_path.is_file() {
                continue;
            }
            if !has_visible_source(&session_dir) {
                continue;
            }

            let fingerprint = approved_file_fingerprint(&session_dir);
            if fingerprint.size == 0 {
                continue;
            }
            if let (Some(cutoff), Some(latest_mtime)) = (newer_than, fingerprint.latest_mtime) {
                if latest_mtime < cutoff {
                    continue;
                }
            }

            let summary = read_summary(&summary_path);
            sessions.push(DiscoveredSession {
                id: session_id,
                engine: Engine::Grok,
                path: session_dir,
                model: summary
                    .as_ref()
                    .and_then(|value| value.get("current_model_id"))
                    .and_then(Value::as_str)
                    .map(str::to_string),
                cwd: summary
                    .as_ref()
                    .and_then(|value| value.get("git_root_dir"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| decode_grok_cwd(&encoded_cwd)),
                started_at: summary
                    .as_ref()
                    .and_then(|value| value.get("created_at"))
                    .and_then(Value::as_str)
                    .map(str::to_string),
                forked_from_id: None,
                file_size: fingerprint.index_key,
            });
        }
    }

    Ok(sessions)
}

fn has_visible_source(session_dir: &Path) -> bool {
    has_nonempty_jsonl(session_dir.join("updates.jsonl"))
        || has_nonempty_jsonl(session_dir.join("chat_history.jsonl"))
}

fn has_nonempty_jsonl(path: PathBuf) -> bool {
    let Ok(file) = fs::File::open(path) else {
        return false;
    };
    use std::io::{BufRead, BufReader};
    BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .any(|line| !line.trim().is_empty())
}

#[derive(Default)]
struct FileFingerprint {
    size: u64,
    index_key: u64,
    latest_mtime: Option<SystemTime>,
}

fn approved_file_fingerprint(session_dir: &Path) -> FileFingerprint {
    let mut fingerprint = FileFingerprint::default();
    for name in APPROVED_FILES {
        let path = session_dir.join(name);
        let Ok(meta) = fs::metadata(path) else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        let len = meta.len();
        fingerprint.size = fingerprint.size.saturating_add(len);
        fingerprint.index_key = mix_fingerprint(fingerprint.index_key, name.as_bytes());
        fingerprint.index_key = mix_fingerprint(fingerprint.index_key, &len.to_le_bytes());
        if let Ok(mtime) = meta.modified() {
            if let Ok(duration) = mtime.duration_since(UNIX_EPOCH) {
                let nanos = duration.as_nanos().min(u128::from(u64::MAX)) as u64;
                fingerprint.index_key =
                    mix_fingerprint(fingerprint.index_key, &nanos.to_le_bytes());
            }
            fingerprint.latest_mtime = Some(
                fingerprint
                    .latest_mtime
                    .map(|current| current.max(mtime))
                    .unwrap_or(mtime),
            );
        }
    }
    fingerprint.index_key &= i64::MAX as u64;
    if fingerprint.index_key == 0 && fingerprint.size > 0 {
        fingerprint.index_key = fingerprint.size;
    }
    fingerprint
}

pub(crate) fn session_index_key(session_dir: &Path) -> u64 {
    approved_file_fingerprint(session_dir).index_key
}

fn mix_fingerprint(mut state: u64, bytes: &[u8]) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;
    if state == 0 {
        state = FNV_OFFSET;
    }
    for byte in bytes {
        state ^= u64::from(*byte);
        state = state.wrapping_mul(FNV_PRIME);
    }
    state
}

fn read_summary(path: &Path) -> Option<Value> {
    let data = fs::read_to_string(path).ok()?;
    serde_json::from_str(&data).ok()
}

fn looks_like_grok_uuid(value: &str) -> bool {
    let parts: Vec<&str> = value.split('-').collect();
    parts.len() == 5
        && parts.iter().map(|part| part.len()).eq([8, 4, 4, 4, 12])
        && parts
            .iter()
            .all(|part| part.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

fn decode_grok_cwd(encoded: &str) -> Option<String> {
    percent_decode(encoded).filter(|decoded| !decoded.is_empty())
}

fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut idx = 0;
    while idx < bytes.len() {
        if bytes[idx] == b'%' {
            let hi = *bytes.get(idx + 1)?;
            let lo = *bytes.get(idx + 2)?;
            let decoded = from_hex(hi)? << 4 | from_hex(lo)?;
            out.push(decoded);
            idx += 3;
        } else {
            out.push(bytes[idx]);
            idx += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn from_hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn decodes_grok_percent_cwd() {
        assert_eq!(
            decode_grok_cwd("%2Ftmp%2Fgaal-grok-fixture").as_deref(),
            Some("/tmp/gaal-grok-fixture")
        );
    }

    #[test]
    fn validates_uuid_shape() {
        assert!(looks_like_grok_uuid("019f46d3-0000-7000-8000-000000000001"));
        assert!(!looks_like_grok_uuid("019f46d3"));
    }

    #[test]
    fn discovers_fixture_sessions_with_full_ids() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("grok")
            .join("home")
            .join(".grok")
            .join("sessions");

        let sessions = discover_grok_sessions_from_root(&root, None).expect("discover fixtures");

        assert_eq!(sessions.len(), 3);
        assert!(sessions
            .iter()
            .all(|session| session.engine == Engine::Grok));
        assert!(sessions
            .iter()
            .any(|session| session.id == "019f46d3-0000-7000-8000-000000000001"));
        assert!(sessions
            .iter()
            .any(|session| session.id == "019f46d3-0000-7000-8000-000000000002"));
        assert!(sessions.iter().all(|session| session
            .cwd
            .as_deref()
            .is_some_and(|cwd| cwd == "/tmp/gaal-grok-fixture/")));
    }
}
