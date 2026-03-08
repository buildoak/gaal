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

/// Write content to a file atomically using a temporary file and rename.
/// This prevents partial writes if the process is killed mid-write.
pub fn atomic_write(path: &Path, content: &str) -> std::io::Result<()> {
    let tmp_path = path.with_extension("tmp");
    fs::write(&tmp_path, content)?;
    fs::rename(&tmp_path, path)?;
    Ok(())
}
