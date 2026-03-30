use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, NaiveDate, Utc};
use clap::{Args, ValueEnum};
use rusqlite::Connection;
use serde::Serialize;

use crate::commands::inspect::{find_latest_session_id, find_session_ids_by_prefix};
use crate::db::open_db_readonly;
use crate::db::queries::{get_facts, get_handoff};
use crate::error::GaalError;
use crate::model::{Fact, FactType, HandoffRecord};
use crate::output::json::print_json;

/// CLI arguments for `gaal recall`.
#[derive(Debug, Clone, Args)]
pub struct RecallArgs {
    /// Query text for semantic session lookup.
    pub query: Option<String>,
    /// Direct handoff lookup by session ID. Bypasses semantic search.
    /// Supports ID prefix and `latest`. Mutually exclusive with QUERY.
    pub id: Option<String>,
    /// Recency window in days.
    #[arg(long, default_value_t = 14)]
    pub days_back: i64,
    /// Max number of sessions to return.
    #[arg(long, default_value_t = 3)]
    pub limit: usize,
    /// Output format.
    #[arg(long, value_enum, default_value_t = RecallFormat::Brief)]
    pub format: RecallFormat,
    /// Minimum substance score.
    #[arg(long, default_value_t = 1)]
    pub substance: i32,
    /// Print human-readable output.
    #[arg(short = 'H', action = clap::ArgAction::SetTrue)]
    pub human: bool,
}

/// Output format options for recall.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, ValueEnum)]
pub enum RecallFormat {
    /// Structured summary fields.
    Summary,
    /// Condensed 3-5 line block per session.
    #[default]
    Brief,
    /// Raw handoff markdown content.
    Handoff,
    /// Summary + handoff + files + errors.
    Full,
    /// Eywa-compatible plain markdown output.
    Eywa,
}

#[derive(Debug, Clone)]
struct RecallSession {
    handoff: HandoffRecord,
    started_at: String,
    session_date: NaiveDate,
    project_tokens: HashSet<String>,
    keyword_tokens: HashSet<String>,
    headline_tokens: HashSet<String>,
}

#[derive(Debug, Clone)]
struct ScoredSession {
    session: RecallSession,
    score: f64,
}

#[derive(Debug, Serialize)]
struct RecallSummary {
    session_id: String,
    date: String,
    headline: Option<String>,
    handoff_path: Option<String>,
    projects: Vec<String>,
    keywords: Vec<String>,
    substance: i32,
    duration_minutes: i32,
    score: f64,
}

#[derive(Debug, Serialize)]
struct HandoffOutput {
    summary: RecallSummary,
    handoff: Option<String>,
}

#[derive(Debug, Serialize)]
struct FullOutput {
    summary: RecallSummary,
    handoff: Option<String>,
    files: FileSets,
    errors: Vec<ErrorOutput>,
}

#[derive(Debug, Default, Serialize)]
struct FileSets {
    read: Vec<String>,
    written: Vec<String>,
}

#[derive(Debug, Serialize)]
struct ErrorOutput {
    ts: String,
    subject: Option<String>,
    detail: Option<String>,
    exit_code: Option<i32>,
}

/// Execute `gaal recall`.
pub fn run(args: RecallArgs) -> Result<(), GaalError> {
    // --id and QUERY are mutually exclusive
    if args.id.is_some() && args.query.is_some() {
        return Err(GaalError::ParseError(
            "recall --id and QUERY are mutually exclusive; use one or the other".to_string(),
        ));
    }

    // Direct lookup by session ID
    if let Some(ref raw_id) = args.id {
        return run_by_id(raw_id, &args);
    }

    if args.query.is_none() {
        print_recall_help();
        return Ok(());
    }

    let conn = open_db_readonly()?;
    let sessions = load_all_handoffs(&conn)?;
    if sessions.is_empty() {
        return Err(GaalError::NoResults);
    }

    let min_substance = args.substance.max(1);
    let known_projects = collect_known_project_tokens(&sessions);
    let query_tokens = tokenize_query(args.query.as_deref(), &known_projects);
    let mut ranked = score_sessions(&sessions, &query_tokens, args.days_back, min_substance);

    if ranked.is_empty() {
        ranked = fallback_recent_substantive(&sessions, min_substance, args.limit);
    } else if ranked.len() > args.limit {
        ranked.truncate(args.limit);
    }

    if ranked.is_empty() {
        return Err(GaalError::NoResults);
    }

    match args.format {
        RecallFormat::Summary => render_summary(&ranked, args.human),
        RecallFormat::Brief => render_brief(&ranked, args.human),
        RecallFormat::Handoff => render_handoff(&ranked, args.human),
        RecallFormat::Full => render_full(&conn, &ranked, args.human),
        RecallFormat::Eywa => render_eywa(&ranked, args.human),
    }
}

/// Direct handoff retrieval by session ID. Bypasses semantic search entirely.
fn run_by_id(raw_id: &str, args: &RecallArgs) -> Result<(), GaalError> {
    let conn = open_db_readonly()?;

    // Resolve the session ID: support `latest` and prefix matching
    let session_id = if raw_id == "latest" {
        find_latest_session_id(&conn)?
    } else {
        let matches = find_session_ids_by_prefix(&conn, raw_id)?;
        match matches.len() {
            0 => return Err(GaalError::NotFound(raw_id.to_string())),
            1 => matches.into_iter().next().unwrap(),
            _ => return Err(GaalError::AmbiguousId(raw_id.to_string())),
        }
    };

    // Look up the handoff for this session
    let handoff = get_handoff(&conn, &session_id)?;
    let Some(handoff) = handoff else {
        return Err(GaalError::NotFound(format!(
            "handoff:{session_id}"
        )));
    };

    // Build a started_at from the sessions table for date display
    let started_at: String = conn
        .query_row(
            "SELECT started_at FROM sessions WHERE id = ?1",
            [&session_id],
            |row| row.get(0),
        )
        .unwrap_or_else(|_| String::new());
    let session_date =
        parse_session_date(&started_at).unwrap_or_else(|| Utc::now().date_naive());

    let recall_session = RecallSession {
        handoff,
        started_at,
        session_date,
        project_tokens: HashSet::new(),
        keyword_tokens: HashSet::new(),
        headline_tokens: HashSet::new(),
    };
    let scored = ScoredSession {
        session: recall_session,
        score: 0.0,
    };
    let ranked = vec![scored];

    match args.format {
        RecallFormat::Summary => render_summary(&ranked, args.human),
        RecallFormat::Brief => render_brief(&ranked, args.human),
        RecallFormat::Handoff => render_handoff(&ranked, args.human),
        RecallFormat::Full => render_full(&conn, &ranked, args.human),
        RecallFormat::Eywa => render_eywa(&ranked, args.human),
    }
}

fn load_all_handoffs(conn: &Connection) -> Result<Vec<RecallSession>, GaalError> {
    let mut stmt = conn.prepare(
        r#"
        SELECT
            h.session_id,
            h.headline,
            h.projects,
            h.keywords,
            h.substance,
            h.duration_minutes,
            h.generated_at,
            h.generated_by,
            h.content_path,
            s.started_at
        FROM handoffs h
        INNER JOIN sessions s ON s.id = h.session_id
        "#,
    )?;
    let mut rows = stmt.query([])?;

    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let session_id: String = row.get("session_id")?;
        let headline: Option<String> = row.get("headline")?;
        let projects_raw: Option<String> = row.get("projects")?;
        let keywords_raw: Option<String> = row.get("keywords")?;
        let substance: i32 = row.get("substance")?;
        let duration_minutes: i32 = row.get("duration_minutes")?;
        let generated_at: Option<String> = row.get("generated_at")?;
        let generated_by: Option<String> = row.get("generated_by")?;
        let content_path: Option<String> = row.get("content_path")?;
        let started_at: String = row.get("started_at")?;

        let projects = parse_string_vec(projects_raw);
        let keywords = parse_string_vec(keywords_raw);
        let session_date =
            parse_session_date(&started_at).unwrap_or_else(|| Utc::now().date_naive());
        let project_tokens = tokenize_fields(&projects);
        let keyword_tokens = tokenize_fields(&keywords);
        let headline_tokens: HashSet<String> = headline
            .as_deref()
            .map(|h| {
                split_tokens(h)
                    .into_iter()
                    .filter(|t| !STOPWORDS.contains(&t.as_str()))
                    .collect()
            })
            .unwrap_or_default();

        out.push(RecallSession {
            handoff: HandoffRecord {
                session_id,
                headline,
                projects,
                keywords,
                substance,
                duration_minutes,
                generated_at,
                generated_by,
                content_path,
            },
            started_at,
            session_date,
            project_tokens,
            keyword_tokens,
            headline_tokens,
        });
    }

    Ok(out)
}

fn score_sessions(
    sessions: &[RecallSession],
    query_tokens: &[String],
    days_back: i64,
    min_substance: i32,
) -> Vec<ScoredSession> {
    if query_tokens.is_empty() {
        return Vec::new();
    }

    let n_total = sessions.len() as f64;
    let mut idf = HashMap::with_capacity(query_tokens.len());
    for token in query_tokens {
        let mut df = 0.0_f64;
        for session in sessions {
            if session.project_tokens.contains(token)
                || session.keyword_tokens.contains(token)
                || session.headline_tokens.contains(token)
            {
                df += 1.0;
            }
        }
        let value = if df > 0.0 {
            (n_total / df).ln().max(0.0)
        } else {
            0.0
        };
        idf.insert(token.as_str(), value);
    }

    let today = Utc::now().date_naive();
    let mut scored = Vec::new();
    for session in sessions {
        if session.handoff.substance < min_substance {
            continue;
        }

        let mut score = 0.0_f64;
        for token in query_tokens {
            let token_idf = *idf.get(token.as_str()).unwrap_or(&0.0);
            if session.project_tokens.contains(token) {
                score += 3.0 * token_idf;
            }
            if session.keyword_tokens.contains(token) {
                score += 2.0 * token_idf;
            }
            if session.headline_tokens.contains(token) {
                score += 2.5 * token_idf;
            }
        }
        if score <= 0.0 {
            continue;
        }

        let raw_age = (today - session.session_date).num_days();
        let age_days = raw_age.max(1) as f64;
        let window = days_back.max(0) as f64;
        if age_days <= window {
            score *= 1.0 + 1.0 / age_days.sqrt();
        } else {
            score *= 0.5_f64.powf((age_days - window) / 7.0);
        }

        let duration_minutes = f64::from(session.handoff.duration_minutes.max(0));
        score *= 1.0 + 0.1 * (duration_minutes + 1.0).ln();

        scored.push(ScoredSession {
            session: session.clone(),
            score,
        });
    }

    scored.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| b.session.started_at.cmp(&a.session.started_at))
    });
    scored
}

fn fallback_recent_substantive(
    sessions: &[RecallSession],
    min_substance: i32,
    limit: usize,
) -> Vec<ScoredSession> {
    let mut out: Vec<ScoredSession> = sessions
        .iter()
        .filter(|session| session.handoff.substance >= min_substance)
        .cloned()
        .map(|session| ScoredSession {
            session,
            score: 0.0,
        })
        .collect();
    out.sort_by(|a, b| b.session.started_at.cmp(&a.session.started_at));
    if out.len() > limit {
        out.truncate(limit);
    }
    out
}

fn render_summary(results: &[ScoredSession], human: bool) -> Result<(), GaalError> {
    let summaries: Vec<RecallSummary> = results.iter().map(to_summary).collect();
    if human {
        for (idx, summary) in summaries.iter().enumerate() {
            if idx > 0 {
                println!();
            }
            print_human_session_header(summary);
            println!(
                "Substance: {} | Duration: {}m | Score: {:.1}",
                summary.substance, summary.duration_minutes, summary.score
            );
        }
        Ok(())
    } else {
        print_json(&summaries)?;
        Ok(())
    }
}

fn render_brief(results: &[ScoredSession], human: bool) -> Result<(), GaalError> {
    if human {
        for (idx, row) in results.iter().enumerate() {
            if idx > 0 {
                println!();
            }
            let summary = to_summary(row);
            print_human_session_header(&summary);
            println!(
                "Substance: {} | Duration: {}m | Score: {:.1}",
                summary.substance, summary.duration_minutes, summary.score
            );
        }
        Ok(())
    } else {
        let briefs: Vec<String> = results.iter().map(to_brief_block).collect();
        print_json(&briefs)?;
        Ok(())
    }
}

fn render_handoff(results: &[ScoredSession], human: bool) -> Result<(), GaalError> {
    let payload: Vec<HandoffOutput> = results
        .iter()
        .map(|row| HandoffOutput {
            summary: to_summary(row),
            handoff: read_handoff_markdown(row.session.handoff.content_path.as_deref()),
        })
        .collect();

    if human {
        for (idx, item) in payload.iter().enumerate() {
            if idx > 0 {
                println!();
            }
            print_human_session_header(&item.summary);
            println!(
                "Substance: {} | Duration: {}m | Score: {:.1}",
                item.summary.substance, item.summary.duration_minutes, item.summary.score
            );
            if let Some(ref content) = item.handoff {
                println!();
                println!("{content}");
            } else {
                println!("(handoff markdown unavailable)");
            }
        }
        Ok(())
    } else {
        print_json(&payload)?;
        Ok(())
    }
}

fn render_full(conn: &Connection, results: &[ScoredSession], human: bool) -> Result<(), GaalError> {
    let mut out = Vec::with_capacity(results.len());
    for row in results {
        let facts = get_facts(conn, &row.session.handoff.session_id, None)?;
        let (files, errors) = extract_files_and_errors(&facts);
        out.push(FullOutput {
            summary: to_summary(row),
            handoff: read_handoff_markdown(row.session.handoff.content_path.as_deref()),
            files,
            errors,
        });
    }

    if human {
        for (idx, item) in out.iter().enumerate() {
            if idx > 0 {
                println!();
            }
            print_human_session_header(&item.summary);
            println!(
                "Substance: {} | Duration: {}m | Score: {:.1}",
                item.summary.substance, item.summary.duration_minutes, item.summary.score
            );
            if let Some(ref content) = item.handoff {
                println!();
                println!("{content}");
            } else {
                println!("(handoff markdown unavailable)");
            }
            println!("Files read: {}", comma_list(&item.files.read));
            println!("Files written: {}", comma_list(&item.files.written));
            if item.errors.is_empty() {
                println!("Errors: none");
            } else {
                println!("Errors:");
                for err in &item.errors {
                    println!(
                        "  - [{}] {} | exit={:?}",
                        err.ts,
                        err.subject.as_deref().unwrap_or("unknown"),
                        err.exit_code
                    );
                }
            }
        }
        Ok(())
    } else {
        print_json(&out)?;
        Ok(())
    }
}

fn render_eywa(results: &[ScoredSession], _human: bool) -> Result<(), GaalError> {
    let count = results.len();
    println!("## Gaal: {} past sessions\n", count);

    for (idx, row) in results.iter().enumerate() {
        let date = row.session.session_date.to_string();
        let headline = row
            .session
            .handoff
            .headline
            .as_deref()
            .unwrap_or("(no headline)");
        println!("### {} --- {}\n", date, headline);

        // Read handoff markdown and strip YAML frontmatter
        if let Some(content) = read_handoff_markdown(row.session.handoff.content_path.as_deref()) {
            let stripped = strip_yaml_frontmatter(&content);
            println!("{}", stripped.trim());
        } else {
            println!("(handoff content unavailable)");
        }

        if idx < count - 1 {
            println!("\n---\n");
        }
    }

    Ok(())
}

/// Strip YAML frontmatter (content between --- delimiters at the start of the document).
fn strip_yaml_frontmatter(content: &str) -> &str {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return content;
    }
    // Find the closing ---
    let after_first = &trimmed[3..];
    if let Some(end_pos) = after_first.find("\n---") {
        let remainder = &after_first[end_pos + 4..];
        // Skip any trailing newline after the closing ---
        remainder.strip_prefix('\n').unwrap_or(remainder)
    } else {
        content
    }
}

/// Print a structured session header for human-readable output.
fn print_human_session_header(summary: &RecallSummary) {
    let short_id = summary.session_id.chars().take(8).collect::<String>();
    println!(
        "\u{2501}\u{2501}\u{2501} Session {} ({}) \u{2501}\u{2501}\u{2501}",
        short_id, summary.date
    );
    println!(
        "Headline: {}",
        summary.headline.as_deref().unwrap_or("(no headline)")
    );
    println!("Projects: {}", comma_list(&summary.projects));
    println!("Keywords: {}", comma_list(&summary.keywords));
}

fn to_summary(row: &ScoredSession) -> RecallSummary {
    RecallSummary {
        session_id: row.session.handoff.session_id.clone(),
        date: row.session.session_date.to_string(),
        headline: row.session.handoff.headline.clone(),
        handoff_path: row.session.handoff.content_path.clone(),
        projects: row.session.handoff.projects.clone(),
        keywords: row.session.handoff.keywords.clone(),
        substance: row.session.handoff.substance,
        duration_minutes: row.session.handoff.duration_minutes,
        score: row.score,
    }
}

fn to_brief_block(row: &ScoredSession) -> String {
    let summary = to_summary(row);
    let headline = summary.headline.as_deref().unwrap_or("(no headline)");
    let projects = comma_list(&summary.projects);
    let keywords = comma_list(&summary.keywords);
    format!(
        "session: {}\ndate: {}\nheadline: {}\nprojects: {}\nkeywords: {}\nsubstance: {} duration_minutes: {} score: {:.3}",
        summary.session_id,
        summary.date,
        headline,
        projects,
        keywords,
        summary.substance,
        summary.duration_minutes,
        summary.score
    )
}

fn extract_files_and_errors(facts: &[Fact]) -> (FileSets, Vec<ErrorOutput>) {
    let mut read_files: HashSet<String> = HashSet::new();
    let mut written_files: HashSet<String> = HashSet::new();
    let mut errors = Vec::new();

    for fact in facts {
        match fact.fact_type {
            FactType::FileRead => {
                if let Some(path) = fact.subject.as_deref() {
                    read_files.insert(path.to_string());
                }
            }
            FactType::FileWrite => {
                if let Some(path) = fact.subject.as_deref() {
                    written_files.insert(path.to_string());
                }
            }
            FactType::Error => {
                errors.push(ErrorOutput {
                    ts: fact.ts.clone(),
                    subject: fact.subject.clone(),
                    detail: fact.detail.clone(),
                    exit_code: fact.exit_code,
                });
            }
            _ => {}
        }
    }

    let mut read = read_files.into_iter().collect::<Vec<_>>();
    let mut written = written_files.into_iter().collect::<Vec<_>>();
    read.sort();
    written.sort();

    (FileSets { read, written }, errors)
}

fn tokenize_query(query: Option<&str>, _known_projects: &HashSet<String>) -> Vec<String> {
    let Some(query_text) = query else {
        return Vec::new();
    };

    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for token in split_tokens(query_text) {
        if STOPWORDS.contains(&token.as_str()) {
            continue;
        }
        if seen.insert(token.clone()) {
            out.push(token);
        }
    }
    out
}

fn tokenize_fields(items: &[String]) -> HashSet<String> {
    let mut set = HashSet::new();
    for item in items {
        for token in split_tokens(item) {
            set.insert(token);
        }
    }
    set
}

fn split_tokens(text: &str) -> Vec<String> {
    text.to_ascii_lowercase()
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect()
}

fn collect_known_project_tokens(sessions: &[RecallSession]) -> HashSet<String> {
    let mut known = HashSet::new();
    for session in sessions {
        known.extend(session.project_tokens.iter().cloned());
    }
    known
}

fn parse_string_vec(raw: Option<String>) -> Vec<String> {
    let Some(value) = raw else {
        return Vec::new();
    };
    if value.trim().is_empty() {
        return Vec::new();
    }

    if let Ok(parsed) = serde_json::from_str::<Vec<String>>(&value) {
        return parsed;
    }
    if let Ok(parsed) = serde_json::from_str::<Vec<serde_json::Value>>(&value) {
        return parsed
            .into_iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect();
    }
    vec![value]
}

fn parse_session_date(timestamp: &str) -> Option<NaiveDate> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(timestamp) {
        return Some(dt.date_naive());
    }
    if let Ok(date) = NaiveDate::parse_from_str(timestamp, "%Y-%m-%d") {
        return Some(date);
    }
    timestamp
        .get(0..10)
        .and_then(|prefix| NaiveDate::parse_from_str(prefix, "%Y-%m-%d").ok())
}

fn read_handoff_markdown(path: Option<&str>) -> Option<String> {
    let raw = path?;
    let expanded = expand_home(raw);
    fs::read_to_string(expanded).ok()
}

fn expand_home(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    Path::new(path).to_path_buf()
}

fn comma_list(values: &[String]) -> String {
    if values.is_empty() {
        "-".to_string()
    } else {
        values.join(", ")
    }
}

fn print_recall_help() {
    eprintln!("gaal recall — Ranked session retrieval for continuity and context");
    eprintln!();
    eprintln!("Usage: gaal recall <query> [flags]");
    eprintln!("       gaal recall --id <session-id> [flags]");
    eprintln!();
    eprintln!("Flags:");
    eprintln!("  --id <id>          Direct handoff lookup by session ID (bypasses search)");
    eprintln!("  --days-back <n>    Recency window in days (default: 14)");
    eprintln!("  --limit <n>        Max number of sessions to return (default: 3)");
    eprintln!(
        "  --format <fmt>     Output format: summary, brief, handoff, full, eywa (default: brief)"
    );
    eprintln!("  --substance <n>    Minimum substance score (default: 1)");
    eprintln!("  -H                 Human-readable output");
    eprintln!();
    eprintln!("Examples:");
    eprintln!("  gaal recall \"gaussian moat\" -H");
    eprintln!("  gaal recall \"auth migration\" --days-back 30 --limit 5");
    eprintln!("  gaal recall \"deploy\" --format handoff");
    eprintln!("  gaal recall --id abc12345 --format brief -H");
    eprintln!("  gaal recall --id latest -H");
}

const STOPWORDS: &[&str] = &[
    "the", "a", "an", "is", "was", "are", "were", "it", "its", "in", "on", "at", "to", "for", "of",
    "with", "and", "or", "but", "not", "this", "that", "by", "from",
];
