use std::path::PathBuf;
use std::time::Duration;

use rusqlite::Connection;

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
        PRAGMA busy_timeout = 5000;
        "#,
    )
    .map_err(map_db_err)?;

    conn.busy_timeout(Duration::from_millis(5_000))
        .map_err(map_db_err)?;

    // Run migrations BEFORE the full schema batch so that new columns exist
    // when CREATE INDEX IF NOT EXISTS references them.  ALTER TABLE ADD COLUMN
    // is a no-op error when the column already exists, so .ok() swallows that.
    conn.execute_batch(
        "ALTER TABLE sessions ADD COLUMN session_type TEXT DEFAULT 'standalone';",
    )
    .ok();

    conn.execute_batch(DB_SCHEMA).map_err(map_db_err)?;
    Ok(())
}

/// Return the default SQLite index path (`~/.gaal/index.db`).
pub fn db_path() -> PathBuf {
    gaal_home().join("index.db")
}

/// Open the default SQLite database, creating directories and schema as needed.
pub fn open_db() -> Result<Connection, GaalError> {
    let home = gaal_home();
    std::fs::create_dir_all(&home).map_err(GaalError::Io)?;

    let conn = Connection::open(db_path()).map_err(map_db_err)?;
    init_db(&conn)?;
    Ok(conn)
}

fn map_db_err(err: rusqlite::Error) -> GaalError {
    GaalError::Db(err)
}
