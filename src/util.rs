use std::fs;
use std::path::Path;

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

/// Return the filename/session-display identifier used for derived artifacts.
///
/// Claude, Codex, and Gemini keep the historic first-8 convention. Hermes IDs
/// start with date/time components, so first-8 aliases collide across sessions.
pub fn session_artifact_id(engine: &str, id: &str) -> String {
    let sanitized = sanitize_filename(id);
    if engine == "hermes" {
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
