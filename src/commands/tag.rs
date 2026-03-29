use rusqlite::named_params;
use serde::Serialize;

use crate::commands::inspect::find_latest_session_id;
use crate::db::open_db;
use crate::db::queries::{add_tag, get_session, remove_tag};
use crate::error::GaalError;
use crate::output::json::print_json;

/// Arguments for `gaal tag`.
#[derive(Debug, Clone)]
pub struct TagArgs {
    /// Session id or id prefix. Use `ls` to list distinct tags.
    pub id: String,
    /// Tags to add/remove.
    pub tags: Vec<String>,
    /// Remove mode. When false, tags are added.
    pub remove: bool,
}

#[derive(Debug, Serialize)]
struct TagResult {
    session_id: String,
    action: String,
    tags: Vec<String>,
}

/// Runs the `gaal tag` command.
pub fn run(args: TagArgs) -> Result<(), GaalError> {
    let conn = open_db()?;
    if args.id == "ls" {
        if args.remove || !args.tags.is_empty() {
            return Err(GaalError::ParseError(
                "`gaal tag ls` does not accept tags or --remove".to_string(),
            ));
        }
        let tags = list_tags(&conn)?;
        return print_json(&tags).map_err(GaalError::from);
    }

    let session_id = resolve_session_id(&conn, &args.id)?;
    if args.tags.is_empty() {
        return Err(GaalError::ParseError(
            "at least one tag is required".to_string(),
        ));
    }

    if args.remove {
        for tag in &args.tags {
            remove_tag(&conn, &session_id, tag)?;
        }
    } else {
        for tag in &args.tags {
            add_tag(&conn, &session_id, tag)?;
        }
    }

    let action = if args.remove { "removed" } else { "added" };
    let payload = TagResult {
        session_id,
        action: action.to_string(),
        tags: args.tags,
    };
    print_json(&payload).map_err(GaalError::from)
}

fn list_tags(conn: &rusqlite::Connection) -> Result<Vec<String>, GaalError> {
    let mut stmt = conn
        .prepare("SELECT DISTINCT tag FROM session_tags ORDER BY tag ASC")
        .map_err(GaalError::from)?;
    let mut rows = stmt.query([]).map_err(GaalError::from)?;

    let mut tags = Vec::new();
    while let Some(row) = rows.next().map_err(GaalError::from)? {
        tags.push(row.get::<_, String>(0).map_err(GaalError::from)?);
    }
    Ok(tags)
}

fn resolve_session_id(
    conn: &rusqlite::Connection,
    id_or_prefix: &str,
) -> Result<String, GaalError> {
    if id_or_prefix == "latest" {
        return find_latest_session_id(conn);
    }

    if let Some(session) = get_session(conn, id_or_prefix)? {
        return Ok(session.id);
    }

    let mut stmt = conn
        .prepare(
            r#"
            SELECT id
            FROM sessions
            WHERE id LIKE :prefix
            ORDER BY started_at DESC
            "#,
        )
        .map_err(GaalError::from)?;
    let pattern = format!("{id_or_prefix}%");
    let mut rows = stmt
        .query(named_params! { ":prefix": pattern })
        .map_err(GaalError::from)?;

    let mut ids = Vec::new();
    while let Some(row) = rows.next().map_err(GaalError::from)? {
        ids.push(row.get::<_, String>(0).map_err(GaalError::from)?);
    }

    if ids.is_empty() {
        return Err(GaalError::NotFound(id_or_prefix.to_string()));
    }
    if ids.len() > 1 {
        return Err(GaalError::AmbiguousId(format!(
            "{id_or_prefix} ({})",
            ids.join(", ")
        )));
    }
    Ok(ids.remove(0))
}
