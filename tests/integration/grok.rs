use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

const SESSION_TOOL_ID: &str = "019f46d3-0000-7000-8000-000000000002";
const SESSION_TOOL_ALIAS: &str = "00000002";
const SESSION_FALLBACK_ID: &str = "019f46d3-0000-7000-8000-000000000003";
const FAKE_TG_TOKEN: &str = "1234567890:ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghi";

struct TestEnv {
    root: PathBuf,
    home: PathBuf,
    gaal_home: PathBuf,
    grok_home: PathBuf,
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
    std::env::temp_dir().join(format!("gaal-grok-test-{}-{nanos}", std::process::id()))
}

fn setup_env() -> TestEnv {
    let root = unique_temp_root();
    let home = root.join("home");
    let gaal_home = root.join("gaal-home");
    fs::create_dir_all(&home).expect("create HOME");
    fs::create_dir_all(&gaal_home).expect("create GAAL_HOME");
    let grok_home = repo_root().join("tests/fixtures/grok/home/.grok");
    TestEnv {
        root,
        home,
        gaal_home,
        grok_home,
    }
}

fn gaal(env: &TestEnv, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_gaal"))
        .args(args)
        .env("HOME", &env.home)
        .env("GAAL_HOME", &env.gaal_home)
        .env("GROK_HOME", &env.grok_home)
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

#[test]
fn grok_default_backfill_indexes_searches_and_renders_native_sessions() {
    let env = setup_env();

    let default_summary = run_json(&env, &["index", "backfill", "--force"]);
    assert_eq!(default_summary["indexed"], 3);
    assert_eq!(default_summary["errors"], 0);

    let explicit_summary = run_json(
        &env,
        &[
            "index",
            "backfill",
            "--engine",
            "grok",
            "--force",
            "--with-markdown",
        ],
    );
    // The just-advanced per-engine cursor keeps the immediate second pass empty.
    assert_eq!(explicit_summary["indexed"], 0);
    assert_eq!(explicit_summary["errors"], 0);

    let reindex_summary = run_json(&env, &["index", "reindex", SESSION_TOOL_ALIAS]);
    assert_eq!(reindex_summary["session_id"], SESSION_TOOL_ID);
    assert!(reindex_summary["facts"].as_u64().unwrap_or_default() > 0);

    let status = run_json(&env, &["index", "status"]);
    assert_eq!(status["grok"]["sessions_with_artifacts"], 3);
    assert!(
        status["grok"]["source_artifacts"]
            .as_i64()
            .unwrap_or_default()
            >= 3
    );
    assert!(
        status["grok"]["private_records"]
            .as_i64()
            .unwrap_or_default()
            > 0
    );
    assert!(
        status["grok"]["malformed_records"]
            .as_i64()
            .unwrap_or_default()
            >= 2
    );

    let source = run_json(&env, &["inspect", SESSION_TOOL_ID, "--source"]);
    assert_eq!(source["id"], SESSION_TOOL_ID);
    assert!(source["source"]["artifacts"]
        .as_array()
        .expect("source artifacts array")
        .iter()
        .any(|artifact| artifact["role"].as_str() == Some("updates")));
    assert!(source["source"]["observations"]
        .as_array()
        .expect("source observations array")
        .iter()
        .any(|observation| observation["kind"].as_str() == Some("source_selected")));
    assert!(source["source"]["artifacts"]
        .as_array()
        .expect("source artifacts array")
        .iter()
        .any(|artifact| {
            artifact["role"].as_str() == Some("updates")
                && artifact["parse_status"].as_str() == Some("partial_malformed")
        }));
    let source_text = source.to_string();
    assert!(!source_text.contains("GROK_PRIV_RAWINPUT_BODY_SHOULD_NOT_INDEX"));
    assert!(!source_text.contains("GROK_PRIV_RAWOUTPUT_BODY_SHOULD_NOT_INDEX"));

    let source_by_alias = run_json(&env, &["inspect", SESSION_TOOL_ALIAS, "--source"]);
    assert_eq!(source_by_alias["id"], SESSION_TOOL_ID);

    let resolved_alias = run_json(&env, &["resolve", SESSION_TOOL_ALIAS]);
    assert_eq!(resolved_alias["session_id"], SESSION_TOOL_ID);
    assert_eq!(resolved_alias["short_id"], SESSION_TOOL_ID);

    let resolve_human = assert_success(
        gaal(&env, &["resolve", SESSION_TOOL_ALIAS, "-H"]),
        &["resolve", SESSION_TOOL_ALIAS, "-H"],
    );
    assert!(resolve_human.contains("Source:"));
    assert!(!resolve_human.contains("JSONL:"));

    let inspect_help = assert_success(gaal(&env, &["inspect", "--help"]), &["inspect", "--help"]);
    assert!(inspect_help.contains("Source-artifact diagnostics and path"));

    let salt = run_json(
        &env,
        &[
            "find-salt",
            "GROK_VISIBLE_TOOL_RESULT_SHOULD_INDEX",
            "--engine",
            "grok",
        ],
    );
    assert_eq!(salt["session_id"], SESSION_TOOL_ID);
    assert_eq!(salt["engine"], "grok");
    assert_eq!(salt["indexed"], true);
    assert_eq!(salt["source_role"], "updates");
    assert!(salt["matched_source_path"]
        .as_str()
        .is_some_and(|path| path.ends_with("updates.jsonl")));
    let salt_path = salt["jsonl_path"].as_str().expect("salt source path");
    assert!(salt_path.ends_with(SESSION_TOOL_ID));
    assert!(!salt_path.ends_with("updates.jsonl"));

    let salt_human = assert_success(
        gaal(
            &env,
            &[
                "find-salt",
                "GROK_VISIBLE_TOOL_RESULT_SHOULD_INDEX",
                "--engine",
                "grok",
                "-H",
            ],
        ),
        &[
            "find-salt",
            "GROK_VISIBLE_TOOL_RESULT_SHOULD_INDEX",
            "--engine",
            "grok",
            "-H",
        ],
    );
    assert!(salt_human.contains("Source:"));
    assert!(!salt_human.contains("JSONL:"));

    let chat_salt = run_json(
        &env,
        &[
            "find-salt",
            "GROK_VISIBLE_CHAT_TOOL_RESULT_SHOULD_INDEX",
            "--engine",
            "grok",
        ],
    );
    assert_eq!(chat_salt["session_id"], SESSION_FALLBACK_ID);
    assert_eq!(chat_salt["source_role"], "chat_history");
    assert!(chat_salt["matched_source_path"]
        .as_str()
        .is_some_and(|path| path.ends_with("chat_history.jsonl")));

    let user_prompt_salt = gaal(
        &env,
        &[
            "find-salt",
            "GROK_VISIBLE_USER_PROMPT_SHOULD_INDEX",
            "--engine",
            "grok",
        ],
    );
    assert!(!user_prompt_salt.status.success());
    assert!(String::from_utf8_lossy(&user_prompt_salt.stderr)
        .contains("No visible session source artifact contains salt token"));

    let handoff_dry_run = run_json(
        &env,
        &[
            "create-handoff",
            "--jsonl",
            salt_path,
            "--dry-run",
            "--engine",
            "codex",
        ],
    );
    let dry_run = handoff_dry_run
        .as_array()
        .and_then(|items| items.first())
        .expect("handoff dry-run array");
    assert_eq!(dry_run["strategy"], "single");
    assert_eq!(dry_run["session_id"], SESSION_TOOL_ID);
    assert_eq!(dry_run["engine"], "grok");
    assert_eq!(dry_run["source_engine"], "grok");
    assert_eq!(dry_run["worker_engine"], "codex");
    assert_eq!(dry_run["indexed"], true);
    assert!(dry_run["handoff_path"]
        .as_str()
        .is_some_and(|path| path.ends_with(&format!(
            "/data/grok/handoffs/2026/07/09/{SESSION_TOOL_ID}.md"
        ))));

    let ls = run_json(&env, &["ls", "--engine", "grok", "--limit", "10"]);
    assert_eq!(ls["total_unfiltered"], 3);
    let sessions = ls["sessions"].as_array().expect("ls sessions array");
    assert!(sessions.iter().any(|session| {
        session["id"].as_str() == Some(SESSION_TOOL_ID)
            && session["short_id"].as_str() == Some(SESSION_TOOL_ID)
    }));

    let fallback_search = assert_success(
        gaal(&env, &["search", "Fallback", "-H"]),
        &["search", "Fallback", "-H"],
    );
    assert!(fallback_search.contains(SESSION_FALLBACK_ID));
    assert!(fallback_search.contains("Fallback user message GROK_VISIBLE_USER_PROMPT_SHOULD_INDEX"));

    let oversized_search = assert_success(
        gaal(
            &env,
            &["search", "GROK_VISIBLE_OVERSIZE_OUTPUT_SHOULD_INDEX", "-H"],
        ),
        &["search", "GROK_VISIBLE_OVERSIZE_OUTPUT_SHOULD_INDEX", "-H"],
    );
    assert!(oversized_search.contains("GROK_VISIBLE_OVERSIZE_OUTPUT_SHOULD_INDEX"));
    assert!(!oversized_search.contains("GROK_OVERSIZE_TAIL_SHOULD_NOT_INDEX"));
    let truncation_marker_search = assert_success(
        gaal(&env, &["search", "GAAL_TRUNCATED_TOOL_OUTPUT", "-H"]),
        &["search", "GAAL_TRUNCATED_TOOL_OUTPUT", "-H"],
    );
    assert!(truncation_marker_search.contains("retained_chars=16000"));
    let oversized_tail_search = gaal(
        &env,
        &["search", "GROK_OVERSIZE_TAIL_SHOULD_NOT_INDEX", "-H"],
    );
    assert!(!oversized_tail_search.status.success());
    assert!(String::from_utf8_lossy(&oversized_tail_search.stderr)
        .contains("No indexed facts matched that search query"));

    let private_search = gaal(&env, &["search", "GROK_PRIV", "-H"]);
    assert!(!private_search.status.success());
    assert!(String::from_utf8_lossy(&private_search.stderr)
        .contains("No indexed facts matched that search query"));

    let raw_secret_search = gaal(&env, &["search", FAKE_TG_TOKEN, "-H"]);
    assert!(!raw_secret_search.status.success());
    assert!(String::from_utf8_lossy(&raw_secret_search.stderr)
        .contains("No indexed facts matched that search query"));

    let redacted_secret_search = assert_success(
        gaal(&env, &["search", "REDACTED_TELEGRAM_BOT_TOKEN", "-H"]),
        &["search", "REDACTED_TELEGRAM_BOT_TOKEN", "-H"],
    );
    assert!(redacted_secret_search.contains("REDACTED_TELEGRAM_BOT_TOKEN"));
    assert!(!redacted_secret_search.contains(FAKE_TG_TOKEN));

    let transcript = assert_success(
        gaal(
            &env,
            &["transcript", SESSION_TOOL_ALIAS, "--force", "--stdout"],
        ),
        &["transcript", SESSION_TOOL_ALIAS, "--force", "--stdout"],
    );
    assert!(transcript.contains("GROK_VISIBLE_TOOL_RESULT_SHOULD_INDEX"));
    assert!(transcript.contains("GROK_VISIBLE_TOOL_FAILURE_SHOULD_INDEX"));
    assert!(transcript.contains("GROK_VISIBLE_READ_RESULT_SHOULD_INDEX"));
    assert!(transcript.contains("GROK_VISIBLE_WRITE_RESULT_SHOULD_INDEX"));
    assert!(transcript.contains("GROK_VISIBLE_LIST_RESULT_SHOULD_INDEX"));
    assert!(transcript.contains("GROK_VISIBLE_OVERSIZE_OUTPUT_SHOULD_INDEX"));
    assert!(transcript.contains("GAAL_TRUNCATED_TOOL_OUTPUT"));
    assert!(!transcript.contains("GROK_OVERSIZE_TAIL_SHOULD_NOT_INDEX"));
    assert!(!transcript.contains("GROK_VISIBLE_DUPLICATE_TERMINAL_RESULT_SHOULD_NOT_INDEX"));
    assert!(!transcript.contains("GROK_VISIBLE_DUPLICATE_SHELL_RESULT_SHOULD_NOT_INDEX"));
    assert!(transcript.contains("[REDACTED_TELEGRAM_BOT_TOKEN]"));
    assert!(!transcript.contains(FAKE_TG_TOKEN));
    assert!(!transcript.contains("GROK_PRIV_RAWINPUT_BODY_SHOULD_NOT_INDEX"));
    assert!(!transcript.contains("GROK_PRIV_RAWOUTPUT_BODY_SHOULD_NOT_INDEX"));
    assert!(!transcript.contains("GROK_PRIV_LIST_BODY_SHOULD_NOT_INDEX"));

    let fallback_transcript = assert_success(
        gaal(
            &env,
            &["transcript", SESSION_FALLBACK_ID, "--force", "--stdout"],
        ),
        &["transcript", SESSION_FALLBACK_ID, "--force", "--stdout"],
    );
    assert!(fallback_transcript.contains("GROK_VISIBLE_CHAT_TOOL_RESULT_SHOULD_INDEX"));
    assert!(fallback_transcript.contains("GROK_VISIBLE_CHAT_WRITE_RESULT_SHOULD_INDEX"));
    assert!(fallback_transcript.contains("[REDACTED_TELEGRAM_BOT_TOKEN]"));
    assert!(!fallback_transcript.contains(FAKE_TG_TOKEN));
    assert!(!fallback_transcript.contains("GROK_PRIV_CHAT_READ_BODY_SHOULD_NOT_INDEX"));
    assert!(!fallback_transcript.contains("GROK_PRIV_CHAT_WRITE_BODY_SHOULD_NOT_INDEX"));
    assert!(!fallback_transcript.contains("GROK_PRIV_CHAT_IMAGE_SHOULD_NOT_INDEX"));

    let activity = assert_success(
        gaal(
            &env,
            &[
                "activity",
                "--session",
                SESSION_TOOL_ID,
                "--since",
                "2026-07-09",
                "--before",
                "2026-07-10",
                "--force",
                "--stdout",
            ],
        ),
        &[
            "activity",
            "--session",
            SESSION_TOOL_ID,
            "--since",
            "2026-07-09",
            "--before",
            "2026-07-10",
            "--force",
            "--stdout",
        ],
    );
    assert!(activity.contains("GROK_VISIBLE_TOOL_RESULT_SHOULD_INDEX"));
    assert!(!activity.contains("GROK_PRIV_RAWOUTPUT_BODY_SHOULD_NOT_INDEX"));
}
