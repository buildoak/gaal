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
        conn.execute_batch(
            "ALTER TABLE sessions ADD COLUMN peak_context INTEGER DEFAULT 0;",
        )
        .ok();
    }

    // Token accounting columns: cache_read_tokens, cache_creation_tokens, reasoning_tokens.
    for col in ["cache_read_tokens", "cache_creation_tokens", "reasoning_tokens"] {
        let has_col: bool = conn
            .query_row(
                &format!(
                    "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name='{col}'"
                ),
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

    conn.execute_batch(DB_SCHEMA).map_err(map_db_err)?;
    Ok(())
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
