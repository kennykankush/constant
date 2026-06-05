//! End-to-end CLI regression tests.
//!
//! Each test runs the built `constant` binary as a subprocess with
//! `CODEX_HOME`/`CLAUDE_HOME`/`HOME` pointed at a per-test tempdir, so they never
//! touch the real session stores and never race on env (each process gets its
//! own). The conversation source is a hand-written neutral IR fixture (a
//! supported source format), so the tests don't depend on the native codex/claude
//! on-disk shapes.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU32, Ordering};

const BIN: &str = env!("CARGO_BIN_EXE_constant");

fn tmpdir() -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    // PID + nanos + counter so a reused PID from a prior run can't collide with a
    // stale directory (which would break the "wrote nothing" assertions).
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "constant-cli-test-{}-{}-{}",
        std::process::id(),
        nanos,
        N.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A minimal neutral IR conversation with a known origin runtime, so `carry` can
/// re-hydrate it and label the trail.
fn ir_fixture(dir: &Path) -> PathBuf {
    let path = dir.join("fixture.json");
    let ir = r#"{
  "ir_version": "transession/v1",
  "metadata": {
    "session_id": "fixture-0001",
    "source_format": "codex",
    "cwd": "/tmp/constant-cli-proj"
  },
  "events": [
    { "kind": "message", "role": "user",
      "blocks": [ { "kind": "text", "text": "hello from the fixture" } ] },
    { "kind": "message", "role": "assistant",
      "blocks": [ { "kind": "text", "text": "acknowledged" } ] }
  ]
}"#;
    std::fs::write(&path, ir).unwrap();
    path
}

fn run(dir: &Path, args: &[&str]) -> Output {
    Command::new(BIN)
        .args(args)
        .env("HOME", dir)
        .env("CODEX_HOME", dir.join("codex"))
        .env("CLAUDE_HOME", dir.join("claude"))
        .env("CLAUDE_CONFIG_DIR", dir.join("claude"))
        .env_remove("TRANSESSION_CODEX_HOME")
        .env_remove("TRANSESSION_CLAUDE_HOME")
        .output()
        .expect("failed to run constant")
}

fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).to_string()
}
fn err(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).to_string()
}

#[test]
fn doctor_json_is_valid() {
    let dir = tmpdir();
    let o = run(&dir, &["doctor", "--json"]);
    assert!(o.status.success(), "doctor failed: {}", err(&o));
    let v: serde_json::Value = serde_json::from_str(&out(&o)).expect("doctor emitted invalid JSON");
    assert!(v.get("codex").is_some() && v.get("claude").is_some());
}

#[test]
fn unknown_flag_is_rejected() {
    let dir = tmpdir();
    let o = run(&dir, &["doctor", "--nope"]);
    assert!(!o.status.success());
    assert!(err(&o).contains("unknown flag"), "{}", err(&o));
}

#[test]
fn carry_requires_a_source() {
    let dir = tmpdir();
    let o = run(&dir, &["carry", "--to", "claude"]);
    assert!(!o.status.success());
    assert!(err(&o).contains("requires"), "{}", err(&o));
}

#[test]
fn carry_from_and_session_are_mutually_exclusive() {
    let dir = tmpdir();
    let fix = ir_fixture(&dir);
    let o = run(
        &dir,
        &[
            "carry",
            "--to",
            "codex",
            "--from",
            "codex",
            "--session",
            fix.to_str().unwrap(),
        ],
    );
    assert!(!o.status.success());
    assert!(err(&o).contains("cannot be combined"), "{}", err(&o));
}

#[test]
fn missing_flag_value_errors() {
    let dir = tmpdir();
    let o = run(&dir, &["carry", "--to", "claude", "--session"]);
    assert!(!o.status.success());
    assert!(err(&o).contains("needs a value"), "{}", err(&o));
}

#[test]
fn carry_mints_target_and_preserves_source() {
    let dir = tmpdir();
    let fix = ir_fixture(&dir);
    let before = std::fs::read(&fix).unwrap();

    let o = run(
        &dir,
        &[
            "carry",
            "--to",
            "claude",
            "--session",
            fix.to_str().unwrap(),
            "--json",
        ],
    );
    assert!(o.status.success(), "carry failed: {}", err(&o));
    let v: serde_json::Value = serde_json::from_str(&out(&o)).expect("carry emitted invalid JSON");
    assert!(v.get("id").is_some() && v.get("resume").is_some());

    // F1: the source is byte-identical after the carry.
    assert_eq!(before, std::fs::read(&fix).unwrap(), "carry modified the source");
    // a claude projection was written into the isolated store.
    assert!(
        dir.join("claude").join("projects").exists(),
        "no claude session was created"
    );
}

#[test]
fn carry_dry_run_writes_nothing() {
    let dir = tmpdir();
    let fix = ir_fixture(&dir);
    let o = run(
        &dir,
        &[
            "carry",
            "--to",
            "claude",
            "--session",
            fix.to_str().unwrap(),
            "--dry-run",
        ],
    );
    assert!(o.status.success(), "{}", err(&o));
    assert!(out(&o).contains("would carry"));
    assert!(
        !dir.join("claude").join("projects").exists(),
        "dry-run wrote a session"
    );
}

#[test]
fn carry_logs_the_trail() {
    let dir = tmpdir();
    let fix = ir_fixture(&dir);
    let c = run(
        &dir,
        &["carry", "--to", "claude", "--session", fix.to_str().unwrap()],
    );
    assert!(c.status.success(), "{}", err(&c));
    let o = run(&dir, &["trail", "--all"]);
    assert!(o.status.success(), "{}", err(&o));
    assert!(
        out(&o).contains("hello-from-the-fixture"),
        "trail missing the conversation: {}",
        out(&o)
    );
}

#[test]
fn export_to_stdout_is_ir() {
    let dir = tmpdir();
    let fix = ir_fixture(&dir);
    let o = run(&dir, &["export", "--session", fix.to_str().unwrap()]);
    assert!(o.status.success(), "{}", err(&o));
    assert!(out(&o).contains("\"ir_version\""));
    assert!(out(&o).contains("hello from the fixture"));
}

#[test]
fn export_out_writes_a_file() {
    let dir = tmpdir();
    let fix = ir_fixture(&dir);
    let dest = dir.join("master.json");
    let o = run(
        &dir,
        &[
            "export",
            "--session",
            fix.to_str().unwrap(),
            "--out",
            dest.to_str().unwrap(),
        ],
    );
    assert!(o.status.success(), "{}", err(&o));
    assert!(dest.exists(), "export --out did not create the file");
    assert!(std::fs::read_to_string(&dest).unwrap().contains("\"ir_version\""));
}

#[test]
fn export_refuses_to_overwrite_its_source() {
    let dir = tmpdir();
    let fix = ir_fixture(&dir);
    let before = std::fs::read(&fix).unwrap();
    let o = run(
        &dir,
        &[
            "export",
            "--session",
            fix.to_str().unwrap(),
            "--out",
            fix.to_str().unwrap(),
        ],
    );
    assert!(!o.status.success(), "export overwrote its own source!");
    assert_eq!(before, std::fs::read(&fix).unwrap(), "source was modified");
}
