use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::db;
use crate::db::queries::SessionRow;
use crate::error::GaalError;
use crate::output::json::print_json;

/// Arguments for `gaal resolve`.
#[derive(Debug, Clone)]
pub struct ResolveArgs {
    /// Short session ID prefix.
    pub id: String,
    /// Optional engine filter.
    pub engine: Option<String>,
    /// Human-readable output mode.
    pub human: bool,
}

#[derive(Debug, Serialize)]
struct ResolveOutput {
    session_id: String,
    short_id: String,
    engine: String,
    jsonl_path: String,
    transcript_path: Option<String>,
    transcript_exists: bool,
    handoff_path: Option<String>,
    handoff_exists: bool,
    session_type: String,
    model: Option<String>,
}

#[derive(Debug, Clone)]
struct ResolvedPaths {
    jsonl_path: String,
    transcript_path: Option<String>,
    transcript_exists: bool,
    handoff_path: Option<String>,
    handoff_exists: bool,
}

/// Resolve a short session ID to session metadata and derived artifact paths.
pub fn run(args: ResolveArgs) -> Result<(), GaalError> {
    let conn = db::open_db_readonly()?;
    let matches = db::queries::resolve_by_prefix(&conn, &args.id, args.engine.as_deref())?;

    match matches.len() {
        0 => Err(GaalError::NotFound(args.id)),
        1 => {
            let session = &matches[0];
            let paths = compute_paths(session);
            if args.human {
                print_human(session, &paths);
            } else {
                let output = to_output(session, &paths);
                print_json(&output).map_err(GaalError::from)?;
            }
            Ok(())
        }
        _ => {
            eprintln_matches(&matches, args.human, &args.id);
            Err(GaalError::AmbiguousId(args.id))
        }
    }
}

fn to_output(session: &SessionRow, paths: &ResolvedPaths) -> ResolveOutput {
    ResolveOutput {
        session_id: session.id.clone(),
        short_id: short_id(&session.id),
        engine: session.engine.clone(),
        jsonl_path: paths.jsonl_path.clone(),
        transcript_path: paths.transcript_path.clone(),
        transcript_exists: paths.transcript_exists,
        handoff_path: paths.handoff_path.clone(),
        handoff_exists: paths.handoff_exists,
        session_type: session.session_type.clone(),
        model: session.model.clone(),
    }
}

fn compute_paths(session: &SessionRow) -> ResolvedPaths {
    let home = dirs::home_dir().unwrap_or_default();
    let gaal_home = std::env::var("GAAL_HOME")
        .ok()
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".gaal"));
    compute_paths_from_home(session, &gaal_home)
}

fn compute_paths_from_home(session: &SessionRow, gaal_home: &Path) -> ResolvedPaths {
    let date_parts = parse_date_parts(&session.started_at);
    let short_id = short_id(&session.id);

    let transcript_path = date_parts.as_ref().map(|(year, month, day)| {
        gaal_home
            .join("data")
            .join(&session.engine)
            .join("sessions")
            .join(year)
            .join(month)
            .join(day)
            .join(format!("{short_id}.md"))
    });

    let handoff_path = date_parts.as_ref().map(|(year, month, day)| {
        gaal_home
            .join("data")
            .join(&session.engine)
            .join("handoffs")
            .join(year)
            .join(month)
            .join(day)
            .join(format!("{short_id}.md"))
    });

    ResolvedPaths {
        jsonl_path: session.jsonl_path.clone(),
        transcript_path: transcript_path
            .as_ref()
            .map(|path| path.to_string_lossy().to_string()),
        transcript_exists: transcript_path.as_ref().is_some_and(|path| path.exists()),
        handoff_path: handoff_path
            .as_ref()
            .map(|path| path.to_string_lossy().to_string()),
        handoff_exists: handoff_path.as_ref().is_some_and(|path| path.exists()),
    }
}

fn parse_date_parts(started_at: &str) -> Option<(String, String, String)> {
    if started_at.len() < 10 {
        return None;
    }

    let mut parts = started_at[..10].split('-');
    let year = parts.next()?;
    let month = parts.next()?;
    let day = parts.next()?;
    if year.len() != 4 || month.len() != 2 || day.len() != 2 {
        return None;
    }

    Some((year.to_string(), month.to_string(), day.to_string()))
}

fn print_human(session: &SessionRow, paths: &ResolvedPaths) {
    let model = session.model.as_deref().unwrap_or("unknown");
    println!(
        "Session:    {} ({model}, {})",
        short_id(&session.id),
        session.session_type
    );
    println!("JSONL:      {}", compact_home(&paths.jsonl_path));
    println!(
        "Transcript: {} [{}]",
        display_path(&paths.transcript_path),
        if paths.transcript_exists {
            "ok"
        } else {
            "not rendered"
        }
    );
    println!(
        "Handoff:    {} [{}]",
        display_path(&paths.handoff_path),
        if paths.handoff_exists {
            "ok"
        } else {
            "not generated"
        }
    );
}

fn eprintln_matches(matches: &[SessionRow], human: bool, prefix: &str) {
    eprintln!("Multiple sessions match `{prefix}`:");
    for session in matches {
        let model = session.model.as_deref().unwrap_or("unknown");
        eprintln!(
            "  {}  {}  {}  {}",
            session.id, session.engine, model, session.session_type
        );
    }

    let has_claude = matches.iter().any(|session| session.engine == "claude");
    let has_codex = matches.iter().any(|session| session.engine == "codex");
    if has_claude && has_codex {
        if human {
            eprintln!("Hint: rerun with --engine claude or --engine codex.");
        } else {
            eprintln!("hint: rerun with --engine claude or --engine codex");
        }
    }
}

fn display_path(path: &Option<String>) -> String {
    path.as_deref()
        .map(compact_home)
        .unwrap_or_else(|| "(unknown)".to_string())
}

fn compact_home(path: &str) -> String {
    let Some(home) = dirs::home_dir() else {
        return path.to_string();
    };
    let home_str = home.to_string_lossy();
    if path == home_str.as_ref() {
        "~".to_string()
    } else if let Some(rest) = path.strip_prefix(home_str.as_ref()) {
        format!("~{rest}")
    } else {
        path.to_string()
    }
}

fn short_id(session_id: &str) -> String {
    session_id.chars().take(8).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn parse_date_parts_accepts_rfc3339_prefix() {
        assert_eq!(
            parse_date_parts("2026-03-30T17:18:59.339Z"),
            Some(("2026".to_string(), "03".to_string(), "30".to_string()))
        );
    }

    #[test]
    fn parse_date_parts_rejects_invalid_input() {
        assert_eq!(parse_date_parts("20260330"), None);
        assert_eq!(parse_date_parts("2026-3-30"), None);
    }

    #[test]
    fn compute_paths_uses_engine_date_and_short_id() {
        let base = unique_test_dir();
        let session = SessionRow {
            id: "dc5e98dc12345678".to_string(),
            engine: "claude".to_string(),
            model: Some("claude-opus-4-6".to_string()),
            cwd: None,
            started_at: "2026-03-30T17:18:59.339Z".to_string(),
            ended_at: None,
            exit_signal: None,
            last_event_at: None,
            parent_id: None,
            session_type: "coordinator".to_string(),
            jsonl_path: "/tmp/session.jsonl".to_string(),
            total_input_tokens: 0,
            total_output_tokens: 0,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
            reasoning_tokens: 0,
            total_tools: 0,
            total_turns: 0,
            peak_context: 0,
            last_indexed_offset: 0,
            subagent_type: None,
        };

        let transcript = base
            .join("data")
            .join("claude")
            .join("sessions")
            .join("2026")
            .join("03")
            .join("30")
            .join("dc5e98dc.md");
        let handoff = base
            .join("data")
            .join("claude")
            .join("handoffs")
            .join("2026")
            .join("03")
            .join("30")
            .join("dc5e98dc.md");
        fs::create_dir_all(transcript.parent().expect("transcript parent"))
            .expect("create transcript dir");
        fs::create_dir_all(handoff.parent().expect("handoff parent")).expect("create handoff dir");
        fs::write(&transcript, "rendered").expect("write transcript");
        fs::write(&handoff, "handoff").expect("write handoff");

        let paths = compute_paths_from_home(&session, &base);

        assert_eq!(paths.jsonl_path, "/tmp/session.jsonl");
        assert_eq!(
            paths.transcript_path,
            Some(transcript.to_string_lossy().to_string())
        );
        assert!(paths.transcript_exists);
        assert_eq!(
            paths.handoff_path,
            Some(handoff.to_string_lossy().to_string())
        );
        assert!(paths.handoff_exists);

        fs::remove_dir_all(&base).expect("cleanup");
    }

    fn unique_test_dir() -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("unix time")
            .as_nanos();
        let base = std::env::temp_dir().join(format!("gaal-resolve-test-{nonce}"));
        fs::create_dir_all(&base).expect("create temp dir");
        base
    }
}
