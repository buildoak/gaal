use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use gaal::db::{
    init_db,
    queries::{upsert_handoff, upsert_session, SessionRow},
};
use gaal::model::HandoffRecord;
use rusqlite::Connection;
use serde_json::Value;

const PRIMARY_ID: &str = "12345678";
const FALLBACK_ID: &str = "fedcba09";
const AGY_OUTPUT_SALT: &str = "GAAL_SALT_AGY_OUTPUT_12345678";
static AGY_TEST_LOCK: Mutex<()> = Mutex::new(());

struct TestEnv {
    _guard: MutexGuard<'static, ()>,
    root: PathBuf,
    home: PathBuf,
    gaal_home: PathBuf,
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn unique_temp_root() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("gaal-agy-test-{}-{nanos}", std::process::id()))
}

fn copy_dir_all(from: &Path, to: &Path) {
    fs::create_dir_all(to).expect("create destination fixture directory");
    for entry in fs::read_dir(from).expect("read fixture directory") {
        let entry = entry.expect("read fixture entry");
        let file_type = entry.file_type().expect("read fixture entry type");
        let src = entry.path();
        let dst = to.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_all(&src, &dst);
        } else {
            fs::copy(&src, &dst).unwrap_or_else(|err| {
                panic!("copy fixture {} -> {}: {err}", src.display(), dst.display())
            });
        }
    }
}

fn setup_env() -> TestEnv {
    let guard = AGY_TEST_LOCK
        .lock()
        .expect("lock agy integration test guard");
    let root = unique_temp_root();
    let home = root.join("home");
    let gaal_home = root.join("gaal-home");
    copy_dir_all(&repo_root().join("tests/fixtures/agy/home"), &home);
    fs::create_dir_all(&gaal_home).expect("create GAAL_HOME");
    TestEnv {
        _guard: guard,
        root,
        home,
        gaal_home,
    }
}

fn gaal(env: &TestEnv, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_gaal"))
        .args(args)
        .env("HOME", &env.home)
        .env("GAAL_HOME", &env.gaal_home)
        .env_remove("HERMES_HOME")
        .output()
        .unwrap_or_else(|err| panic!("run gaal {args:?}: {err}"))
}

fn assert_success(output: Output, args: &[&str]) -> String {
    if !output.status.success() {
        panic!(
            "gaal {args:?} failed with status {}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    String::from_utf8(output.stdout).expect("gaal stdout is valid utf-8")
}

fn run_json(env: &TestEnv, args: &[&str]) -> Value {
    let stdout = assert_success(gaal(env, args), args);
    serde_json::from_str(&stdout)
        .unwrap_or_else(|err| panic!("parse JSON from gaal {args:?}: {err}\nstdout:\n{stdout}"))
}

fn session_ids(ls: &Value) -> Vec<String> {
    ls["sessions"]
        .as_array()
        .expect("ls output contains sessions array")
        .iter()
        .map(|session| {
            session["id"]
                .as_str()
                .expect("session id is a string")
                .to_string()
        })
        .collect()
}

fn index_agy(env: &TestEnv) -> Value {
    run_json(env, &["index", "backfill", "--engine", "agy", "--force"])
}

fn seed_recall_handoff(env: &TestEnv, substance: i32) {
    let conn = Connection::open(env.gaal_home.join("index.db")).expect("open test gaal index");
    init_db(&conn).expect("initialize test gaal index");
    let session = SessionRow {
        id: "lowagy00".to_string(),
        engine: "agy".to_string(),
        model: Some("Gemini 3.5 Flash (Low)".to_string()),
        cwd: Some("/tmp/agy-low-substance".to_string()),
        started_at: "2026-06-23T10:00:00Z".to_string(),
        ended_at: Some("2026-06-23T10:01:00Z".to_string()),
        exit_signal: None,
        last_event_at: Some("2026-06-23T10:01:00Z".to_string()),
        parent_id: None,
        session_type: "standalone".to_string(),
        jsonl_path: "/tmp/lowagy00.jsonl".to_string(),
        total_input_tokens: 0,
        total_output_tokens: 0,
        cache_read_tokens: 0,
        cache_creation_tokens: 0,
        reasoning_tokens: 0,
        total_tools: 0,
        total_turns: 1,
        peak_context: 0,
        last_indexed_offset: 0,
        subagent_type: None,
        gemini_summary: None,
    };
    upsert_session(&conn, &session).expect("upsert low-substance agy session");
    upsert_handoff(
        &conn,
        &HandoffRecord {
            session_id: session.id,
            headline: Some("Low substance agy banana smoke".to_string()),
            projects: vec!["agent-mux-agy".to_string()],
            keywords: vec!["banana".to_string(), "AGY_LOW_SUBSTANCE".to_string()],
            substance,
            duration_minutes: 1,
            generated_at: Some("2026-06-23T10:02:00Z".to_string()),
            generated_by: Some("test".to_string()),
            content_path: None,
        },
    )
    .expect("upsert low-substance agy handoff");
}

#[test]
fn agy_detect_engine_uses_content_markers_for_copied_jsonl() {
    let env = setup_env();
    let source = env.home.join(".gemini/antigravity-cli/brain/12345678-90ab-cdef-1234-567890abcdef/.system_generated/logs/transcript_full.jsonl");
    let copied = env.root.join("copied-agy-transcript.jsonl");
    fs::copy(&source, &copied).unwrap_or_else(|err| {
        panic!(
            "copy fixture {} -> {}: {err}",
            source.display(),
            copied.display()
        )
    });

    let engine = gaal::parser::detect_engine(&copied).expect("detect copied agy transcript");
    assert_eq!(engine, gaal::parser::Engine::Agy);
    assert_eq!(
        gaal::parser::extract_agy_session_id(&copied).as_deref(),
        Some(PRIMARY_ID)
    );
}

#[test]
fn agy_find_salt_scans_brain_transcripts_and_returns_brain_id() {
    let env = setup_env();

    let found = run_json(&env, &["find-salt", AGY_OUTPUT_SALT]);
    assert_eq!(found["engine"], "agy");
    assert_eq!(found["session_id"], PRIMARY_ID);
    assert!(
        found["jsonl_path"]
            .as_str()
            .is_some_and(|path| path.ends_with(".system_generated/logs/transcript_full.jsonl")),
        "find-salt should return the agy transcript path: {found:#}"
    );

    let filtered = run_json(&env, &["find-salt", "--engine", "agy", AGY_OUTPUT_SALT]);
    assert_eq!(filtered["engine"], "agy");
    assert_eq!(filtered["session_id"], PRIMARY_ID);
}

#[test]
fn agy_find_salt_rejects_unsupported_engine_filter() {
    let env = setup_env();

    let output = gaal(&env, &["find-salt", "--engine", "gemini", AGY_OUTPUT_SALT]);
    assert!(!output.status.success(), "gemini filter should be rejected");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("invalid value 'gemini'")
            && stderr.contains("claude")
            && stderr.contains("codex")
            && stderr.contains("agy"),
        "invalid engine error should teach supported salt engines, stderr:\n{stderr}"
    );
}

#[test]
fn agy_find_salt_ignores_user_prompt_echoes() {
    let env = setup_env();
    let user_only_salt = "GAAL_SALT_AGY_USER_ONLY_87654321";
    let logs = env
        .home
        .join(".gemini")
        .join("antigravity-cli")
        .join("brain")
        .join("87654321-90ab-cdef-1234-567890abcdef")
        .join(".system_generated")
        .join("logs");
    fs::create_dir_all(&logs).expect("create user-only agy logs dir");
    let transcript = logs.join("transcript_full.jsonl");
    fs::write(
        &transcript,
        format!(
            "{{\"step_index\":0,\"source\":\"USER_EXPLICIT\",\"type\":\"USER_INPUT\",\"event_type\":\"USER_INPUT\",\"status\":\"DONE\",\"created_at\":\"2026-06-22T10:00:00Z\",\"session_id\":\"87654321-90ab-cdef-1234-567890abcdef\",\"content\":\"please print {user_only_salt}\"}}\n"
        ),
    )
    .unwrap_or_else(|err| panic!("write user-only agy transcript {}: {err}", transcript.display()));

    let output = gaal(&env, &["find-salt", user_only_salt]);
    assert!(
        !output.status.success(),
        "find-salt must not match agy user prompt echoes\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn agy_backfill_indexes_native_and_fallback_sessions_for_ls() {
    let env = setup_env();

    let backfill = index_agy(&env);
    assert_eq!(backfill["indexed"], 2);
    assert_eq!(backfill["errors"], 0);

    let ls = run_json(
        &env,
        &[
            "ls",
            "--engine",
            "agy",
            "--since",
            "2026-06-01",
            "--all",
            "--limit",
            "10",
        ],
    );
    let ids = session_ids(&ls);
    assert!(ids.contains(&PRIMARY_ID.to_string()), "ls ids: {ids:?}");
    assert!(ids.contains(&FALLBACK_ID.to_string()), "ls ids: {ids:?}");
    assert!(
        ids.iter().all(|id| id.len() == 8),
        "agy sessions should use 8-char IDs: {ids:?}"
    );

    let primary = ls["sessions"]
        .as_array()
        .expect("sessions array")
        .iter()
        .find(|session| session["id"] == PRIMARY_ID)
        .expect("primary agy session listed");
    assert_eq!(primary["engine"], "agy");
    assert_eq!(primary["cwd"], "gaal-agy-fixture");
}

#[test]
fn agy_who_and_search_cover_file_command_web_and_image_facts() {
    let env = setup_env();

    let backfill = index_agy(&env);
    assert_eq!(backfill["indexed"], 2);

    let who_ran = run_json(
        &env,
        &[
            "who",
            "ran",
            "cargo",
            "--engine",
            "agy",
            "--since",
            "2026-06-01",
            "--full",
        ],
    );
    let ran_rows = who_ran["sessions"].as_array().expect("who ran rows");
    assert!(
        ran_rows.iter().any(|row| row["session_id"] == PRIMARY_ID
            && row["detail"]
                .as_str()
                .is_some_and(|detail| detail.contains("cargo test"))),
        "who ran should include the synthetic cargo command: {who_ran:#}"
    );

    let who_read = run_json(
        &env,
        &[
            "who",
            "read",
            "src/parser/agy.rs",
            "--engine",
            "agy",
            "--since",
            "2026-06-01",
            "--full",
        ],
    );
    let read_rows = who_read["sessions"].as_array().expect("who read rows");
    assert!(
        read_rows
            .iter()
            .any(|row| row["session_id"] == PRIMARY_ID && row["subject"] == "src/parser/agy.rs"),
        "who read should include the synthetic VIEW_FILE fact: {who_read:#}"
    );

    let web_search = run_json(
        &env,
        &[
            "search",
            "antigravity",
            "--engine",
            "agy",
            "--since",
            "2026-06-01",
            "--limit",
            "5",
        ],
    );
    let web_rows = web_search["results"]
        .as_array()
        .expect("web search result rows");
    assert!(
        web_rows.iter().any(|row| row["session_id"] == PRIMARY_ID),
        "search should find the synthetic SEARCH_WEB fact: {web_search:#}"
    );

    let search = run_json(
        &env,
        &[
            "search",
            "aurora-image-token",
            "--engine",
            "agy",
            "--since",
            "2026-06-01",
            "--limit",
            "5",
        ],
    );
    let search_rows = search["results"].as_array().expect("search result rows");
    assert!(
        search_rows
            .iter()
            .any(|row| row["session_id"] == PRIMARY_ID),
        "search should find the synthetic GENERATE_IMAGE fact: {search:#}"
    );

    let who_wrote_image = run_json(
        &env,
        &[
            "who",
            "wrote",
            "aurora-image-token",
            "--engine",
            "agy",
            "--since",
            "2026-06-01",
            "--full",
        ],
    );
    let wrote_rows = who_wrote_image["sessions"]
        .as_array()
        .expect("who wrote rows");
    assert!(
        wrote_rows
            .iter()
            .any(|row| row["session_id"] == PRIMARY_ID
                && row["subject"] == "tmp/aurora-image-token.png"),
        "who wrote should include the synthetic generated image artifact: {who_wrote_image:#}"
    );
    assert_eq!(
        wrote_rows
            .iter()
            .filter(|row| row["session_id"] == PRIMARY_ID)
            .count(),
        1,
        "who wrote should not duplicate generated image facts: {who_wrote_image:#}"
    );

    let failed = run_json(
        &env,
        &[
            "who",
            "ran",
            "false",
            "--engine",
            "agy",
            "--since",
            "2026-06-01",
            "--failed",
            "--full",
        ],
    );
    let failed_rows = failed["sessions"].as_array().expect("failed rows");
    assert!(
        failed_rows
            .iter()
            .any(|row| row["session_id"] == PRIMARY_ID && row["detail"].as_str() == Some("false")),
        "who ran --failed should include native agy exit_code metadata: {failed:#}"
    );
}

#[test]
fn agy_handoff_jsonl_dry_run_uses_brain_uuid_short_id() {
    let env = setup_env();
    let transcript = env.home.join(".gemini/antigravity-cli/brain/12345678-90ab-cdef-1234-567890abcdef/.system_generated/logs/transcript_full.jsonl");

    let dry_run = run_json(
        &env,
        &[
            "create-handoff",
            "--jsonl",
            transcript.to_str().expect("fixture path is utf-8"),
            "--engine",
            "codex",
            "--dry-run",
        ],
    );
    let rows = dry_run.as_array().expect("dry-run result rows");
    assert!(
        rows.iter()
            .any(|row| row["session_id"] == PRIMARY_ID && row["engine"] == "agy"),
        "agy --jsonl dry-run should use the source path/content engine and first 8 chars of brain UUID, not transcript filename or worker --engine: {dry_run:#}"
    );
}

#[test]
fn agy_create_handoff_dry_run_uses_current_default_provider_model_and_effort() {
    let env = setup_env();
    let transcript = env.home.join(".gemini/antigravity-cli/brain/12345678-90ab-cdef-1234-567890abcdef/.system_generated/logs/transcript_full.jsonl");

    let dry_run = run_json(
        &env,
        &[
            "create-handoff",
            "--jsonl",
            transcript.to_str().expect("fixture path is utf-8"),
            "--dry-run",
        ],
    );
    let row = dry_run
        .as_array()
        .and_then(|rows| rows.first())
        .expect("single dry-run row");
    assert_eq!(row["provider"], "agent-mux");
    assert_eq!(row["provider_supported"], true);
    assert_eq!(row["model"], "gpt-5.4-mini");
    assert_eq!(row["effort"], "xhigh");
    assert_eq!(row["engine"], "agy");
    assert_eq!(row["side_effects"]["spawn_provider_worker"], false);
    assert_eq!(row["side_effects"]["spend_tokens"], false);
}

#[test]
fn agy_handoff_jsonl_dry_run_detects_copied_agy_content() {
    let env = setup_env();
    let source = env.home.join(".gemini/antigravity-cli/brain/12345678-90ab-cdef-1234-567890abcdef/.system_generated/logs/transcript_full.jsonl");
    let copied = env.root.join("copied-agy-handoff.jsonl");
    fs::copy(&source, &copied).unwrap_or_else(|err| {
        panic!(
            "copy fixture {} -> {}: {err}",
            source.display(),
            copied.display()
        )
    });

    let dry_run = run_json(
        &env,
        &[
            "create-handoff",
            "--jsonl",
            copied.to_str().expect("fixture path is utf-8"),
            "--dry-run",
        ],
    );
    let rows = dry_run.as_array().expect("dry-run result rows");
    assert!(
        rows.iter()
            .any(|row| row["session_id"] == PRIMARY_ID && row["engine"] == "agy"),
        "copied agy --jsonl dry-run should use content-based agy detection: {dry_run:#}"
    );
}

#[test]
fn agy_batch_handoff_dry_run_includes_short_sessions_when_min_turns_zero() {
    let env = setup_env();
    let backfill = index_agy(&env);
    assert_eq!(backfill["indexed"], 2);

    let dry_run = run_json(
        &env,
        &[
            "create-handoff",
            "--batch",
            "--since",
            "30d",
            "--min-turns",
            "0",
            "--dry-run",
        ],
    );
    let rows = dry_run.as_array().expect("batch dry-run rows");
    assert!(
        rows.iter()
            .any(|row| row["session_id"] == PRIMARY_ID && row["status"] == "pending"),
        "batch dry-run should include primary agy session with min_turns=0: {dry_run:#}"
    );
    assert!(
        rows.iter()
            .any(|row| row["session_id"] == FALLBACK_ID && row["status"] == "pending"),
        "batch dry-run should include fallback agy session with min_turns=0: {dry_run:#}"
    );
}

#[test]
fn agy_recall_honors_explicit_zero_substance_threshold() {
    let env = setup_env();
    seed_recall_handoff(&env, 0);

    let default_output = gaal(&env, &["recall", "banana"]);
    assert!(
        !default_output.status.success(),
        "default recall should keep filtering substance=0 rows"
    );

    let recall = run_json(
        &env,
        &["recall", "banana", "--substance", "0", "--format", "brief"],
    );
    let rows = recall.as_array().expect("recall brief rows");
    assert!(
        rows.iter().any(|row| row
            .as_str()
            .is_some_and(|text| text.contains("lowagy00") && text.contains("substance: 0"))),
        "explicit --substance 0 should return low-substance agy handoff: {recall:#}"
    );
}

#[test]
fn agy_transcript_renders_from_full_and_fallback_jsonl() {
    let env = setup_env();

    let backfill = index_agy(&env);
    assert_eq!(backfill["indexed"], 2);

    let primary = run_json(&env, &["transcript", PRIMARY_ID, "--force"]);
    let primary_path = PathBuf::from(
        primary["path"]
            .as_str()
            .expect("primary transcript returns a path"),
    );
    assert!(primary_path.starts_with(&env.gaal_home));
    assert!(
        primary_path.ends_with("data/agy/sessions/2026/06/20/12345678.md"),
        "unexpected primary transcript path: {}",
        primary_path.display()
    );
    let primary_markdown = fs::read_to_string(&primary_path).expect("read primary transcript");
    assert!(primary_markdown.contains("Inspect the sanitized agy parser fixture"));
    assert!(primary_markdown.contains("aurora-image-token"));
    assert!(primary_markdown.contains("src/parser/agy.rs"));
    assert!(primary_markdown.contains("GenerateImage"));
    assert!(primary_markdown.contains("tmp/aurora-image-token.png"));

    let fallback_stdout = assert_success(
        gaal(&env, &["transcript", FALLBACK_ID, "--force", "--stdout"]),
        &["transcript", FALLBACK_ID, "--force", "--stdout"],
    );
    assert!(fallback_stdout.contains("Fallback transcript.jsonl fixture"));
    assert!(fallback_stdout.contains("src/discovery/agy.rs"));
}
