use std::fs;
use std::path::{Path, PathBuf};

use chrono::Local;
use serde::Serialize;

use crate::commands::inspect::resolve_one;
use crate::config::{gaal_home, load_config};
use crate::db::open_db_readonly;
use crate::db::queries::SessionRow;
use crate::error::GaalError;
use crate::output::json::print_json;

/// CLI arguments for `gaal transcript`.
#[derive(Debug, Clone)]
pub struct TranscriptArgs {
    /// Session ID or ID prefix. Use `latest` to resolve the newest session.
    pub id: Option<String>,
    /// Re-render even if cached file exists.
    pub force: bool,
    /// Dump markdown to stdout instead of returning file path JSON.
    pub stdout: bool,
    /// Render human-readable output.
    pub human: bool,
}

#[derive(Debug, Serialize)]
struct TranscriptResult {
    path: String,
    size_bytes: u64,
    estimated_tokens: u64,
    warning: String,
}

#[derive(Debug, Clone)]
struct TranscriptPaths {
    gaal_path: PathBuf,
    external_path: Option<PathBuf>,
}

/// Execute the `gaal transcript` command.
pub fn run(args: TranscriptArgs) -> Result<(), GaalError> {
    let Some(raw_id) = args.id else {
        print_transcript_help();
        return Ok(());
    };

    let conn = open_db_readonly()?;
    let session = resolve_one(&conn, &raw_id).map_err(map_session_resolution_error)?;
    let config = load_config();
    let paths = transcript_paths(&session, config.markdown_output_dir.as_deref());
    let md_path = resolve_markdown_path(&session, &paths, args.force)?;

    if args.stdout {
        let markdown = read_markdown_file(&md_path)?;
        print!("{markdown}");
        return Ok(());
    }

    let size_bytes = file_size_bytes(&md_path)?;
    let estimated_tokens = size_bytes / 4;
    let warning = build_warning(estimated_tokens);
    let result = TranscriptResult {
        path: absolute_display_path(&md_path)?,
        size_bytes,
        estimated_tokens,
        warning,
    };

    if args.human {
        println!("Transcript: {}", result.path);
        println!("Size: {} bytes", result.size_bytes);
        println!("Estimated tokens: {}", result.estimated_tokens);
        println!("{}", result.warning);
        return Ok(());
    }

    print_json(&result).map_err(GaalError::from)
}

fn map_session_resolution_error(err: GaalError) -> GaalError {
    match err {
        GaalError::NotFound(id) => GaalError::NotFound(format!(
            "{id}; hint: run 'gaal ls' to see available sessions"
        )),
        other => other,
    }
}

fn resolve_markdown_path(
    session: &SessionRow,
    paths: &TranscriptPaths,
    force: bool,
) -> Result<PathBuf, GaalError> {
    if !force {
        if paths.gaal_path.exists() {
            return Ok(paths.gaal_path.clone());
        }
        if let Some(external_path) = &paths.external_path {
            if external_path.exists() {
                return Ok(external_path.clone());
            }
        }
    }

    let markdown = render_markdown(session)?;
    write_markdown_file(&paths.gaal_path, &markdown)?;
    Ok(paths.gaal_path.clone())
}

fn render_markdown(session: &SessionRow) -> Result<String, GaalError> {
    let jsonl_path = Path::new(&session.jsonl_path);
    if !jsonl_path.exists() {
        return Err(GaalError::NotFound(format!(
            "JSONL source file not found: {}",
            session.jsonl_path
        )));
    }

    crate::render::session_md::render_session_markdown(jsonl_path)
        .map_err(|e| GaalError::Internal(format!("failed to render session markdown: {e}")))
}

fn write_markdown_file(path: &Path, content: &str) -> Result<(), GaalError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            GaalError::Internal(format!(
                "failed to write transcript file at {}: {}",
                path.display(),
                err
            ))
        })?;
    }

    crate::util::atomic_write(path, content).map_err(|err| {
        GaalError::Internal(format!(
            "failed to write transcript file at {}: {}",
            path.display(),
            err
        ))
    })
}

fn read_markdown_file(path: &Path) -> Result<String, GaalError> {
    fs::read_to_string(path).map_err(|err| {
        GaalError::Internal(format!(
            "failed to read transcript file at {}: {}",
            path.display(),
            err
        ))
    })
}

fn transcript_paths(session: &SessionRow, output_dir: Option<&Path>) -> TranscriptPaths {
    let short_id = session.id.chars().take(8).collect::<String>();
    let (year, month, day) = date_parts(&session.started_at);

    let gaal_path = gaal_home()
        .join("data")
        .join(&session.engine)
        .join("sessions")
        .join(&year)
        .join(&month)
        .join(&day)
        .join(format!("{short_id}.md"));

    let external_path = output_dir.map(|dir| {
        dir.join(&year)
            .join(&month)
            .join(&day)
            .join(format!("{short_id}.md"))
    });

    TranscriptPaths {
        gaal_path,
        external_path,
    }
}

fn file_size_bytes(path: &Path) -> Result<u64, GaalError> {
    fs::metadata(path)
        .map(|metadata| metadata.len())
        .map_err(|err| {
            GaalError::Internal(format!(
                "failed to read transcript file size at {}: {}",
                path.display(),
                err
            ))
        })
}

fn absolute_display_path(path: &Path) -> Result<String, GaalError> {
    if path.is_absolute() {
        return Ok(path.to_string_lossy().to_string());
    }

    let cwd = std::env::current_dir().map_err(|err| {
        GaalError::Internal(format!(
            "failed to resolve absolute transcript path for {}: {}",
            path.display(),
            err
        ))
    })?;
    Ok(cwd.join(path).to_string_lossy().to_string())
}

fn build_warning(estimated_tokens: u64) -> String {
    let tokens_k = estimated_tokens / 1_000;
    format!(
        "~{}K tokens. Recommend reading via subagent, not coordinator context.",
        tokens_k
    )
}

fn date_parts(started_at: &str) -> (String, String, String) {
    let fallback = || {
        let now = Local::now();
        (
            now.format("%Y").to_string(),
            now.format("%m").to_string(),
            now.format("%d").to_string(),
        )
    };

    let Some(prefix) = started_at.get(0..10) else {
        return fallback();
    };
    let mut parts = prefix.split('-');
    let year = parts.next().unwrap_or_default();
    let month = parts.next().unwrap_or_default();
    let day = parts.next().unwrap_or_default();

    if year.len() == 4 && month.len() == 2 && day.len() == 2 {
        (year.to_string(), month.to_string(), day.to_string())
    } else {
        fallback()
    }
}

fn print_transcript_help() {
    eprintln!("gaal transcript — Get session transcript markdown (replaces inspect --markdown)");
    eprintln!();
    eprintln!("Usage: gaal transcript <session-id> [flags]");
    eprintln!();
    eprintln!("Arguments:");
    eprintln!("  <session-id>    Session ID, ID prefix, or `latest`");
    eprintln!();
    eprintln!("Flags:");
    eprintln!("  --force         Re-render even if cached markdown exists");
    eprintln!("  --stdout        Print markdown to stdout (no JSON wrapper)");
    eprintln!("  -H, --human     Print a human-readable summary instead of JSON");
    eprintln!();
    eprintln!("Examples:");
    eprintln!("  gaal transcript latest");
    eprintln!("  gaal transcript 249aad1e");
    eprintln!("  gaal transcript latest --stdout");
    eprintln!("  gaal transcript latest --force");
    eprintln!("  gaal ls    # if session resolution fails");
}
