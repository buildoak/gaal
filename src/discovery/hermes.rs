use std::fs;
use std::path::PathBuf;
use std::time::SystemTime;

use anyhow::Result;
use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{Connection, OpenFlags};

use crate::discovery::discover::DiscoveredSession;
use crate::parser::types::Engine;

/// Discover Hermes Agent sessions from `~/.hermes/state.db`.
///
/// Hermes stores many logical sessions in one SQLite database, so every
/// discovered session points to the same source path and carries the Hermes
/// session id as the canonical Gaal id.
pub fn discover_hermes_sessions(newer_than: Option<SystemTime>) -> Result<Vec<DiscoveredSession>> {
    let state_db = hermes_state_db_path();
    discover_hermes_sessions_from_path(state_db, newer_than)
}

fn discover_hermes_sessions_from_path(
    state_db: PathBuf,
    newer_than: Option<SystemTime>,
) -> Result<Vec<DiscoveredSession>> {
    if !state_db.exists() {
        return Ok(Vec::new());
    }

    let meta = fs::metadata(&state_db)?;
    if !meta.is_file() || meta.len() == 0 {
        return Ok(Vec::new());
    }
    if let Some(cutoff) = newer_than {
        if meta.modified().ok().is_some_and(|mtime| mtime < cutoff) {
            return Ok(Vec::new());
        }
    }

    let conn = open_readonly(&state_db)?;
    let mut stmt = conn.prepare(
        r#"
        SELECT id, model, started_at, parent_session_id
        FROM sessions
        ORDER BY
            CASE WHEN parent_session_id IS NULL THEN 0 ELSE 1 END,
            started_at DESC,
            id ASC
        "#,
    )?;
    let rows = stmt.query_map([], |row| {
        let started_at: f64 = row.get(2)?;
        Ok(DiscoveredSession {
            id: row.get::<_, String>(0)?,
            engine: Engine::Hermes,
            path: state_db.clone(),
            model: row.get::<_, Option<String>>(1)?,
            cwd: None,
            started_at: Some(unix_real_to_rfc3339(started_at)),
            forked_from_id: row.get::<_, Option<String>>(3)?,
            file_size: meta.len(),
        })
    })?;

    let mut sessions = Vec::new();
    for row in rows {
        sessions.push(row?);
    }
    Ok(sessions)
}

pub(crate) fn hermes_state_db_path() -> PathBuf {
    if let Ok(raw) = std::env::var("HERMES_STATE_DB") {
        if !raw.trim().is_empty() {
            return PathBuf::from(raw);
        }
    }
    if let Ok(raw) = std::env::var("HERMES_HOME") {
        if !raw.trim().is_empty() {
            return PathBuf::from(raw).join("state.db");
        }
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".hermes")
        .join("state.db")
}

pub(crate) fn open_readonly(path: &std::path::Path) -> Result<Connection> {
    Ok(Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?)
}

pub(crate) fn unix_real_to_rfc3339(raw: f64) -> String {
    if !raw.is_finite() {
        return "1970-01-01T00:00:00Z".to_string();
    }
    let secs = raw.trunc() as i64;
    let nanos = ((raw.fract().abs()) * 1_000_000_000.0).round() as u32;
    DateTime::<Utc>::from_timestamp(secs, nanos.min(999_999_999))
        .unwrap_or(DateTime::<Utc>::UNIX_EPOCH)
        .to_rfc3339_opts(SecondsFormat::Millis, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_db() -> PathBuf {
        let db_path = std::env::temp_dir().join(format!(
            "gaal-hermes-discovery-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time")
                .as_nanos()
        ));
        let conn = Connection::open(&db_path).expect("open fixture db");
        conn.execute_batch(include_str!("../../tests/fixtures/hermes/state.sql"))
            .expect("load fixture sql");
        db_path
    }

    #[test]
    fn discovers_logical_sessions_from_state_db() {
        let db_path = fixture_db();
        let sessions =
            discover_hermes_sessions_from_path(db_path.clone(), None).expect("discover sessions");
        fs::remove_file(&db_path).ok();

        assert_eq!(sessions.len(), 5);
        assert!(sessions
            .iter()
            .all(|session| session.engine == Engine::Hermes));
        assert!(sessions.iter().all(|session| session.path == db_path));
        assert!(sessions
            .iter()
            .any(|session| session.id == "20260504_141414_childbb"
                && session.forked_from_id.as_deref() == Some("20260504_131313_parentaa")));
    }
}
