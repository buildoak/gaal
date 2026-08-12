use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

const COORDINATOR_ID: &str = "cafe0001";
const SUBAGENT_IDS: [&str; 2] = ["cafe0002", "cafe0003"];
const ORPHAN_ID: &str = "cafe0004";
const MISSING_PARENT_ID: &str = "dead0000";
const FIXTURE_CWD: &str = "/tmp/gaal-codex-fixture";

static CODEX_TEST_LOCK: Mutex<()> = Mutex::new(());

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
    std::env::temp_dir().join(format!("gaal-codex-test-{}-{nanos}", std::process::id()))
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
    // Tolerate poisoning: one failing test should report its own assertion
    // rather than cascade into every other test in the module.
    let guard = CODEX_TEST_LOCK
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    let root = unique_temp_root();
    let home = root.join("home");
    let gaal_home = root.join("gaal-home");
    copy_dir_all(&repo_root().join("tests/fixtures/codex/home"), &home);
    fs::create_dir_all(&gaal_home).expect("create GAAL_HOME");
    TestEnv {
        _guard: guard,
        root,
        home,
        gaal_home,
    }
}

fn command(env: &TestEnv, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_gaal"));
    command
        .args(args)
        .env("HOME", &env.home)
        .env("GAAL_HOME", &env.gaal_home)
        .env_remove("HERMES_HOME");
    command
}

fn gaal(env: &TestEnv, args: &[&str]) -> Output {
    command(env, args)
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

fn index_codex(env: &TestEnv) -> Output {
    gaal(env, &["index", "backfill", "--engine", "codex"])
}

fn session(env: &TestEnv, id: &str) -> Value {
    run_json(env, &["inspect", id])
}

#[test]
fn codex_subagents_link_to_a_parent_discovered_after_them() {
    let env = setup_env();
    let output = index_codex(&env);
    let stdout = assert_success(output, &["index", "backfill", "--engine", "codex"]);
    let summary: Value = serde_json::from_str(&stdout).expect("backfill summary is JSON");

    assert_eq!(
        summary["errors"], 0,
        "backfill must not drop sessions on a parent link: {summary}"
    );
    assert_eq!(
        summary["indexed"], 4,
        "all fixture sessions index: {summary}"
    );

    for id in SUBAGENT_IDS {
        let child = session(&env, id);
        assert_eq!(child["session_type"], "subagent", "{id} is a subagent");
        assert_eq!(
            child["parent_id"], COORDINATOR_ID,
            "{id} links to its coordinator"
        );
    }

    let parent = session(&env, COORDINATOR_ID);
    assert_eq!(
        parent["session_type"], "coordinator",
        "a parent with indexed subagents is promoted"
    );
}

#[test]
fn codex_subagent_with_unindexed_parent_stays_indexed_and_unlinked() {
    let env = setup_env();
    let output = index_codex(&env);
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert_success(output, &["index", "backfill", "--engine", "codex"]);

    let orphan = session(&env, ORPHAN_ID);
    assert_eq!(orphan["session_type"], "subagent");
    assert!(
        orphan.get("parent_id").is_none(),
        "an unindexed parent leaves the link unset rather than dangling: {orphan}"
    );
    assert!(
        stderr.contains(MISSING_PARENT_ID) && stderr.contains("not indexed"),
        "the dropped link is reported, not silent:\n{stderr}"
    );
}

#[test]
fn codex_search_reports_the_session_cwd() {
    let env = setup_env();
    assert_success(
        index_codex(&env),
        &["index", "backfill", "--engine", "codex"],
    );

    let found = run_json(
        &env,
        &["search", "orphan worker task", "--since", "2026-05-01"],
    );
    let results = found["results"]
        .as_array()
        .expect("search output contains results");
    let hit = results
        .iter()
        .find(|result| result["session_id"] == ORPHAN_ID)
        .unwrap_or_else(|| panic!("search finds the orphan session: {found}"));

    assert_eq!(
        hit["cwd"], FIXTURE_CWD,
        "a search hit carries the cwd needed to resume the session it came from"
    );
}

/// `gaal ... | head` closes stdout early. That is normal shell usage and must
/// not surface as an error payload or a non-zero exit.
#[test]
fn closing_stdout_early_exits_quietly() {
    let env = setup_env();
    assert_success(
        index_codex(&env),
        &["index", "backfill", "--engine", "codex"],
    );

    // Enough output to overflow the pipe buffer, so the writes are guaranteed
    // to hit the closed reader instead of being absorbed by the kernel.
    let rollout = env
        .home
        .join(".codex/sessions/2026/05/05")
        .join("rollout-2026-05-05T10-00-00-01960000-0000-7000-8000-0000cafe0001.jsonl");
    let mut padded = fs::read_to_string(&rollout).expect("read fixture rollout");
    padded.push_str(&padding_turns(2_000));
    fs::write(&rollout, padded).expect("pad fixture rollout");
    let backfill = ["index", "backfill", "--engine", "codex", "--force"];
    assert_success(gaal(&env, &backfill), &backfill);

    let args = ["transcript", COORDINATOR_ID, "--stdout"];
    let mut child = command(&env, &args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn gaal transcript");

    // Read one byte, then drop the reader: the remaining writes see EPIPE.
    let mut stdout = child.stdout.take().expect("child stdout is piped");
    let mut first = [0u8; 1];
    let _ = stdout.read(&mut first);
    drop(stdout);

    let output = child.wait_with_output().expect("wait for gaal transcript");
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(
        stderr.is_empty(),
        "a closed pipe must produce no diagnostics at all:\n{stderr}"
    );
    // Dying on SIGPIPE (exit 141) is the expected shape here, so only the
    // failure modes are asserted against: a panic (101) or an error payload.
    assert_ne!(
        output.status.code(),
        Some(101),
        "a closed pipe must not panic"
    );
}

/// Codex turns whose rendered transcript is large enough to fill a pipe buffer.
fn padding_turns(count: usize) -> String {
    (0..count)
        .flat_map(|idx| {
            [
                serde_json::json!({
                    "timestamp": "2026-05-05T10:00:01.000Z",
                    "type": "event_msg",
                    "payload": {
                        "type": "user_message",
                        "message": format!("padding prompt {idx}"),
                        "images": [],
                    },
                }),
                serde_json::json!({
                    "timestamp": "2026-05-05T10:00:02.000Z",
                    "type": "event_msg",
                    "payload": {
                        "type": "agent_message",
                        "message": format!("padding reply {idx}"),
                        "phase": "commentary",
                    },
                }),
            ]
        })
        .map(|record| format!("{record}\n"))
        .collect()
}
