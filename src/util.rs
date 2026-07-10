use std::fs;
use std::path::Path;

const HERMES_ALIAS_LEN: usize = 8;
const HERMES_ALIAS_ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x100000001b3;

/// Sanitize a string for safe use as a filename component.
/// Replaces path separators, parent-dir traversals, and null bytes with underscores.
/// Truncates to 255 characters.
pub fn sanitize_filename(id: &str) -> String {
    let sanitized: String = id.replace(['/', '\\', '\0'], "_").replace("..", "__");
    if sanitized.len() > 255 {
        sanitized[..255].to_string()
    } else {
        sanitized
    }
}

/// Deterministic 8-character Hermes alias candidate.
///
/// The base candidate hashes `hermes:` plus the full native session id. Collision
/// retries append a deterministic counter salt; the SQLite alias registry decides
/// which counter is actually assigned.
pub fn hermes_alias_candidate(session_id: &str, counter: u32) -> String {
    let input = if counter == 0 {
        format!("hermes:{session_id}")
    } else {
        format!("hermes:{session_id}:{counter}")
    };
    base32_8(stable_hash64(input.as_bytes()))
}

pub fn is_hermes_alias(value: &str) -> bool {
    value.len() == HERMES_ALIAS_LEN
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || (b'2'..=b'7').contains(&byte))
}

/// Last 8 hex characters of a dash-stripped Grok UUID.
///
/// Grok frequently emits UUIDv7-shaped IDs with timestamp-heavy prefixes, so
/// this mirrors Codex's human lookup convention while preserving the full UUID
/// as the canonical session ID.
pub fn grok_alias_candidate(session_id: &str) -> Option<String> {
    let hex: String = session_id.chars().filter(|c| *c != '-').collect();
    if hex.len() < 8 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    Some(hex[hex.len() - 8..].to_ascii_lowercase())
}

pub fn is_grok_alias(value: &str) -> bool {
    value.len() == 8 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Return the filename/session-display identifier used for derived artifacts.
///
/// Claude, Gemini, and agy keep the historic first-8 convention. Codex session
/// IDs are already shortened during discovery. Hermes and Grok keep full
/// sanitized IDs for artifacts; their short IDs live in the alias registry.
pub fn session_artifact_id(engine: &str, id: &str) -> String {
    let sanitized = sanitize_filename(id);
    if matches!(engine, "hermes" | "grok") {
        sanitized
    } else {
        sanitized.chars().take(8).collect()
    }
}

/// Write content to a file atomically using a temporary file and rename.
/// This prevents partial writes if the process is killed mid-write.
pub fn atomic_write(path: &Path, content: &str) -> std::io::Result<()> {
    let tmp_path = path.with_extension("tmp");
    fs::write(&tmp_path, content)?;
    fs::rename(&tmp_path, path)?;
    Ok(())
}

fn stable_hash64(bytes: &[u8]) -> u64 {
    let mut hash = FNV_OFFSET_BASIS;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }

    // Stable avalanche finalizer to improve distribution before taking 40 bits.
    hash ^= hash >> 33;
    hash = hash.wrapping_mul(0xff51afd7ed558ccd);
    hash ^= hash >> 33;
    hash = hash.wrapping_mul(0xc4ceb9fe1a85ec53);
    hash ^ (hash >> 33)
}

fn base32_8(value: u64) -> String {
    let mut out = String::with_capacity(HERMES_ALIAS_LEN);
    for idx in 0..HERMES_ALIAS_LEN {
        let shift = 64 - 5 * (idx + 1);
        let alphabet_idx = ((value >> shift) & 0x1f) as usize;
        out.push(HERMES_ALIAS_ALPHABET[alphabet_idx] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hermes_alias_candidate_is_stable_lowercase_base32() {
        let alias = hermes_alias_candidate("20260504_141414_childbb", 0);
        assert_eq!(alias, hermes_alias_candidate("20260504_141414_childbb", 0));
        assert_eq!(alias.len(), 8);
        assert!(is_hermes_alias(&alias));
        assert_ne!(alias, hermes_alias_candidate("20260504_141414_childbb", 1));
    }

    #[test]
    fn session_artifact_id_keeps_full_hermes_id_without_registry() {
        assert_eq!(
            session_artifact_id("hermes", "20260504_141414_childbb"),
            "20260504_141414_childbb"
        );
        assert_eq!(
            session_artifact_id("grok", "019f46d3-79f7-74a2-a11c-0d1df7cc19be"),
            "019f46d3-79f7-74a2-a11c-0d1df7cc19be"
        );
        assert_eq!(session_artifact_id("claude", "abcdef123456"), "abcdef12");
    }

    #[test]
    fn grok_alias_candidate_uses_last_8_dash_stripped_hex() {
        assert_eq!(
            grok_alias_candidate("019f4711-2e99-78a3-a9ec-6fd31874b168").as_deref(),
            Some("1874b168")
        );
        assert!(is_grok_alias("1874b168"));
        assert_eq!(grok_alias_candidate("not-a-uuid"), None);
    }
}
