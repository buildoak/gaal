use std::collections::{HashMap, HashSet};

use gaal::{
    db::{
        init_db,
        queries::{
            add_tag, get_facts, get_handoff, get_session, get_tags, insert_facts_batch,
            list_sessions, query_who, upsert_handoff, upsert_session, FactType as QueryFactType,
            ListFilter, SessionRow, WhoFilter,
        },
        DB_SCHEMA,
    },
    error::GaalError,
    model::{Fact, FactType, HandoffRecord},
};
use rusqlite::Connection;

fn test_conn() -> Connection {
    let conn = Connection::open_in_memory().expect("open in-memory sqlite");
    init_db(&conn).expect("initialize schema");
    assert!(DB_SCHEMA.contains("CREATE TABLE IF NOT EXISTS sessions"));
    conn
}

fn session_row(
    id: &str,
    engine: &str,
    cwd: Option<&str>,
    started_at: &str,
    ended_at: Option<&str>,
    exit_signal: Option<&str>,
) -> SessionRow {
    SessionRow {
        id: id.to_string(),
        engine: engine.to_string(),
        model: Some("gpt-5".to_string()),
        cwd: cwd.map(str::to_string),
        started_at: started_at.to_string(),
        ended_at: ended_at.map(str::to_string),
        exit_signal: exit_signal.map(str::to_string),
        last_event_at: ended_at
            .map(str::to_string)
            .or_else(|| Some(started_at.to_string())),
        parent_id: None,
        jsonl_path: format!("/tmp/{id}.jsonl"),
        total_input_tokens: 120,
        total_output_tokens: 40,
        total_tools: 5,
        total_turns: 7,
        last_indexed_offset: 256,
    }
}

fn fact(
    session_id: &str,
    ts: &str,
    turn_number: i32,
    fact_type: FactType,
    subject: Option<&str>,
    detail: Option<&str>,
) -> Fact {
    Fact {
        id: None,
        session_id: session_id.to_string(),
        ts: ts.to_string(),
        turn_number: Some(turn_number),
        fact_type,
        subject: subject.map(str::to_string),
        detail: detail.map(str::to_string),
        exit_code: None,
        success: Some(true),
    }
}

fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(|token| token.to_ascii_lowercase())
        .collect()
}

fn tokenize_fields(values: &[String]) -> HashSet<String> {
    values
        .iter()
        .flat_map(|value| tokenize(value))
        .collect::<HashSet<_>>()
}

fn score_handoffs_by_query(handoffs: &[HandoffRecord], query: &str) -> HashMap<String, f64> {
    let query_tokens = tokenize(query);
    let n_total = handoffs.len() as f64;

    let token_sets: Vec<(String, HashSet<String>, HashSet<String>)> = handoffs
        .iter()
        .map(|handoff| {
            (
                handoff.session_id.clone(),
                tokenize_fields(&handoff.projects),
                tokenize_fields(&handoff.keywords),
            )
        })
        .collect();

    let mut idf: HashMap<String, f64> = HashMap::new();
    for token in &query_tokens {
        let mut df = 0.0_f64;
        for (_session_id, projects, keywords) in &token_sets {
            if projects.contains(token) || keywords.contains(token) {
                df += 1.0;
            }
        }

        let value = if df > 0.0 {
            (n_total / df).ln().max(0.0)
        } else {
            0.0
        };
        idf.insert(token.clone(), value);
    }

    let mut out = HashMap::new();
    for (session_id, projects, keywords) in token_sets {
        let mut score = 0.0_f64;
        for token in &query_tokens {
            let token_idf = *idf.get(token).unwrap_or(&0.0);
            if projects.contains(token) {
                score += 3.0 * token_idf;
            }
            if keywords.contains(token) {
                score += 2.0 * token_idf;
            }
        }
        out.insert(session_id, score);
    }

    out
}

#[test]
fn session_insert_and_query_roundtrip() {
    let conn = test_conn();
    let parent = session_row(
        "parent-123",
        "claude",
        Some("/tmp/test-project"),
        "2026-03-01T09:00:00Z",
        Some("2026-03-01T09:30:00Z"),
        None,
    );
    upsert_session(&conn, &parent).expect("upsert parent session");

    let session = SessionRow {
        id: "sess-roundtrip-1".to_string(),
        engine: "claude".to_string(),
        model: Some("claude-3-7-sonnet".to_string()),
        cwd: Some("/tmp/test-project".to_string()),
        started_at: "2026-03-01T10:00:00Z".to_string(),
        ended_at: Some("2026-03-01T10:30:00Z".to_string()),
        exit_signal: Some("ok".to_string()),
        last_event_at: Some("2026-03-01T10:29:10Z".to_string()),
        parent_id: Some(parent.id.clone()),
        jsonl_path: "/tmp/sess-roundtrip-1.jsonl".to_string(),
        total_input_tokens: 2_000,
        total_output_tokens: 900,
        total_tools: 13,
        total_turns: 21,
        last_indexed_offset: 4_096,
    };

    upsert_session(&conn, &session).expect("upsert session");
    let loaded = get_session(&conn, &session.id)
        .expect("query session")
        .expect("session should exist");

    assert_eq!(loaded.id, session.id);
    assert_eq!(loaded.engine, session.engine);
    assert_eq!(loaded.model, session.model);
    assert_eq!(loaded.cwd, session.cwd);
    assert_eq!(loaded.started_at, session.started_at);
    assert_eq!(loaded.ended_at, session.ended_at);
    assert_eq!(loaded.exit_signal, session.exit_signal);
    assert_eq!(loaded.last_event_at, session.last_event_at);
    assert_eq!(loaded.parent_id, session.parent_id);
    assert_eq!(loaded.jsonl_path, session.jsonl_path);
    assert_eq!(loaded.total_input_tokens, session.total_input_tokens);
    assert_eq!(loaded.total_output_tokens, session.total_output_tokens);
    assert_eq!(loaded.total_tools, session.total_tools);
    assert_eq!(loaded.total_turns, session.total_turns);
    assert_eq!(loaded.last_indexed_offset, session.last_indexed_offset);
}

#[test]
fn fact_insert_and_query_roundtrip() {
    let conn = test_conn();
    let session = session_row(
        "sess-facts-1",
        "codex",
        Some("/repo/gaal"),
        "2026-03-01T11:00:00Z",
        Some("2026-03-01T11:15:00Z"),
        None,
    );
    upsert_session(&conn, &session).expect("upsert session");

    let facts = vec![
        fact(
            &session.id,
            "2026-03-01T11:00:01Z",
            1,
            FactType::FileRead,
            Some("src/main.rs"),
            None,
        ),
        fact(
            &session.id,
            "2026-03-01T11:00:05Z",
            1,
            FactType::FileWrite,
            Some("src/lib.rs"),
            Some("applied patch"),
        ),
        fact(
            &session.id,
            "2026-03-01T11:00:09Z",
            2,
            FactType::Command,
            Some("cargo test"),
            Some("cargo test --quiet"),
        ),
        Fact {
            id: None,
            session_id: session.id.clone(),
            ts: "2026-03-01T11:00:15Z".to_string(),
            turn_number: Some(2),
            fact_type: FactType::Error,
            subject: Some("cargo test".to_string()),
            detail: Some("test failed".to_string()),
            exit_code: Some(101),
            success: Some(false),
        },
        fact(
            &session.id,
            "2026-03-01T11:00:20Z",
            3,
            FactType::GitOp,
            Some("commit"),
            Some("fix tests"),
        ),
    ];

    insert_facts_batch(&conn, &facts).expect("insert facts batch");
    let loaded = get_facts(&conn, &session.id, None).expect("query facts");

    assert_eq!(loaded.len(), facts.len());

    let expected_types: Vec<&str> = facts.iter().map(|f| f.fact_type.as_str()).collect();
    let actual_types: Vec<&str> = loaded.iter().map(|f| f.fact_type.as_str()).collect();
    assert_eq!(actual_types, expected_types);
}

#[test]
fn ls_filters_work() {
    let conn = test_conn();

    let claude_completed = session_row(
        "s-claude-complete",
        "claude",
        Some("/tmp/test-project"),
        "2026-03-01T09:00:00Z",
        Some("2026-03-01T09:40:00Z"),
        None,
    );
    let codex_completed = session_row(
        "s-codex-complete",
        "codex",
        Some("/tmp/test-project"),
        "2026-03-02T09:00:00Z",
        Some("2026-03-02T09:25:00Z"),
        None,
    );
    let claude_active = session_row(
        "s-claude-active",
        "claude",
        Some("/tmp/other-project"),
        "2026-03-03T09:00:00Z",
        None,
        None,
    );

    upsert_session(&conn, &claude_completed).expect("insert claude completed");
    upsert_session(&conn, &codex_completed).expect("insert codex completed");
    upsert_session(&conn, &claude_active).expect("insert claude active");

    let claude_only = list_sessions(
        &conn,
        &ListFilter {
            engine: Some("claude".to_string()),
            limit: Some(20),
            ..Default::default()
        },
    )
    .expect("list by engine");
    assert_eq!(claude_only.len(), 2);
    assert!(claude_only.iter().all(|s| s.engine == "claude"));

    let day_two_only = list_sessions(
        &conn,
        &ListFilter {
            since: Some("2026-03-02T00:00:00Z".to_string()),
            before: Some("2026-03-02T23:59:59Z".to_string()),
            limit: Some(20),
            ..Default::default()
        },
    )
    .expect("list by date range");
    assert_eq!(day_two_only.len(), 1);
    assert_eq!(day_two_only[0].id, codex_completed.id);

    let cwd_filtered = list_sessions(
        &conn,
        &ListFilter {
            cwd: Some("test-project".to_string()),
            limit: Some(20),
            ..Default::default()
        },
    )
    .expect("list by cwd");
    let cwd_ids: HashSet<String> = cwd_filtered.into_iter().map(|s| s.id).collect();
    assert!(cwd_ids.contains(&claude_completed.id));
    assert!(cwd_ids.contains(&codex_completed.id));
    assert!(!cwd_ids.contains(&claude_active.id));

    let completed = list_sessions(
        &conn,
        &ListFilter {
            status: Some(vec!["completed".to_string()]),
            limit: Some(20),
            ..Default::default()
        },
    )
    .expect("list completed");
    let completed_ids: HashSet<String> = completed.into_iter().map(|s| s.id).collect();
    assert!(completed_ids.contains(&claude_completed.id));
    assert!(completed_ids.contains(&codex_completed.id));
    assert!(!completed_ids.contains(&claude_active.id));

    let unknown = list_sessions(
        &conn,
        &ListFilter {
            status: Some(vec!["unknown".to_string()]),
            limit: Some(20),
            ..Default::default()
        },
    )
    .expect("list unknown");
    assert_eq!(unknown.len(), 1);
    assert_eq!(unknown[0].id, claude_active.id);
}

#[test]
fn who_inverted_queries_return_correct_results() {
    let conn = test_conn();
    let session = session_row(
        "who-sess-1",
        "claude",
        Some("/repo/gaal"),
        "2026-03-01T12:00:00Z",
        Some("2026-03-01T12:10:00Z"),
        None,
    );
    upsert_session(&conn, &session).expect("upsert session");

    let facts = vec![
        fact(
            &session.id,
            "2026-03-01T12:00:01Z",
            1,
            FactType::FileRead,
            Some("src/main.rs"),
            Some("read source"),
        ),
        fact(
            &session.id,
            "2026-03-01T12:00:02Z",
            1,
            FactType::FileWrite,
            Some("src/lib.rs"),
            Some("write lib"),
        ),
        fact(
            &session.id,
            "2026-03-01T12:00:03Z",
            2,
            FactType::Command,
            None,
            Some("cargo test"),
        ),
        fact(
            &session.id,
            "2026-03-01T12:00:04Z",
            2,
            FactType::GitOp,
            Some("status"),
            Some("git status"),
        ),
    ];

    insert_facts_batch(&conn, &facts).expect("insert facts");

    let file_read_rows = query_who(
        &conn,
        &WhoFilter {
            fact_types: vec![QueryFactType::FileRead],
            limit: Some(10),
            ..Default::default()
        },
    )
    .expect("query_who file_read");
    assert_eq!(file_read_rows.len(), 1);
    assert_eq!(file_read_rows[0].fact_type, "file_read");
    assert_eq!(file_read_rows[0].subject.as_deref(), Some("src/main.rs"));

    let file_write_rows = query_who(
        &conn,
        &WhoFilter {
            fact_types: vec![QueryFactType::FileWrite],
            limit: Some(10),
            ..Default::default()
        },
    )
    .expect("query_who file_write");
    assert_eq!(file_write_rows.len(), 1);
    assert_eq!(file_write_rows[0].fact_type, "file_write");
    assert_eq!(file_write_rows[0].subject.as_deref(), Some("src/lib.rs"));

    let command_rows = query_who(
        &conn,
        &WhoFilter {
            fact_types: vec![QueryFactType::Command],
            limit: Some(10),
            ..Default::default()
        },
    )
    .expect("query_who command");
    assert_eq!(command_rows.len(), 1);
    assert_eq!(command_rows[0].fact_type, "command");
    assert_eq!(command_rows[0].detail.as_deref(), Some("cargo test"));
}

#[test]
fn recall_scoring_produces_expected_ordering() {
    let conn = test_conn();

    let session_gaal = session_row(
        "recall-gaal",
        "claude",
        Some("/repo/gaal"),
        "2026-02-25T10:00:00Z",
        Some("2026-02-25T10:40:00Z"),
        None,
    );
    let session_orac = session_row(
        "recall-orac",
        "codex",
        Some("/repo/orac"),
        "2026-02-26T11:00:00Z",
        Some("2026-02-26T11:35:00Z"),
        None,
    );
    upsert_session(&conn, &session_gaal).expect("insert gaal session");
    upsert_session(&conn, &session_orac).expect("insert orac session");

    let handoff_gaal = HandoffRecord {
        session_id: session_gaal.id.clone(),
        headline: Some("Improved gaal search UX".to_string()),
        projects: vec!["gaal".to_string()],
        keywords: vec!["search".to_string(), "index".to_string()],
        substance: 6,
        duration_minutes: 42,
        generated_at: Some("2026-02-25T10:41:00Z".to_string()),
        generated_by: Some("claude-3-7".to_string()),
        content_path: Some("/tmp/handoff-gaal.md".to_string()),
    };
    let handoff_orac = HandoffRecord {
        session_id: session_orac.id.clone(),
        headline: Some("Parser cleanup".to_string()),
        projects: vec!["orac".to_string()],
        keywords: vec!["parser".to_string()],
        substance: 5,
        duration_minutes: 36,
        generated_at: Some("2026-02-26T11:36:00Z".to_string()),
        generated_by: Some("gpt-5".to_string()),
        content_path: Some("/tmp/handoff-orac.md".to_string()),
    };

    upsert_handoff(&conn, &handoff_gaal).expect("insert gaal handoff");
    upsert_handoff(&conn, &handoff_orac).expect("insert orac handoff");

    let loaded_handoffs = vec![
        get_handoff(&conn, &session_gaal.id)
            .expect("load gaal handoff")
            .expect("gaal handoff exists"),
        get_handoff(&conn, &session_orac.id)
            .expect("load orac handoff")
            .expect("orac handoff exists"),
    ];

    let scores = score_handoffs_by_query(&loaded_handoffs, "gaal search");
    let gaal_score = *scores
        .get(&session_gaal.id)
        .expect("gaal score should be present");
    let orac_score = *scores
        .get(&session_orac.id)
        .expect("orac score should be present");

    assert!(
        gaal_score > orac_score,
        "expected gaal score ({gaal_score}) > orac score ({orac_score})"
    );
}

#[test]
fn exit_code_mapping() {
    assert_eq!(GaalError::NoResults.exit_code(), 1);
    assert_eq!(GaalError::AmbiguousId("abc".to_string()).exit_code(), 2);
    assert_eq!(GaalError::NotFound("x".to_string()).exit_code(), 3);
    assert_eq!(GaalError::NoIndex.exit_code(), 10);
    assert_eq!(
        GaalError::ParseError("bad input".to_string()).exit_code(),
        11
    );
}

#[test]
fn status_computation_for_completed_failed_and_active_sessions() {
    let conn = test_conn();

    let completed = session_row(
        "status-completed",
        "claude",
        Some("/repo/gaal"),
        "2026-03-01T08:00:00Z",
        Some("2026-03-01T08:20:00Z"),
        None,
    );
    let failed = session_row(
        "status-failed",
        "codex",
        Some("/repo/gaal"),
        "2026-03-01T09:00:00Z",
        Some("2026-03-01T09:20:00Z"),
        Some("error"),
    );
    let active = session_row(
        "status-active",
        "claude",
        Some("/repo/gaal"),
        "2026-03-01T10:00:00Z",
        None,
        None,
    );

    upsert_session(&conn, &completed).expect("insert completed");
    upsert_session(&conn, &failed).expect("insert failed");
    upsert_session(&conn, &active).expect("insert active");

    let completed_rows = list_sessions(
        &conn,
        &ListFilter {
            status: Some(vec!["completed".to_string()]),
            limit: Some(20),
            ..Default::default()
        },
    )
    .expect("list completed status");
    assert!(completed_rows.iter().any(|s| s.id == completed.id));
    assert!(!completed_rows.iter().any(|s| s.id == failed.id));
    assert!(!completed_rows.iter().any(|s| s.id == active.id));

    let failed_rows = list_sessions(
        &conn,
        &ListFilter {
            status: Some(vec!["failed".to_string()]),
            limit: Some(20),
            ..Default::default()
        },
    )
    .expect("list failed status");
    assert!(failed_rows.iter().any(|s| s.id == failed.id));
    assert!(!failed_rows.iter().any(|s| s.id == completed.id));
    assert!(!failed_rows.iter().any(|s| s.id == active.id));

    let unknown_rows = list_sessions(
        &conn,
        &ListFilter {
            status: Some(vec!["unknown".to_string()]),
            limit: Some(20),
            ..Default::default()
        },
    )
    .expect("list unknown status");
    assert!(unknown_rows.iter().any(|s| s.id == active.id));
    assert!(!unknown_rows.iter().any(|s| s.id == completed.id));
    assert!(!unknown_rows.iter().any(|s| s.id == failed.id));
}

#[test]
fn handoff_roundtrip_and_tag_operations() {
    let conn = test_conn();

    let session = session_row(
        "handoff-tags-1",
        "claude",
        Some("/repo/gaal"),
        "2026-03-01T14:00:00Z",
        Some("2026-03-01T14:45:00Z"),
        None,
    );
    upsert_session(&conn, &session).expect("insert session");

    let handoff = HandoffRecord {
        session_id: session.id.clone(),
        headline: Some("Shipped integration tests".to_string()),
        projects: vec!["gaal".to_string(), "tests".to_string()],
        keywords: vec!["integration".to_string(), "sqlite".to_string()],
        substance: 7,
        duration_minutes: 45,
        generated_at: Some("2026-03-01T14:46:00Z".to_string()),
        generated_by: Some("gpt-5".to_string()),
        content_path: Some("/tmp/handoff-tags-1.md".to_string()),
    };

    upsert_handoff(&conn, &handoff).expect("upsert handoff");

    let loaded = get_handoff(&conn, &session.id)
        .expect("get handoff")
        .expect("handoff exists");
    assert_eq!(loaded.session_id, handoff.session_id);
    assert_eq!(loaded.headline, handoff.headline);
    assert_eq!(loaded.projects, handoff.projects);
    assert_eq!(loaded.keywords, handoff.keywords);
    assert_eq!(loaded.substance, handoff.substance);
    assert_eq!(loaded.duration_minutes, handoff.duration_minutes);
    assert_eq!(loaded.generated_at, handoff.generated_at);
    assert_eq!(loaded.generated_by, handoff.generated_by);
    assert_eq!(loaded.content_path, handoff.content_path);

    add_tag(&conn, &session.id, "backend").expect("add backend tag");
    add_tag(&conn, &session.id, "priority").expect("add priority tag");
    add_tag(&conn, &session.id, "priority").expect("idempotent duplicate tag insert");

    let tags = get_tags(&conn, &session.id).expect("get tags");
    assert_eq!(tags, vec!["backend".to_string(), "priority".to_string()]);
}
