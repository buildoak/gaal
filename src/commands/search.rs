use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use anyhow::anyhow;
use chrono::{DateTime, NaiveDate, Utc};
use clap::{Args, ValueEnum};
use rusqlite::{named_params, Connection};
use serde::Serialize;
use tantivy::collector::TopDocs;
use tantivy::query::{BooleanQuery, Occur, Query, QueryParser, TermQuery};
use tantivy::schema::{
    Field, IndexRecordOption, Schema, SchemaBuilder, TantivyDocument, Value, INDEXED, STORED,
    STRING, TEXT,
};
use tantivy::snippet::SnippetGenerator;
use tantivy::{doc, Index, ReloadPolicy, Term};

use crate::config::gaal_home;
use crate::db::open_db_readonly;
use crate::error::GaalError;
use crate::output::human::{format_timestamp, print_table};
use crate::output::json::print_json;

/// Arguments for `gaal search`.
#[derive(Debug, Clone, Args)]
pub struct SearchArgs {
    /// Free-text query.
    pub query: String,
    /// Time window lower bound (for example: 30d, 12h, 2026-03-01, RFC3339).
    #[arg(long, default_value = "30d")]
    pub since: String,
    /// Restrict to sessions whose CWD contains this substring.
    #[arg(long)]
    pub cwd: Option<String>,
    /// Restrict to one engine.
    #[arg(long)]
    pub engine: Option<String>,
    /// Restrict to a fact field group.
    #[arg(long, value_enum, default_value_t = SearchField::All)]
    pub field: SearchField,
    /// Context amount used to size search snippets.
    #[arg(long, default_value_t = 2)]
    pub context: usize,
    /// Maximum number of rows returned.
    #[arg(long, default_value_t = 20)]
    pub limit: usize,
    /// Human-readable output (otherwise JSON).
    #[arg(short = 'H', long = "human")]
    pub human: bool,
}

/// Search field groups for fact type filtering.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum SearchField {
    Prompts,
    Replies,
    Commands,
    Errors,
    Files,
    All,
}

impl SearchField {
    fn fact_types(self) -> Option<&'static [&'static str]> {
        match self {
            Self::Prompts => Some(&["user_prompt"]),
            Self::Replies => Some(&["assistant_reply"]),
            Self::Commands => Some(&["command"]),
            Self::Errors => Some(&["error"]),
            Self::Files => Some(&["file_read", "file_write"]),
            Self::All => None,
        }
    }
}

/// One matched fact row returned by search.
#[derive(Debug, Clone, Serialize)]
pub struct SearchResult {
    pub session_id: String,
    pub engine: String,
    pub turn: u64,
    pub fact_type: String,
    pub subject: String,
    pub snippet: String,
    pub ts: String,
    pub score: f32,
    pub session_headline: String,
}

#[derive(Debug, Clone, Copy)]
struct SearchIndexFields {
    session_id: Field,
    engine: Field,
    turn: Field,
    fact_type: Field,
    subject: Field,
    detail: Field,
    ts: Field,
    session_headline: Field,
}

/// Executes `gaal search`.
pub fn run(args: SearchArgs) -> Result<(), GaalError> {
    let conn = open_db_readonly()?;
    let since_bound = parse_since_bound(&args.since)?;
    let fetch_limit = args.limit.max(1).saturating_mul(10);
    let mut results =
        search_facts_with_context(&args.query, args.field, fetch_limit, args.context)?;

    if let Some(engine) = args.engine.as_deref() {
        results.retain(|row| row.engine == engine);
    }

    if let Some(cwd) = args.cwd.as_deref() {
        let allowed = session_ids_for_cwd(&conn, cwd)?;
        results.retain(|row| allowed.contains(&row.session_id));
    }

    results.retain(|row| fact_is_after(&row.ts, &since_bound));
    results.truncate(args.limit.max(1));
    if results.is_empty() {
        return Err(GaalError::NoResults);
    }

    if args.human {
        print_search_human(&results);
        return Ok(());
    }

    print_json(&results).map_err(GaalError::from)
}

/// Rebuild the Tantivy search index from all indexed facts.
pub fn build_search_index(conn: &Connection) -> Result<(), GaalError> {
    let index = open_or_create_index()?;
    let fields = resolve_fields(&index.schema())?;
    let mut writer = index.writer(50_000_000).map_err(map_tantivy_err)?;
    writer.delete_all_documents().map_err(map_tantivy_err)?;

    let mut stmt = conn
        .prepare(
            r#"
            SELECT
                f.session_id,
                s.engine,
                f.turn_number,
                f.fact_type,
                COALESCE(f.subject, ''),
                COALESCE(f.detail, ''),
                f.ts,
                COALESCE(h.headline, '')
            FROM facts f
            INNER JOIN sessions s ON s.id = f.session_id
            LEFT JOIN handoffs h ON h.session_id = f.session_id
            ORDER BY f.id ASC
            "#,
        )
        .map_err(GaalError::from)?;

    let mut rows = stmt.query([]).map_err(GaalError::from)?;
    while let Some(row) = rows.next().map_err(GaalError::from)? {
        let session_id: String = row.get(0).map_err(GaalError::from)?;
        let engine: String = row.get(1).map_err(GaalError::from)?;
        let turn: Option<i64> = row.get(2).map_err(GaalError::from)?;
        let fact_type: String = row.get(3).map_err(GaalError::from)?;
        let subject: String = row.get(4).map_err(GaalError::from)?;
        let detail: String = row.get(5).map_err(GaalError::from)?;
        let ts: String = row.get(6).map_err(GaalError::from)?;
        let session_headline: String = row.get(7).map_err(GaalError::from)?;

        writer
            .add_document(doc!(
                fields.session_id => session_id,
                fields.engine => engine,
                fields.turn => turn_to_u64(turn),
                fields.fact_type => fact_type,
                fields.subject => subject,
                fields.detail => detail,
                fields.ts => ts,
                fields.session_headline => session_headline
            ))
            .map_err(map_tantivy_err)?;
    }

    writer.commit().map_err(map_tantivy_err)?;
    writer.wait_merging_threads().map_err(map_tantivy_err)?;
    Ok(())
}

/// Search facts with BM25 over subject/detail text.
pub fn search_facts(
    query: &str,
    field_filter: SearchField,
    limit: usize,
) -> Result<Vec<SearchResult>, GaalError> {
    search_facts_with_context(query, field_filter, limit, 2)
}

fn search_facts_with_context(
    query: &str,
    field_filter: SearchField,
    limit: usize,
    context: usize,
) -> Result<Vec<SearchResult>, GaalError> {
    if query.trim().is_empty() {
        return Err(GaalError::ParseError("query cannot be empty".to_string()));
    }

    let index = open_existing_index()?;
    let fields = resolve_fields(&index.schema())?;
    let reader = index
        .reader_builder()
        .reload_policy(ReloadPolicy::OnCommitWithDelay)
        .try_into()
        .map_err(map_tantivy_err)?;
    let searcher = reader.searcher();

    let parser = QueryParser::for_index(&index, vec![fields.subject, fields.detail]);
    // Use lenient parsing so that special characters like parentheses,
    // brackets, etc. don't cause hard parse failures. Tantivy's strict
    // parser treats () as grouping operators which breaks queries like
    // "sqrt(36)". Lenient mode drops unparseable fragments and returns
    // partial results.
    let (parsed_query, _parse_errors) = parser.parse_query_lenient(query);

    let combined_query =
        combine_query_with_fact_filter(parsed_query, field_filter, fields.fact_type);
    let top_docs = searcher
        .search(combined_query.as_ref(), &TopDocs::with_limit(limit.max(1)))
        .map_err(map_tantivy_err)?;

    let mut snippet_generator =
        SnippetGenerator::create(&searcher, combined_query.as_ref(), fields.detail)
            .map_err(map_tantivy_err)?;
    snippet_generator.set_max_num_chars(snippet_chars(context));

    let mut out = Vec::with_capacity(top_docs.len());
    for (score, address) in top_docs {
        let retrieved: TantivyDocument = searcher.doc(address).map_err(map_tantivy_err)?;
        let subject = doc_text(&retrieved, fields.subject);
        let detail = doc_text(&retrieved, fields.detail);
        let snippet = detail
            .as_deref()
            .map(|value| snippet_generator.snippet(value).to_html())
            .filter(|value| !value.is_empty())
            .or_else(|| detail.map(|value| truncate_chars(&value, snippet_chars(context))))
            .or_else(|| subject.clone())
            .unwrap_or_default();

        out.push(SearchResult {
            session_id: doc_text(&retrieved, fields.session_id).unwrap_or_default(),
            engine: doc_text(&retrieved, fields.engine).unwrap_or_default(),
            turn: doc_u64(&retrieved, fields.turn).unwrap_or(0),
            fact_type: doc_text(&retrieved, fields.fact_type).unwrap_or_default(),
            subject: subject.unwrap_or_default(),
            snippet,
            ts: doc_text(&retrieved, fields.ts).unwrap_or_default(),
            score,
            session_headline: doc_text(&retrieved, fields.session_headline).unwrap_or_default(),
        });
    }

    Ok(out)
}

fn combine_query_with_fact_filter(
    base_query: Box<dyn Query>,
    field_filter: SearchField,
    fact_type_field: Field,
) -> Box<dyn Query> {
    let Some(types) = field_filter.fact_types() else {
        return base_query;
    };

    let mut filter_clauses: Vec<(Occur, Box<dyn Query>)> = Vec::with_capacity(types.len());
    for value in types {
        let term = Term::from_field_text(fact_type_field, value);
        filter_clauses.push((
            Occur::Should,
            Box::new(TermQuery::new(term, IndexRecordOption::Basic)),
        ));
    }

    let fact_type_query: Box<dyn Query> = if filter_clauses.len() == 1 {
        filter_clauses
            .into_iter()
            .next()
            .map(|(_, query)| query)
            .unwrap_or_else(|| Box::new(BooleanQuery::new(Vec::new())))
    } else {
        Box::new(BooleanQuery::new(filter_clauses))
    };

    Box::new(BooleanQuery::new(vec![
        (Occur::Must, base_query),
        (Occur::Must, fact_type_query),
    ]))
}

fn print_search_human(results: &[SearchResult]) {
    if results.is_empty() {
        println!("No results.");
        return;
    }

    let headers = [
        "Score", "Session", "Engine", "Turn", "Type", "Time", "Snippet",
    ];
    let rows: Vec<Vec<String>> = results
        .iter()
        .map(|row| {
            vec![
                format!("{:.2}", row.score),
                row.session_id.chars().take(8).collect::<String>(),
                row.engine.clone(),
                row.turn.to_string(),
                row.fact_type.clone(),
                format_timestamp(&row.ts),
                truncate_chars(&row.snippet, 120),
            ]
        })
        .collect();
    print_table(&headers, &rows);
}

fn parse_since_bound(value: &str) -> Result<DateTime<Utc>, GaalError> {
    if let Some(bound) = parse_relative_duration(value) {
        return Ok(bound);
    }

    if let Ok(parsed) = DateTime::parse_from_rfc3339(value) {
        return Ok(parsed.with_timezone(&Utc));
    }

    if let Ok(parsed) = NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        let Some(naive) = parsed.and_hms_opt(0, 0, 0) else {
            return Err(GaalError::ParseError(format!(
                "invalid since value: {value}"
            )));
        };
        return Ok(DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc));
    }

    Err(GaalError::ParseError(format!(
        "invalid since value `{value}`; expected 30d/12h, RFC3339, or YYYY-MM-DD"
    )))
}

fn parse_relative_duration(value: &str) -> Option<DateTime<Utc>> {
    let unit = value.chars().last()?;
    let number = value.get(..value.len().saturating_sub(1))?;
    let amount = number.parse::<i64>().ok()?;
    if amount < 0 {
        return None;
    }
    let now = Utc::now();
    let delta = match unit {
        's' => chrono::TimeDelta::try_seconds(amount)?,
        'h' => chrono::TimeDelta::try_hours(amount)?,
        'd' => chrono::TimeDelta::try_days(amount)?,
        'w' => chrono::TimeDelta::try_weeks(amount)?,
        _ => return None,
    };
    now.checked_sub_signed(delta)
}

fn fact_is_after(ts: &str, bound: &DateTime<Utc>) -> bool {
    DateTime::parse_from_rfc3339(ts)
        .map(|parsed| parsed.with_timezone(&Utc) >= *bound)
        .unwrap_or(false)
}

fn session_ids_for_cwd(conn: &Connection, cwd: &str) -> Result<HashSet<String>, GaalError> {
    let pattern = format!("%{cwd}%");
    let mut stmt = conn
        .prepare(
            r#"
            SELECT id
            FROM sessions
            WHERE cwd LIKE :pattern
            "#,
        )
        .map_err(GaalError::from)?;
    let mut rows = stmt
        .query(named_params! { ":pattern": pattern.as_str() })
        .map_err(GaalError::from)?;

    let mut out = HashSet::new();
    while let Some(row) = rows.next().map_err(GaalError::from)? {
        let session_id: String = row.get(0).map_err(GaalError::from)?;
        out.insert(session_id);
    }
    Ok(out)
}

fn tantivy_path() -> PathBuf {
    gaal_home().join("tantivy")
}

fn open_existing_index() -> Result<Index, GaalError> {
    let path = tantivy_path();
    if !path.exists() || !path.join("meta.json").exists() {
        return Err(GaalError::NoIndex);
    }
    Index::open_in_dir(&path).map_err(map_tantivy_err)
}

fn open_or_create_index() -> Result<Index, GaalError> {
    let path = tantivy_path();
    fs::create_dir_all(&path)?;

    if path.join("meta.json").exists() {
        let index = Index::open_in_dir(&path).map_err(map_tantivy_err)?;
        if resolve_fields(&index.schema()).is_ok() {
            return Ok(index);
        }
        drop(index);
        fs::remove_dir_all(&path)?;
        fs::create_dir_all(&path)?;
    }

    Index::create_in_dir(&path, build_schema()).map_err(map_tantivy_err)
}

fn build_schema() -> Schema {
    let mut builder = SchemaBuilder::new();
    builder.add_text_field("session_id", STRING | STORED);
    builder.add_text_field("engine", STRING | STORED);
    builder.add_u64_field("turn", INDEXED | STORED);
    builder.add_text_field("fact_type", STRING | STORED);
    builder.add_text_field("subject", TEXT | STORED);
    builder.add_text_field("detail", TEXT | STORED);
    builder.add_text_field("ts", STRING | STORED);
    builder.add_text_field("session_headline", TEXT | STORED);
    builder.build()
}

fn resolve_fields(schema: &Schema) -> Result<SearchIndexFields, GaalError> {
    Ok(SearchIndexFields {
        session_id: schema
            .get_field("session_id")
            .map_err(|_| GaalError::Internal("missing tantivy field: session_id".to_string()))?,
        engine: schema
            .get_field("engine")
            .map_err(|_| GaalError::Internal("missing tantivy field: engine".to_string()))?,
        turn: schema
            .get_field("turn")
            .map_err(|_| GaalError::Internal("missing tantivy field: turn".to_string()))?,
        fact_type: schema
            .get_field("fact_type")
            .map_err(|_| GaalError::Internal("missing tantivy field: fact_type".to_string()))?,
        subject: schema
            .get_field("subject")
            .map_err(|_| GaalError::Internal("missing tantivy field: subject".to_string()))?,
        detail: schema
            .get_field("detail")
            .map_err(|_| GaalError::Internal("missing tantivy field: detail".to_string()))?,
        ts: schema
            .get_field("ts")
            .map_err(|_| GaalError::Internal("missing tantivy field: ts".to_string()))?,
        session_headline: schema.get_field("session_headline").map_err(|_| {
            GaalError::Internal("missing tantivy field: session_headline".to_string())
        })?,
    })
}

fn doc_text(doc: &TantivyDocument, field: Field) -> Option<String> {
    doc.get_first(field)
        .and_then(|value| value.as_str())
        .map(ToOwned::to_owned)
}

fn doc_u64(doc: &TantivyDocument, field: Field) -> Option<u64> {
    doc.get_first(field).and_then(|value| value.as_u64())
}

fn map_tantivy_err(err: tantivy::TantivyError) -> GaalError {
    GaalError::Other(anyhow!(err))
}

fn turn_to_u64(value: Option<i64>) -> u64 {
    value.and_then(|turn| u64::try_from(turn).ok()).unwrap_or(0)
}

fn snippet_chars(context: usize) -> usize {
    120 + context.saturating_mul(80)
}

fn truncate_chars(input: &str, limit: usize) -> String {
    if limit == 0 {
        return String::new();
    }
    let mut out = input.chars().take(limit).collect::<String>();
    if input.chars().count() > limit {
        out.push_str("...");
    }
    out
}
