use std::path::PathBuf;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};

use crate::config::gaal_home;
use crate::error::GaalError;

/// Embedded SQLite schema DDL.
pub const DB_SCHEMA: &str = include_str!("schema.sql");

/// Initialize the SQLite database schema and runtime pragmas.
pub fn init_db(conn: &Connection) -> Result<(), GaalError> {
    conn.execute_batch(
        r#"
        PRAGMA journal_mode = WAL;
        PRAGMA foreign_keys = ON;
        PRAGMA busy_timeout = 30000;
        "#,
    )
    .map_err(map_db_err)?;

    conn.busy_timeout(Duration::from_millis(30_000))
        .map_err(map_db_err)?;

    migrate_sessions_engine_check(conn)?;

    // Gate the migration behind a column-existence check so we don't attempt
    // ALTER TABLE (which requires a write lock) on every startup.
    let has_session_type: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name='session_type'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map(|count| count > 0)
        .unwrap_or(false);

    if !has_session_type {
        conn.execute_batch(
            "ALTER TABLE sessions ADD COLUMN session_type TEXT DEFAULT 'standalone';",
        )
        .ok();
    }

    let has_peak_context: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name='peak_context'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map(|count| count > 0)
        .unwrap_or(false);

    if !has_peak_context {
        conn.execute_batch("ALTER TABLE sessions ADD COLUMN peak_context INTEGER DEFAULT 0;")
            .ok();
    }

    // Token accounting columns: cache_read_tokens, cache_creation_tokens, reasoning_tokens.
    for col in [
        "cache_read_tokens",
        "cache_creation_tokens",
        "reasoning_tokens",
    ] {
        let has_col: bool = conn
            .query_row(
                &format!("SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name='{col}'"),
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|count| count > 0)
            .unwrap_or(false);
        if !has_col {
            conn.execute_batch(&format!(
                "ALTER TABLE sessions ADD COLUMN {col} INTEGER DEFAULT 0;"
            ))
            .ok();
        }
    }

    // subagent_type column for tracking Agent tool_use subagent_type field.
    let has_subagent_type: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name='subagent_type'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map(|count| count > 0)
        .unwrap_or(false);
    if !has_subagent_type {
        conn.execute_batch("ALTER TABLE sessions ADD COLUMN subagent_type TEXT;")
            .ok();
    }

    let has_gemini_summary: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name='gemini_summary'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map(|count| count > 0)
        .unwrap_or(false);
    if !has_gemini_summary {
        conn.execute_batch("ALTER TABLE sessions ADD COLUMN gemini_summary TEXT;")
            .ok();
    }

    conn.execute_batch(DB_SCHEMA).map_err(map_db_err)?;
    Ok(())
}

fn migrate_sessions_engine_check(conn: &Connection) -> Result<(), GaalError> {
    if !table_exists(conn, "sessions")? {
        return Ok(());
    }

    let gemini_probe_inserted = conn
        .execute(
            "INSERT INTO sessions (id, engine, started_at, jsonl_path) VALUES ('__gaal_gemini_probe__', 'gemini', '1970-01-01T00:00:00Z', '__probe__')",
            [],
        )
        .is_ok();

    if gemini_probe_inserted {
        conn.execute(
            "DELETE FROM sessions WHERE id = '__gaal_gemini_probe__'",
            [],
        )
        .ok();
        return Ok(());
    }

    let existing_columns = session_columns(conn)?;
    let copy_columns: Vec<&str> = SESSION_COLUMNS
        .iter()
        .copied()
        .filter(|column| existing_columns.iter().any(|existing| existing == column))
        .collect();
    let column_list = copy_columns.join(", ");

    conn.execute_batch("PRAGMA foreign_keys = OFF;")
        .map_err(map_db_err)?;

    let migration_result = (|| {
        conn.execute_batch(SESSIONS_TABLE_WITH_GEMINI)
            .map_err(map_db_err)?;

        if !column_list.is_empty() {
            conn.execute(
                &format!(
                    "INSERT INTO sessions_new ({column_list}) SELECT {column_list} FROM sessions"
                ),
                [],
            )
            .map_err(map_db_err)?;
        }

        conn.execute_batch(
            r#"
            DROP TABLE sessions;
            ALTER TABLE sessions_new RENAME TO sessions;
            "#,
        )
        .map_err(map_db_err)?;
        Ok(())
    })();

    let reenable_fk_result = conn.execute_batch("PRAGMA foreign_keys = ON;");
    if let Err(err) = reenable_fk_result {
        return Err(map_db_err(err));
    }

    migration_result
}

fn table_exists(conn: &Connection, table_name: &str) -> Result<bool, GaalError> {
    conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
        [table_name],
        |row| row.get::<_, i64>(0),
    )
    .map(|count| count > 0)
    .map_err(map_db_err)
}

fn session_columns(conn: &Connection) -> Result<Vec<String>, GaalError> {
    let mut stmt = conn
        .prepare("SELECT name FROM pragma_table_info('sessions') ORDER BY cid")
        .map_err(map_db_err)?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(map_db_err)?;

    let mut columns = Vec::new();
    for row in rows {
        columns.push(row.map_err(map_db_err)?);
    }
    Ok(columns)
}

const SESSION_COLUMNS: &[&str] = &[
    "id",
    "engine",
    "model",
    "cwd",
    "started_at",
    "ended_at",
    "exit_signal",
    "last_event_at",
    "parent_id",
    "session_type",
    "jsonl_path",
    "total_input_tokens",
    "total_output_tokens",
    "cache_read_tokens",
    "cache_creation_tokens",
    "reasoning_tokens",
    "total_tools",
    "total_turns",
    "peak_context",
    "last_indexed_offset",
    "subagent_type",
];

const SESSIONS_TABLE_WITH_GEMINI: &str = r#"
CREATE TABLE sessions_new (
    id TEXT PRIMARY KEY,
    engine TEXT NOT NULL CHECK(engine IN ('claude', 'codex', 'gemini')),
    model TEXT,
    cwd TEXT,
    started_at TEXT NOT NULL,
    ended_at TEXT,
    exit_signal TEXT,
    last_event_at TEXT,
    parent_id TEXT REFERENCES sessions(id),
    session_type TEXT DEFAULT 'standalone' CHECK(session_type IN ('standalone', 'coordinator', 'subagent')),
    jsonl_path TEXT NOT NULL,
    total_input_tokens INTEGER DEFAULT 0,
    total_output_tokens INTEGER DEFAULT 0,
    cache_read_tokens INTEGER DEFAULT 0,
    cache_creation_tokens INTEGER DEFAULT 0,
    reasoning_tokens INTEGER DEFAULT 0,
    total_tools INTEGER DEFAULT 0,
    total_turns INTEGER DEFAULT 0,
    peak_context INTEGER DEFAULT 0,
    last_indexed_offset INTEGER DEFAULT 0,
    subagent_type TEXT
);
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_sessions_check_constraint_to_allow_gemini() {
        let conn = Connection::open_in_memory().expect("open db");
        conn.execute_batch(
            r#"
            CREATE TABLE sessions (
                id TEXT PRIMARY KEY,
                engine TEXT NOT NULL CHECK(engine IN ('claude', 'codex')),
                model TEXT,
                cwd TEXT,
                started_at TEXT NOT NULL,
                ended_at TEXT,
                exit_signal TEXT,
                last_event_at TEXT,
                parent_id TEXT REFERENCES sessions(id),
                jsonl_path TEXT NOT NULL
            );
            CREATE TABLE facts (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL REFERENCES sessions(id),
                ts TEXT NOT NULL,
                turn_number INTEGER,
                fact_type TEXT NOT NULL,
                subject TEXT,
                detail TEXT,
                exit_code INTEGER,
                success INTEGER
            );
            INSERT INTO sessions (id, engine, started_at, jsonl_path) VALUES ('sess-1', 'claude', '2026-01-01T00:00:00Z', '/tmp/sess-1');
            INSERT INTO facts (session_id, ts, fact_type) VALUES ('sess-1', '2026-01-01T00:00:00Z', 'user_prompt');
            "#,
        )
        .expect("seed old schema");

        init_db(&conn).expect("migrate schema");

        conn.execute(
            "INSERT INTO sessions (id, engine, started_at, jsonl_path) VALUES ('sess-2', 'gemini', '2026-01-02T00:00:00Z', '/tmp/sess-2')",
            [],
        )
        .expect("insert gemini session");

        let fact_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM facts WHERE session_id = 'sess-1'",
                [],
                |row| row.get(0),
            )
            .expect("count facts");
        assert_eq!(fact_count, 1);
    }
}

/// Return the default SQLite index path (`~/.gaal/index.db`).
pub fn db_path() -> PathBuf {
    gaal_home().join("index.db")
}

/// Open the default SQLite database, creating directories and schema as needed.
///
/// This is for **write commands** (index, handoff, tag).  It runs DDL
/// migrations and uses a 30-second busy timeout.
pub fn open_db() -> Result<Connection, GaalError> {
    let home = gaal_home();
    std::fs::create_dir_all(&home).map_err(GaalError::Io)?;

    let path = db_path();
    let conn = Connection::open(&path).map_err(|e| {
        let mut msg = format!("Failed to open database at {}: {e}", path.display());
        if e.to_string().contains("unable to open database file") {
            msg.push_str(". Ensure ~/.gaal/index.db is accessible. In sandboxed environments, add --allow-read ~/.gaal/ to your sandbox flags.");
        }
        GaalError::Internal(msg)
    })?;
    init_db(&conn)?;
    // Guard: ensure no phantom transaction survives init_db (can happen when
    // schema is already migrated and ALTER TABLE is swallowed by .ok()).
    if !conn.is_autocommit() {
        conn.execute_batch("ROLLBACK;").ok();
    }
    Ok(conn)
}

/// Open the default SQLite database in **read-only** mode.
///
/// No DDL is executed (no CREATE TABLE, ALTER TABLE, CREATE INDEX).
/// Uses a 5-second busy timeout which is sufficient for read queries
/// even under concurrent write load with WAL mode.
pub fn open_db_readonly() -> Result<Connection, GaalError> {
    let path = db_path();
    if !path.exists() {
        // DB hasn't been created yet; fall back to the write path which
        // will create the schema.
        return open_db();
    }

    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = Connection::open_with_flags(&path, flags).map_err(|e| {
        let mut msg = format!("Failed to open database at {}: {e}", path.display());
        if e.to_string().contains("unable to open database file") {
            msg.push_str(". Ensure ~/.gaal/index.db is accessible. In sandboxed environments, add --allow-read ~/.gaal/ to your sandbox flags.");
        }
        GaalError::Internal(msg)
    })?;

    conn.execute_batch(
        r#"
        PRAGMA journal_mode = WAL;
        PRAGMA busy_timeout = 5000;
        "#,
    )
    .map_err(map_db_err)?;

    conn.busy_timeout(Duration::from_millis(5_000))
        .map_err(map_db_err)?;

    Ok(conn)
}

fn map_db_err(err: rusqlite::Error) -> GaalError {
    GaalError::Db(err)
}
