use std::collections::{HashMap, HashSet};

use rusqlite::{named_params, Connection, OptionalExtension, Row};
use serde::Serialize;
use serde_json::{Map as JsonMap, Number, Value};

use crate::error::GaalError;
use crate::model::{Fact, HandoffRecord};

/// Database-level session row, flattened to only SQLite-backed fields.
#[derive(Debug, Clone)]
pub struct SessionRow {
    pub id: String,
    pub engine: String,
    pub model: Option<String>,
    pub cwd: Option<String>,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub exit_signal: Option<String>,
    pub last_event_at: Option<String>,
    pub parent_id: Option<String>,
    pub session_type: String,
    pub jsonl_path: String,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_creation_tokens: i64,
    pub reasoning_tokens: i64,
    pub total_tools: i64,
    pub total_turns: i64,
    pub peak_context: i64,
    pub last_indexed_offset: i64,
}

/// Filters for listing sessions.
#[derive(Debug, Clone, Default)]
pub struct ListFilter {
    pub engine: Option<String>,
    pub session_type: Option<String>,
    pub since: Option<String>,
    pub before: Option<String>,
    pub cwd: Option<String>,
    pub tag: Option<String>,
    pub sort_by: Option<String>,
    pub limit: Option<i64>,
    pub include_subagents: bool,
}

/// Supported fact types for `who` queries and fact filtering.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum FactType {
    FileRead,
    FileWrite,
    Command,
    Error,
    GitOp,
    UserPrompt,
    AssistantReply,
    TaskSpawn,
}

impl FactType {
    fn as_str(&self) -> &'static str {
        match self {
            Self::FileRead => "file_read",
            Self::FileWrite => "file_write",
            Self::Command => "command",
            Self::Error => "error",
            Self::GitOp => "git_op",
            Self::UserPrompt => "user_prompt",
            Self::AssistantReply => "assistant_reply",
            Self::TaskSpawn => "task_spawn",
        }
    }
}

impl std::fmt::Display for FactType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Filters for `query_who`.
#[derive(Debug, Clone, Default)]
pub struct WhoFilter {
    pub fact_types: Vec<FactType>,
    pub subject_pattern: Option<String>,
    pub since: Option<String>,
    pub before: Option<String>,
    pub cwd: Option<String>,
    pub engine: Option<String>,
    pub tag: Option<String>,
    pub failed_only: bool,
    pub limit: Option<i64>,
}

/// A single match row for `query_who`.
#[derive(Debug, Clone)]
pub struct WhoResult {
    pub session_id: String,
    pub engine: String,
    pub ts: String,
    pub fact_type: String,
    pub subject: Option<String>,
    pub detail: Option<String>,
    pub session_headline: Option<String>,
    pub session_type: String,
    pub parent_id: Option<String>,
}

/// Database index status snapshot.
#[derive(Debug, Clone, Default)]
pub struct IndexStatus {
    pub db_size_bytes: u64,
    pub sessions_total: i64,
    pub sessions_by_engine: HashMap<String, i64>,
    pub facts_total: i64,
    pub handoffs_total: i64,
    pub last_indexed_at: Option<String>,
    pub oldest_session: Option<String>,
    pub newest_session: Option<String>,
}

/// Aggregate counters for a filtered session set.
#[derive(Debug, Clone, Default)]
pub struct AggregateResult {
    pub sessions: i64,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub estimated_cost_usd: f64,
    pub by_engine: HashMap<String, i64>,
}

/// Insert or update a session row by primary key.
pub fn upsert_session(conn: &Connection, session: &SessionRow) -> Result<(), GaalError> {
    conn.execute(
        r#"
        INSERT INTO sessions (
            id, engine, model, cwd, started_at, ended_at, exit_signal, last_event_at,
            parent_id, session_type, jsonl_path, total_input_tokens, total_output_tokens,
            cache_read_tokens, cache_creation_tokens, reasoning_tokens,
            total_tools, total_turns, peak_context, last_indexed_offset
        )
        VALUES (
            :id, :engine, :model, :cwd, :started_at, :ended_at, :exit_signal, :last_event_at,
            :parent_id, :session_type, :jsonl_path, :total_input_tokens, :total_output_tokens,
            :cache_read_tokens, :cache_creation_tokens, :reasoning_tokens,
            :total_tools, :total_turns, :peak_context, :last_indexed_offset
        )
        ON CONFLICT(id) DO UPDATE SET
            engine = excluded.engine,
            model = excluded.model,
            cwd = excluded.cwd,
            started_at = excluded.started_at,
            ended_at = excluded.ended_at,
            exit_signal = excluded.exit_signal,
            last_event_at = excluded.last_event_at,
            parent_id = excluded.parent_id,
            session_type = excluded.session_type,
            jsonl_path = excluded.jsonl_path,
            total_input_tokens = excluded.total_input_tokens,
            total_output_tokens = excluded.total_output_tokens,
            cache_read_tokens = excluded.cache_read_tokens,
            cache_creation_tokens = excluded.cache_creation_tokens,
            reasoning_tokens = excluded.reasoning_tokens,
            total_tools = excluded.total_tools,
            total_turns = excluded.total_turns,
            peak_context = excluded.peak_context,
            last_indexed_offset = excluded.last_indexed_offset
        "#,
        named_params! {
            ":id": &session.id,
            ":engine": &session.engine,
            ":model": &session.model,
            ":cwd": &session.cwd,
            ":started_at": &session.started_at,
            ":ended_at": &session.ended_at,
            ":exit_signal": &session.exit_signal,
            ":last_event_at": &session.last_event_at,
            ":parent_id": &session.parent_id,
            ":session_type": &session.session_type,
            ":jsonl_path": &session.jsonl_path,
            ":total_input_tokens": session.total_input_tokens,
            ":total_output_tokens": session.total_output_tokens,
            ":cache_read_tokens": session.cache_read_tokens,
            ":cache_creation_tokens": session.cache_creation_tokens,
            ":reasoning_tokens": session.reasoning_tokens,
            ":total_tools": session.total_tools,
            ":total_turns": session.total_turns,
            ":peak_context": session.peak_context,
            ":last_indexed_offset": session.last_indexed_offset,
        },
    )
    .map_err(db_err)?;
    Ok(())
}

/// Insert one fact and return the inserted row id.
pub fn insert_fact(conn: &Connection, fact: &Fact) -> Result<i64, GaalError> {
    let payload = fact_payload(fact)?;
    conn.execute(
        r#"
        INSERT INTO facts (
            session_id, ts, turn_number, fact_type, subject, detail, exit_code, success
        )
        VALUES (
            :session_id, :ts, :turn_number, :fact_type, :subject, :detail, :exit_code, :success
        )
        "#,
        named_params! {
            ":session_id": &payload.session_id,
            ":ts": &payload.ts,
            ":turn_number": &payload.turn_number,
            ":fact_type": &payload.fact_type,
            ":subject": &payload.subject,
            ":detail": &payload.detail,
            ":exit_code": &payload.exit_code,
            ":success": &payload.success,
        },
    )
    .map_err(db_err)?;
    Ok(conn.last_insert_rowid())
}

/// Insert facts using the provided connection (which may already be inside a transaction).
/// Callers are responsible for transaction management.
pub fn insert_facts_batch(conn: &Connection, facts: &[Fact]) -> Result<(), GaalError> {
    let mut stmt = conn
        .prepare(
            r#"
            INSERT INTO facts (
                session_id, ts, turn_number, fact_type, subject, detail, exit_code, success
            )
            VALUES (
                :session_id, :ts, :turn_number, :fact_type, :subject, :detail, :exit_code, :success
            )
            "#,
        )
        .map_err(db_err)?;

    for fact in facts {
        let payload = fact_payload(fact)?;
        stmt.execute(named_params! {
            ":session_id": &payload.session_id,
            ":ts": &payload.ts,
            ":turn_number": &payload.turn_number,
            ":fact_type": &payload.fact_type,
            ":subject": &payload.subject,
            ":detail": &payload.detail,
            ":exit_code": &payload.exit_code,
            ":success": &payload.success,
        })
        .map_err(db_err)?;
    }
    Ok(())
}

/// Insert or update a handoff row by session id.
pub fn upsert_handoff(conn: &Connection, handoff: &HandoffRecord) -> Result<(), GaalError> {
    let map = to_json_object(handoff)?;
    let session_id = required_json_string(&map, "session_id")?;
    let headline = optional_json_string(&map, "headline");
    let projects = optional_json_text(&map, "projects")?;
    let keywords = optional_json_text(&map, "keywords")?;
    let substance = optional_json_i64(&map, "substance").unwrap_or(0);
    let duration_minutes = optional_json_i64(&map, "duration_minutes").unwrap_or(0);
    let generated_at = optional_json_string(&map, "generated_at");
    let generated_by = optional_json_string(&map, "generated_by");
    let content_path = optional_json_string(&map, "content_path");

    conn.execute(
        r#"
        INSERT INTO handoffs (
            session_id, headline, projects, keywords, substance, duration_minutes,
            generated_at, generated_by, content_path
        )
        VALUES (
            :session_id, :headline, :projects, :keywords, :substance, :duration_minutes,
            :generated_at, :generated_by, :content_path
        )
        ON CONFLICT(session_id) DO UPDATE SET
            headline = excluded.headline,
            projects = excluded.projects,
            keywords = excluded.keywords,
            substance = excluded.substance,
            duration_minutes = excluded.duration_minutes,
            generated_at = excluded.generated_at,
            generated_by = excluded.generated_by,
            content_path = excluded.content_path
        "#,
        named_params! {
            ":session_id": &session_id,
            ":headline": &headline,
            ":projects": &projects,
            ":keywords": &keywords,
            ":substance": substance,
            ":duration_minutes": duration_minutes,
            ":generated_at": &generated_at,
            ":generated_by": &generated_by,
            ":content_path": &content_path,
        },
    )
    .map_err(db_err)?;
    Ok(())
}

/// Add a tag to a session.
pub fn add_tag(conn: &Connection, session_id: &str, tag: &str) -> Result<(), GaalError> {
    conn.execute(
        "INSERT OR IGNORE INTO session_tags (session_id, tag) VALUES (:session_id, :tag)",
        named_params! {
            ":session_id": session_id,
            ":tag": tag,
        },
    )
    .map_err(db_err)?;
    Ok(())
}

/// Remove a tag from a session.
pub fn remove_tag(conn: &Connection, session_id: &str, tag: &str) -> Result<(), GaalError> {
    conn.execute(
        "DELETE FROM session_tags WHERE session_id = :session_id AND tag = :tag",
        named_params! {
            ":session_id": session_id,
            ":tag": tag,
        },
    )
    .map_err(db_err)?;
    Ok(())
}

/// Delete a session and its associated facts by id.
pub fn delete_session(conn: &Connection, id: &str) -> Result<(), GaalError> {
    conn.execute(
        "DELETE FROM facts WHERE session_id = :id",
        named_params! { ":id": id },
    )
    .map_err(db_err)?;
    conn.execute(
        "DELETE FROM sessions WHERE id = :id",
        named_params! { ":id": id },
    )
    .map_err(db_err)?;
    Ok(())
}

/// Fetch one session by id.
pub fn get_session(conn: &Connection, id: &str) -> Result<Option<SessionRow>, GaalError> {
    conn.query_row(
        r#"
        SELECT
            id, engine, model, cwd, started_at, ended_at, exit_signal, last_event_at,
            parent_id, session_type, jsonl_path, total_input_tokens, total_output_tokens,
            cache_read_tokens, cache_creation_tokens, reasoning_tokens,
            total_tools, total_turns, peak_context, last_indexed_offset
        FROM sessions
        WHERE id = :id
        "#,
        named_params! { ":id": id },
        row_to_session,
    )
    .optional()
    .map_err(db_err)
}

/// List sessions with DB-level filtering.
pub fn list_sessions(conn: &Connection, filter: &ListFilter) -> Result<Vec<SessionRow>, GaalError> {
    let cwd_like = filter.cwd.as_ref().map(|value| format!("%{value}%"));
    let limit = filter.limit.unwrap_or(50).max(1);

    let sort_key = filter
        .sort_by
        .as_deref()
        .unwrap_or("started")
        .to_ascii_lowercase();
    let order_by = match sort_key.as_str() {
        "ended" => "s.ended_at DESC",
        "tokens" => "(s.total_input_tokens + s.total_output_tokens) DESC",
        "cost" => "(s.total_input_tokens + s.total_output_tokens) DESC",
        "duration" => "(strftime('%s', COALESCE(s.ended_at, CURRENT_TIMESTAMP)) - strftime('%s', s.started_at)) DESC",
        _ => "s.started_at DESC",
    };

    let sql = format!(
        r#"
        SELECT
            s.id, s.engine, s.model, s.cwd, s.started_at, s.ended_at, s.exit_signal, s.last_event_at,
            s.parent_id, s.session_type, s.jsonl_path, s.total_input_tokens, s.total_output_tokens,
            s.cache_read_tokens, s.cache_creation_tokens, s.reasoning_tokens,
            s.total_tools, s.total_turns, s.peak_context, s.last_indexed_offset
        FROM sessions s
        WHERE (:engine IS NULL OR s.engine = :engine)
          AND (:session_type IS NULL OR s.session_type = :session_type)
          AND (:since IS NULL OR s.started_at >= :since)
          AND (:before IS NULL OR s.started_at <= :before)
          AND (:cwd_like IS NULL OR s.cwd LIKE :cwd_like)
          AND (
              :tag IS NULL OR EXISTS (
                  SELECT 1 FROM session_tags t
                  WHERE t.session_id = s.id AND t.tag = :tag
              )
          )
          AND (:include_subagents = 1 OR s.session_type != 'subagent')
        ORDER BY {order_by}
        LIMIT :limit
        "#
    );

    let mut stmt = conn.prepare(&sql).map_err(db_err)?;
    let mut rows = stmt
        .query(named_params! {
            ":engine": filter.engine.as_deref(),
            ":session_type": filter.session_type.as_deref(),
            ":since": filter.since.as_deref(),
            ":before": filter.before.as_deref(),
            ":cwd_like": cwd_like.as_deref(),
            ":tag": filter.tag.as_deref(),
            ":include_subagents": filter.include_subagents as i64,
            ":limit": limit,
        })
        .map_err(db_err)?;

    let mut out = Vec::new();
    while let Some(row) = rows.next().map_err(db_err)? {
        out.push(row_to_session(row).map_err(db_err)?);
    }

    Ok(out)
}

/// Count sessions matching the filter (ignoring LIMIT) for "Showing N of M" messages.
pub fn count_sessions(conn: &Connection, filter: &ListFilter) -> Result<i64, GaalError> {
    let cwd_like = filter.cwd.as_ref().map(|value| format!("%{value}%"));

    let sql = r#"
        SELECT COUNT(*)
        FROM sessions s
        WHERE (:engine IS NULL OR s.engine = :engine)
          AND (:session_type IS NULL OR s.session_type = :session_type)
          AND (:since IS NULL OR s.started_at >= :since)
          AND (:before IS NULL OR s.started_at <= :before)
          AND (:cwd_like IS NULL OR s.cwd LIKE :cwd_like)
          AND (
              :tag IS NULL OR EXISTS (
                  SELECT 1 FROM session_tags t
                  WHERE t.session_id = s.id AND t.tag = :tag
              )
          )
          AND (:include_subagents = 1 OR s.session_type != 'subagent')
    "#;

    let count: i64 = conn
        .query_row(
            sql,
            named_params! {
                ":engine": filter.engine.as_deref(),
                ":session_type": filter.session_type.as_deref(),
                ":since": filter.since.as_deref(),
                ":before": filter.before.as_deref(),
                ":cwd_like": cwd_like.as_deref(),
                ":tag": filter.tag.as_deref(),
                ":include_subagents": filter.include_subagents as i64,
            },
            |row| row.get(0),
        )
        .map_err(db_err)?;

    Ok(count)
}

/// Get facts for a session, optionally filtered by fact type.
pub fn get_facts(
    conn: &Connection,
    session_id: &str,
    fact_type_filter: Option<FactType>,
) -> Result<Vec<Fact>, GaalError> {
    let fact_type = fact_type_filter.as_ref().map(FactType::as_str);

    let mut stmt = conn
        .prepare(
            r#"
            SELECT
                id, session_id, ts, turn_number, fact_type, subject, detail, exit_code, success
            FROM facts
            WHERE session_id = :session_id
              AND (:fact_type IS NULL OR fact_type = :fact_type)
            ORDER BY ts ASC, id ASC
            "#,
        )
        .map_err(db_err)?;

    let mut rows = stmt
        .query(named_params! {
            ":session_id": session_id,
            ":fact_type": fact_type,
        })
        .map_err(db_err)?;

    let mut out = Vec::new();
    while let Some(row) = rows.next().map_err(db_err)? {
        let id: i64 = row.get("id").map_err(db_err)?;
        let session_id_val: String = row.get("session_id").map_err(db_err)?;
        let ts: String = row.get("ts").map_err(db_err)?;
        let turn_number: Option<i64> = row.get("turn_number").map_err(db_err)?;
        let fact_type_val: String = row.get("fact_type").map_err(db_err)?;
        let subject: Option<String> = row.get("subject").map_err(db_err)?;
        let detail: Option<String> = row.get("detail").map_err(db_err)?;
        let exit_code: Option<i64> = row.get("exit_code").map_err(db_err)?;
        let success: Option<i64> = row.get("success").map_err(db_err)?;

        let mut base = JsonMap::new();
        base.insert("id".to_string(), Value::Number(Number::from(id)));
        base.insert("session_id".to_string(), Value::String(session_id_val));
        base.insert("ts".to_string(), Value::String(ts));
        insert_opt_i64(&mut base, "turn_number", turn_number);
        base.insert("fact_type".to_string(), Value::String(fact_type_val));
        insert_opt_string(&mut base, "subject", subject);
        insert_opt_string(&mut base, "detail", detail);
        insert_opt_i64(&mut base, "exit_code", exit_code);
        insert_opt_i64(&mut base, "success", success);

        let primary = Value::Object(base.clone());
        match serde_json::from_value::<Fact>(primary) {
            Ok(fact) => out.push(fact),
            Err(primary_err) => {
                if let Some(success_value) = success {
                    let mut fallback = base;
                    fallback.insert("success".to_string(), Value::Bool(success_value != 0));
                    let fallback_value = Value::Object(fallback);
                    match serde_json::from_value::<Fact>(fallback_value) {
                        Ok(fact) => out.push(fact),
                        Err(fallback_err) => {
                            return Err(GaalError::Internal(format!(
                                "failed to deserialize fact row: {primary_err}; fallback: {fallback_err}"
                            )));
                        }
                    }
                } else {
                    return Err(GaalError::Internal(format!(
                        "failed to deserialize fact row: {primary_err}"
                    )));
                }
            }
        }
    }

    Ok(out)
}

/// Get the handoff record for a session.
pub fn get_handoff(
    conn: &Connection,
    session_id: &str,
) -> Result<Option<HandoffRecord>, GaalError> {
    let row = conn
        .query_row(
            r#"
            SELECT
                session_id, headline, projects, keywords, substance, duration_minutes,
                generated_at, generated_by, content_path
            FROM handoffs
            WHERE session_id = :session_id
            "#,
            named_params! { ":session_id": session_id },
            |row| {
                Ok((
                    row.get::<_, String>("session_id")?,
                    row.get::<_, Option<String>>("headline")?,
                    row.get::<_, Option<String>>("projects")?,
                    row.get::<_, Option<String>>("keywords")?,
                    row.get::<_, i64>("substance")?,
                    row.get::<_, i64>("duration_minutes")?,
                    row.get::<_, Option<String>>("generated_at")?,
                    row.get::<_, Option<String>>("generated_by")?,
                    row.get::<_, Option<String>>("content_path")?,
                ))
            },
        )
        .optional()
        .map_err(db_err)?;

    let Some((
        session_id_val,
        headline,
        projects_text,
        keywords_text,
        substance,
        duration_minutes,
        generated_at,
        generated_by,
        content_path,
    )) = row
    else {
        return Ok(None);
    };

    let mut map = JsonMap::new();
    map.insert("session_id".to_string(), Value::String(session_id_val));
    insert_opt_string(&mut map, "headline", headline);
    map.insert("projects".to_string(), parse_embedded_json(projects_text));
    map.insert("keywords".to_string(), parse_embedded_json(keywords_text));
    map.insert(
        "substance".to_string(),
        Value::Number(Number::from(substance)),
    );
    map.insert(
        "duration_minutes".to_string(),
        Value::Number(Number::from(duration_minutes)),
    );
    insert_opt_string(&mut map, "generated_at", generated_at);
    insert_opt_string(&mut map, "generated_by", generated_by);
    insert_opt_string(&mut map, "content_path", content_path);

    let handoff = serde_json::from_value::<HandoffRecord>(Value::Object(map))
        .map_err(|e| GaalError::Internal(format!("failed to deserialize handoff row: {e}")))?;
    Ok(Some(handoff))
}

/// Get all tags for a session.
pub fn get_tags(conn: &Connection, session_id: &str) -> Result<Vec<String>, GaalError> {
    let mut stmt = conn
        .prepare("SELECT tag FROM session_tags WHERE session_id = :session_id ORDER BY tag ASC")
        .map_err(db_err)?;
    let mut rows = stmt
        .query(named_params! { ":session_id": session_id })
        .map_err(db_err)?;

    let mut out = Vec::new();
    while let Some(row) = rows.next().map_err(db_err)? {
        out.push(row.get::<_, String>(0).map_err(db_err)?);
    }
    Ok(out)
}

/// Run an inverted "who did what" query over facts joined with sessions/handoffs.
pub fn query_who(conn: &Connection, filter: &WhoFilter) -> Result<Vec<WhoResult>, GaalError> {
    let subject_pattern = filter
        .subject_pattern
        .as_ref()
        .map(|pattern| format!("%{pattern}%"));
    let cwd_like = filter.cwd.as_ref().map(|value| format!("%{value}%"));
    let limit = filter.limit.unwrap_or(10).max(1);
    let failed_only = if filter.failed_only { 1_i64 } else { 0_i64 };

    // Build a fact_type IN (...) clause from the enum-derived strings.
    // Values come from FactType::as_str() (hardcoded &'static str), so
    // embedding them directly is safe from injection.
    let fact_type_clause = if filter.fact_types.is_empty() {
        "1 = 1".to_string()
    } else {
        let quoted: Vec<String> = filter
            .fact_types
            .iter()
            .map(|ft| format!("'{}'", ft.as_str()))
            .collect();
        format!("f.fact_type IN ({})", quoted.join(", "))
    };

    let sql = format!(
        r#"
            SELECT
                f.session_id,
                s.engine,
                f.ts,
                f.fact_type,
                f.subject,
                f.detail,
                h.headline,
                COALESCE(s.session_type, 'standalone') AS session_type,
                s.parent_id
            FROM facts f
            INNER JOIN sessions s ON s.id = f.session_id
            LEFT JOIN handoffs h ON h.session_id = f.session_id
            WHERE (:since IS NULL OR f.ts >= :since)
              AND (:before IS NULL OR f.ts <= :before)
              AND (:engine IS NULL OR s.engine = :engine)
              AND (:cwd_like IS NULL OR s.cwd LIKE :cwd_like)
              AND (
                    :subject_pattern IS NULL
                    OR COALESCE(f.subject, '') LIKE :subject_pattern
                    OR COALESCE(f.detail, '') LIKE :subject_pattern
                  )
              AND (
                    :tag IS NULL OR EXISTS (
                        SELECT 1
                        FROM session_tags t
                        WHERE t.session_id = s.id AND t.tag = :tag
                    )
                  )
              AND (
                    :failed_only = 0
                    OR (f.exit_code IS NOT NULL AND f.exit_code != 0)
                    OR f.success = 0
                  )
              AND {fact_type_clause}
            ORDER BY f.ts DESC, f.id DESC
            LIMIT :limit
            "#,
    );

    let mut stmt = conn.prepare(&sql).map_err(db_err)?;

    let mut rows = stmt
        .query(named_params! {
            ":since": filter.since.as_deref(),
            ":before": filter.before.as_deref(),
            ":engine": filter.engine.as_deref(),
            ":cwd_like": cwd_like.as_deref(),
            ":subject_pattern": subject_pattern.as_deref(),
            ":tag": filter.tag.as_deref(),
            ":failed_only": failed_only,
            ":limit": limit,
        })
        .map_err(db_err)?;

    // Safety-net post-filter: keeps the allowed_types check in case the SQL
    // clause and the caller's intent ever drift apart.
    let allowed_types: Option<HashSet<String>> = if filter.fact_types.is_empty() {
        None
    } else {
        Some(
            filter
                .fact_types
                .iter()
                .map(|fact_type| fact_type.as_str().to_string())
                .collect(),
        )
    };

    let mut out = Vec::new();
    while let Some(row) = rows.next().map_err(db_err)? {
        let result = WhoResult {
            session_id: row.get("session_id").map_err(db_err)?,
            engine: row.get("engine").map_err(db_err)?,
            ts: row.get("ts").map_err(db_err)?,
            fact_type: row.get("fact_type").map_err(db_err)?,
            subject: row.get("subject").map_err(db_err)?,
            detail: row.get("detail").map_err(db_err)?,
            session_headline: row.get("headline").map_err(db_err)?,
            session_type: row
                .get::<_, Option<String>>("session_type")
                .map_err(db_err)?
                .unwrap_or_else(|| "standalone".to_string()),
            parent_id: row.get("parent_id").map_err(db_err)?,
        };

        if let Some(types) = &allowed_types {
            if !types.contains(&result.fact_type) {
                continue;
            }
        }

        out.push(result);
        if out.len() as i64 >= limit {
            break;
        }
    }

    Ok(out)
}

/// Return high-level index status and row counts.
pub fn get_index_status(conn: &Connection) -> Result<IndexStatus, GaalError> {
    let db_size_bytes = match std::fs::metadata(crate::db::db_path()) {
        Ok(metadata) => metadata.len(),
        Err(_) => 0,
    };

    let sessions_total: i64 = conn
        .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
        .map_err(db_err)?;
    let facts_total: i64 = conn
        .query_row("SELECT COUNT(*) FROM facts", [], |row| row.get(0))
        .map_err(db_err)?;
    let handoffs_total: i64 = conn
        .query_row("SELECT COUNT(*) FROM handoffs", [], |row| row.get(0))
        .map_err(db_err)?;

    let mut sessions_by_engine = HashMap::new();
    {
        let mut stmt = conn
            .prepare("SELECT engine, COUNT(*) as count FROM sessions GROUP BY engine")
            .map_err(db_err)?;
        let mut rows = stmt.query([]).map_err(db_err)?;
        while let Some(row) = rows.next().map_err(db_err)? {
            let engine: String = row.get(0).map_err(db_err)?;
            let count: i64 = row.get(1).map_err(db_err)?;
            sessions_by_engine.insert(engine, count);
        }
    }

    let last_indexed_at: Option<String> = conn
        .query_row("SELECT MAX(last_event_at) FROM sessions", [], |row| {
            row.get(0)
        })
        .map_err(db_err)?;
    let oldest_session: Option<String> = conn
        .query_row("SELECT MIN(started_at) FROM sessions", [], |row| row.get(0))
        .map_err(db_err)?;
    let newest_session: Option<String> = conn
        .query_row("SELECT MAX(started_at) FROM sessions", [], |row| row.get(0))
        .map_err(db_err)?;

    Ok(IndexStatus {
        db_size_bytes,
        sessions_total,
        sessions_by_engine,
        facts_total,
        handoffs_total,
        last_indexed_at,
        oldest_session,
        newest_session,
    })
}

/// Aggregate token/cost and bucket counts over a filtered session set.
pub fn get_aggregate(conn: &Connection, filter: &ListFilter) -> Result<AggregateResult, GaalError> {
    let mut aggregate_filter = filter.clone();
    // Override limit to fetch all matching sessions — list_sessions defaults
    // None to 50, which silently truncated aggregate results.
    aggregate_filter.limit = Some(i64::MAX);
    let sessions = list_sessions(conn, &aggregate_filter)?;

    let mut by_engine: HashMap<String, i64> = HashMap::new();
    let mut total_input_tokens = 0_i64;
    let mut total_output_tokens = 0_i64;
    let mut total_cost = 0.0_f64;

    for session in &sessions {
        total_input_tokens += session.total_input_tokens;
        total_output_tokens += session.total_output_tokens;
        total_cost += estimate_session_cost(session);
        *by_engine.entry(session.engine.clone()).or_insert(0) += 1;
    }

    Ok(AggregateResult {
        sessions: sessions.len() as i64,
        total_input_tokens,
        total_output_tokens,
        estimated_cost_usd: (total_cost * 100.0).round() / 100.0,
        by_engine,
    })
}

fn row_to_session(row: &Row<'_>) -> rusqlite::Result<SessionRow> {
    Ok(SessionRow {
        id: row.get("id")?,
        engine: row.get("engine")?,
        model: row.get("model")?,
        cwd: row.get("cwd")?,
        started_at: row.get("started_at")?,
        ended_at: row.get("ended_at")?,
        exit_signal: row.get("exit_signal")?,
        last_event_at: row.get("last_event_at")?,
        parent_id: row.get("parent_id")?,
        session_type: row
            .get::<_, Option<String>>("session_type")?
            .unwrap_or_else(|| "standalone".to_string()),
        jsonl_path: row.get("jsonl_path")?,
        total_input_tokens: row.get("total_input_tokens")?,
        total_output_tokens: row.get("total_output_tokens")?,
        cache_read_tokens: row.get::<_, Option<i64>>("cache_read_tokens")?.unwrap_or(0),
        cache_creation_tokens: row
            .get::<_, Option<i64>>("cache_creation_tokens")?
            .unwrap_or(0),
        reasoning_tokens: row.get::<_, Option<i64>>("reasoning_tokens")?.unwrap_or(0),
        total_tools: row.get("total_tools")?,
        total_turns: row.get("total_turns")?,
        peak_context: row.get::<_, Option<i64>>("peak_context")?.unwrap_or(0),
        last_indexed_offset: row.get("last_indexed_offset")?,
    })
}

/// Model-aware cost rates: (input, output, cache_read, cache_write) per MTok.
fn cost_rates(model: &str) -> (f64, f64, f64, f64) {
    let m = model.to_lowercase();
    // gpt-5.4-mini must be checked before gpt-5.4
    if m.contains("gpt-5.4-mini") {
        (0.15, 0.60, 0.015, 0.0)
    } else if m.contains("gpt-5.4") {
        (2.50, 10.0, 0.25, 0.0)
    } else if m.contains("opus") {
        (15.0, 75.0, 1.5, 18.75)
    } else if m.contains("haiku") {
        (0.25, 1.25, 0.025, 0.3)
    } else if m.contains("sonnet") {
        (3.0, 15.0, 0.3, 3.75)
    } else {
        (3.0, 15.0, 0.3, 3.75) // default sonnet
    }
}

/// Estimate cost for a single session using model-aware rates.
/// Reasoning tokens are charged at the output rate.
pub fn estimate_session_cost(row: &SessionRow) -> f64 {
    let model = row.model.as_deref().unwrap_or("sonnet");
    let (rate_in, rate_out, rate_cr, rate_cw) = cost_rates(model);
    let mtok = 1_000_000.0_f64;
    let cost = (row.total_input_tokens as f64 / mtok) * rate_in
        + (row.total_output_tokens as f64 / mtok) * rate_out
        + (row.cache_read_tokens as f64 / mtok) * rate_cr
        + (row.cache_creation_tokens as f64 / mtok) * rate_cw
        + (row.reasoning_tokens as f64 / mtok) * rate_out;
    (cost * 100.0).round() / 100.0
}

struct FactPayload {
    session_id: String,
    ts: String,
    turn_number: Option<i64>,
    fact_type: String,
    subject: Option<String>,
    detail: Option<String>,
    exit_code: Option<i64>,
    success: Option<i64>,
}

fn fact_payload(fact: &Fact) -> Result<FactPayload, GaalError> {
    let map = to_json_object(fact)?;
    Ok(FactPayload {
        session_id: required_json_string(&map, "session_id")?,
        ts: required_json_string(&map, "ts")?,
        turn_number: optional_json_i64(&map, "turn_number"),
        fact_type: required_json_string(&map, "fact_type")?,
        subject: optional_json_string(&map, "subject"),
        detail: optional_json_string(&map, "detail"),
        exit_code: optional_json_i64(&map, "exit_code"),
        success: optional_json_i64_or_bool(&map, "success"),
    })
}

fn to_json_object<T: Serialize>(value: &T) -> Result<JsonMap<String, Value>, GaalError> {
    let serialized = serde_json::to_value(value)
        .map_err(|e| GaalError::Internal(format!("serialization error: {e}")))?;
    match serialized {
        Value::Object(map) => Ok(map),
        _ => Err(GaalError::Internal(
            "expected serialized value to be a JSON object".to_string(),
        )),
    }
}

fn required_json_string(map: &JsonMap<String, Value>, key: &str) -> Result<String, GaalError> {
    match map.get(key).and_then(Value::as_str) {
        Some(value) => Ok(value.to_string()),
        None => Err(GaalError::Internal(format!(
            "missing required string field `{key}`"
        ))),
    }
}

fn optional_json_string(map: &JsonMap<String, Value>, key: &str) -> Option<String> {
    let value = map.get(key)?;
    if value.is_null() {
        return None;
    }

    match value.as_str() {
        Some(text) => Some(text.to_string()),
        None => Some(value.to_string()),
    }
}

fn optional_json_i64(map: &JsonMap<String, Value>, key: &str) -> Option<i64> {
    let value = map.get(key)?;
    if value.is_null() {
        return None;
    }

    if let Some(int_value) = value.as_i64() {
        return Some(int_value);
    }

    value.as_str().and_then(|text| text.parse::<i64>().ok())
}

fn optional_json_i64_or_bool(map: &JsonMap<String, Value>, key: &str) -> Option<i64> {
    let value = map.get(key)?;
    if value.is_null() {
        return None;
    }

    if let Some(bool_value) = value.as_bool() {
        return Some(if bool_value { 1 } else { 0 });
    }

    optional_json_i64(map, key)
}

fn optional_json_text(
    map: &JsonMap<String, Value>,
    key: &str,
) -> Result<Option<String>, GaalError> {
    let Some(value) = map.get(key) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    serde_json::to_string(value)
        .map(Some)
        .map_err(|e| GaalError::Internal(format!("failed to serialize `{key}`: {e}")))
}

fn insert_opt_string(map: &mut JsonMap<String, Value>, key: &str, value: Option<String>) {
    match value {
        Some(text) => {
            map.insert(key.to_string(), Value::String(text));
        }
        None => {
            map.insert(key.to_string(), Value::Null);
        }
    }
}

fn insert_opt_i64(map: &mut JsonMap<String, Value>, key: &str, value: Option<i64>) {
    match value {
        Some(int_value) => {
            map.insert(key.to_string(), Value::Number(Number::from(int_value)));
        }
        None => {
            map.insert(key.to_string(), Value::Null);
        }
    }
}

fn parse_embedded_json(value: Option<String>) -> Value {
    let Some(raw) = value else {
        return Value::Null;
    };

    match serde_json::from_str::<Value>(&raw) {
        Ok(parsed) => parsed,
        Err(_) => Value::String(raw),
    }
}

fn db_err(err: rusqlite::Error) -> GaalError {
    GaalError::Db(err)
}
