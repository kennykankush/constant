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
    ir_fixture_named(dir, "fixture.json", "fixture-0001", "hello from the fixture")
}

fn ir_fixture_named(dir: &Path, file: &str, id: &str, text: &str) -> PathBuf {
    let path = dir.join(file);
    let ir = format!(
        r#"{{
  "ir_version": "transession/v1",
  "metadata": {{
    "session_id": "{id}",
    "source_format": "codex",
    "cwd": "/tmp/constant-cli-proj"
  }},
  "events": [
    {{ "kind": "message", "role": "user",
      "blocks": [ {{ "kind": "text", "text": "{text}" }} ] }},
    {{ "kind": "message", "role": "assistant",
      "blocks": [ {{ "kind": "text", "text": "acknowledged" }} ] }}
  ]
}}"#
    );
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
    assert_eq!(
        before,
        std::fs::read(&fix).unwrap(),
        "carry modified the source"
    );
    // a claude projection was written into the isolated store.
    assert!(
        dir.join("claude").join("projects").exists(),
        "no claude session was created"
    );
}

#[test]
fn carry_reports_a_receipt() {
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
            "--json",
        ],
    );
    assert!(o.status.success(), "carry failed: {}", err(&o));
    let v: serde_json::Value = serde_json::from_str(&out(&o)).expect("carry emitted invalid JSON");
    assert_eq!(
        v["receipt"]["turns"].as_u64(),
        Some(2),
        "receipt missing turns: {v}"
    );

    let text = run(
        &dir,
        &[
            "carry",
            "--to",
            "claude",
            "--session",
            fix.to_str().unwrap(),
        ],
    );
    assert!(text.status.success(), "{}", err(&text));
    assert!(
        out(&text).contains("carried 2 turns"),
        "human output missing receipt: {}",
        out(&text)
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
    assert!(
        !dir.join(".constant").exists(),
        "dry-run created Constant state"
    );
}

#[test]
fn carry_debug_dry_run_writes_no_state_without_existing_trail() {
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
            "--debug",
        ],
    );
    assert!(o.status.success(), "{}", err(&o));
    assert!(out(&o).contains("route debug"));
    assert!(
        !dir.join("claude").join("projects").exists(),
        "debug dry-run wrote a session"
    );
    assert!(
        !dir.join(".constant").exists(),
        "debug dry-run created Constant state"
    );
}

#[test]
fn carry_logs_the_trail() {
    let dir = tmpdir();
    let fix = ir_fixture(&dir);
    let c = run(
        &dir,
        &[
            "carry",
            "--to",
            "claude",
            "--session",
            fix.to_str().unwrap(),
        ],
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
fn trail_dedupes_projection_refreshes_and_events_shows_raw_ledger() {
    let dir = tmpdir();
    let fix = ir_fixture(&dir);
    for _ in 0..2 {
        let c = run(
            &dir,
            &[
                "carry",
                "--to",
                "claude",
                "--session",
                fix.to_str().unwrap(),
            ],
        );
        assert!(c.status.success(), "{}", err(&c));
    }

    let current = run(&dir, &["trail", "--all"]);
    assert!(current.status.success(), "{}", err(&current));
    let current_out = out(&current);
    assert!(
        current_out.contains("synced 2x"),
        "trail should collapse repeated writes to one projection: {current_out}"
    );
    assert!(
        current_out.contains("events: 2"),
        "trail should point to raw events: {current_out}"
    );

    let raw = run(&dir, &["trail", "--all", "--events"]);
    assert!(raw.status.success(), "{}", err(&raw));
    let raw_out = out(&raw);
    assert!(
        raw_out.contains("t01"),
        "raw ledger missing first event: {raw_out}"
    );
    assert!(
        raw_out.contains("t02"),
        "raw ledger missing second event: {raw_out}"
    );
}

#[test]
fn route_prints_aliases_and_refreshes() {
    let dir = tmpdir();
    let fix = ir_fixture(&dir);
    for _ in 0..2 {
        let c = run(
            &dir,
            &[
                "carry",
                "--to",
                "claude",
                "--session",
                fix.to_str().unwrap(),
            ],
        );
        assert!(c.status.success(), "{}", err(&c));
    }

    let r = run(&dir, &["route", "--all"]);
    assert!(r.status.success(), "{}", err(&r));
    let text = out(&r);
    assert!(
        text.contains("codex[1] -> claude[1.1]"),
        "route missing aliases: {text}"
    );
    assert!(
        text.contains("synced 2x"),
        "route missing refresh count: {text}"
    );
    assert!(
        text.contains("refresh-existing"),
        "route missing refresh mode: {text}"
    );

    let scoped = run(&dir, &["route", "--session", fix.to_str().unwrap()]);
    assert!(scoped.status.success(), "{}", err(&scoped));
    assert!(
        out(&scoped).contains("claude[1.1]"),
        "session-scoped route missing fork alias"
    );
}

#[test]
fn legacy_trail_rows_still_refresh_existing_projection() {
    let dir = tmpdir();
    let fix = ir_fixture(&dir);

    let first = run(
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
    assert!(first.status.success(), "{}", err(&first));
    let first_json: serde_json::Value =
        serde_json::from_str(&out(&first)).expect("first carry emitted invalid JSON");
    assert!(first_json["id"].as_str().is_some());

    let second = run(
        &dir,
        &[
            "carry",
            "--to",
            "claude",
            "--session",
            fix.to_str().unwrap(),
            "--json",
            "--new",
        ],
    );
    assert!(second.status.success(), "{}", err(&second));
    let second_json: serde_json::Value =
        serde_json::from_str(&out(&second)).expect("second carry emitted invalid JSON");
    let second_id = second_json["id"].as_str().unwrap().to_string();

    let trail_path = dir.join(".constant").join("trail.jsonl");
    let legacy_lines = std::fs::read_to_string(&trail_path)
        .unwrap()
        .lines()
        .map(|line| {
            let mut v: serde_json::Value = serde_json::from_str(line).unwrap();
            let obj = v.as_object_mut().unwrap();
            obj.remove("source_id");
            obj.remove("source_path");
            obj.remove("target_id");
            obj.remove("target_path");
            obj.remove("mode");
            serde_json::to_string(&v).unwrap()
        })
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&trail_path, format!("{legacy_lines}\n")).unwrap();

    let refresh = run(
        &dir,
        &[
            "carry",
            "--to",
            "claude",
            "--session",
            fix.to_str().unwrap(),
            "--json",
            "--debug",
        ],
    );
    assert!(refresh.status.success(), "{}", err(&refresh));
    let refresh_json: serde_json::Value =
        serde_json::from_str(&out(&refresh)).expect("refresh carry emitted invalid JSON");
    assert_eq!(
        refresh_json["id"].as_str(),
        Some(second_id.as_str()),
        "legacy trail rows should refresh the latest existing projection"
    );
    assert_eq!(
        refresh_json["debug"]["mode"].as_str(),
        Some("refresh-existing")
    );
}

#[test]
fn carry_new_creates_a_sibling_projection() {
    let dir = tmpdir();
    let fix = ir_fixture(&dir);

    let first = run(
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
    assert!(first.status.success(), "{}", err(&first));
    let first_json: serde_json::Value =
        serde_json::from_str(&out(&first)).expect("first carry emitted invalid JSON");
    let first_id = first_json["id"].as_str().unwrap().to_string();

    let refresh = run(
        &dir,
        &[
            "carry",
            "--to",
            "claude",
            "--session",
            fix.to_str().unwrap(),
            "--json",
            "--debug",
        ],
    );
    assert!(refresh.status.success(), "{}", err(&refresh));
    let refresh_json: serde_json::Value =
        serde_json::from_str(&out(&refresh)).expect("refresh carry emitted invalid JSON");
    assert_eq!(
        refresh_json["id"].as_str(),
        Some(first_id.as_str()),
        "normal carry should refresh the existing projection"
    );
    assert_eq!(
        refresh_json["debug"]["mode"].as_str(),
        Some("refresh-existing")
    );

    let second = run(
        &dir,
        &[
            "carry",
            "--to",
            "claude",
            "--session",
            fix.to_str().unwrap(),
            "--json",
            "--debug",
            "--new",
        ],
    );
    assert!(second.status.success(), "{}", err(&second));
    let second_json: serde_json::Value =
        serde_json::from_str(&out(&second)).expect("new carry emitted invalid JSON");
    let second_id = second_json["id"].as_str().unwrap().to_string();
    assert_ne!(
        first_id, second_id,
        "--new should mint a separate target continuation"
    );
    assert_eq!(second_json["debug"]["mode"].as_str(), Some("new-fork"));
    assert_eq!(second_json["debug"]["new"].as_bool(), Some(true));

    let r = run(&dir, &["route", "--all"]);
    assert!(r.status.success(), "{}", err(&r));
    let text = out(&r);
    assert!(
        text.contains("codex[1] -> claude[1.1]"),
        "route missing first sibling: {text}"
    );
    assert!(
        text.contains("codex[1] -> claude[1.2]"),
        "route missing second sibling: {text}"
    );

    let codex_from_first = run(
        &dir,
        &[
            "carry",
            "--to",
            "codex",
            "--session",
            first_id.as_str(),
            "--json",
        ],
    );
    assert!(
        codex_from_first.status.success(),
        "{}",
        err(&codex_from_first)
    );
    let codex_first_json: serde_json::Value =
        serde_json::from_str(&out(&codex_from_first)).expect("codex carry emitted invalid JSON");
    let codex_first_id = codex_first_json["id"].as_str().unwrap().to_string();

    let codex_from_second = run(
        &dir,
        &[
            "carry",
            "--to",
            "codex",
            "--session",
            second_id.as_str(),
            "--json",
            "--debug",
        ],
    );
    assert!(
        codex_from_second.status.success(),
        "{}",
        err(&codex_from_second)
    );
    let codex_second_json: serde_json::Value = serde_json::from_str(&out(&codex_from_second))
        .expect("second codex carry emitted invalid JSON");
    let codex_second_id = codex_second_json["id"].as_str().unwrap();
    assert_ne!(
        codex_first_id, codex_second_id,
        "sibling Claude continuations should not collapse into one Codex projection"
    );
    assert_eq!(
        codex_second_json["debug"]["mode"].as_str(),
        Some("new-fork")
    );

    let branched = run(&dir, &["route", "--all"]);
    assert!(branched.status.success(), "{}", err(&branched));
    let branched_text = out(&branched);
    assert!(
        branched_text.contains("claude[1.1] -> codex[1.1.1]"),
        "route missing Codex child from first Claude sibling: {branched_text}"
    );
    assert!(
        branched_text.contains("claude[1.2] -> codex[1.2.1]"),
        "route missing Codex child from second Claude sibling: {branched_text}"
    );
}

#[test]
fn route_parents_same_id_sources_even_when_path_changes() {
    let dir = tmpdir();
    let fix = ir_fixture(&dir);

    let claude = run(
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
    assert!(claude.status.success(), "{}", err(&claude));
    let claude_json: serde_json::Value =
        serde_json::from_str(&out(&claude)).expect("claude carry emitted invalid JSON");
    let claude_id = claude_json["id"].as_str().unwrap().to_string();

    let codex = run(
        &dir,
        &[
            "carry",
            "--to",
            "codex",
            "--session",
            claude_id.as_str(),
            "--json",
            "--debug",
        ],
    );
    assert!(codex.status.success(), "{}", err(&codex));
    let codex_json: serde_json::Value =
        serde_json::from_str(&out(&codex)).expect("codex carry emitted invalid JSON");
    let codex_path = std::path::PathBuf::from(
        codex_json["debug"]["target_path"]
            .as_str()
            .expect("codex carry missing target_path"),
    );

    let original_child = run(
        &dir,
        &[
            "carry",
            "--to",
            "claude",
            "--session",
            codex_path.to_str().unwrap(),
            "--json",
            "--new",
        ],
    );
    assert!(original_child.status.success(), "{}", err(&original_child));

    let moved_path = codex_path.parent().unwrap().join(format!(
        "moved-{}",
        codex_path.file_name().unwrap().to_string_lossy()
    ));
    std::fs::copy(&codex_path, &moved_path).unwrap();

    let child = run(
        &dir,
        &[
            "carry",
            "--to",
            "claude",
            "--session",
            moved_path.to_str().unwrap(),
            "--json",
            "--new",
        ],
    );
    assert!(child.status.success(), "{}", err(&child));

    let r = run(&dir, &["route", "--all"]);
    assert!(r.status.success(), "{}", err(&r));
    let text = out(&r);
    assert!(
        text.contains("codex[1.1.1] -> claude[1.1.1.1]"),
        "route missing first child from original source path: {text}"
    );
    assert!(
        text.contains("codex[1.1.1] -> claude[1.1.1.2]"),
        "route should parent same-id moved source by id fallback without duplicate aliases: {text}"
    );
}

#[test]
fn carry_debug_dry_run_shows_route_decision_without_writing() {
    let dir = tmpdir();
    let fix = ir_fixture(&dir);
    let c = run(
        &dir,
        &[
            "carry",
            "--to",
            "claude",
            "--session",
            fix.to_str().unwrap(),
        ],
    );
    assert!(c.status.success(), "{}", err(&c));

    let d = run(
        &dir,
        &[
            "carry",
            "--to",
            "claude",
            "--session",
            fix.to_str().unwrap(),
            "--dry-run",
            "--debug",
        ],
    );
    assert!(d.status.success(), "{}", err(&d));
    let text = out(&d);
    assert!(text.contains("route debug"), "debug missing header: {text}");
    assert!(
        text.contains("action: refresh-existing"),
        "debug did not identify the reused projection: {text}"
    );

    let raw = run(&dir, &["trail", "--all", "--events"]);
    assert!(raw.status.success(), "{}", err(&raw));
    let raw_out = out(&raw);
    assert!(
        raw_out.contains("t01"),
        "trail lost original event: {raw_out}"
    );
    assert!(
        !raw_out.contains("t02"),
        "dry-run debug wrote a trail event: {raw_out}"
    );
}

#[test]
fn trail_current_view_hides_deleted_projection_but_events_keep_it() {
    let dir = tmpdir();
    let fix = ir_fixture(&dir);
    let c = run(
        &dir,
        &[
            "carry",
            "--to",
            "claude",
            "--session",
            fix.to_str().unwrap(),
        ],
    );
    assert!(c.status.success(), "{}", err(&c));

    // Simulate a user/runtime cleanup deleting the native projection. The
    // append-only event remains, but current trail must not print a dead resume
    // command as if it were usable.
    std::fs::remove_dir_all(dir.join("claude").join("projects")).unwrap();

    let current = run(&dir, &["trail", "--all"]);
    assert!(current.status.success(), "{}", err(&current));
    let current_out = out(&current);
    assert!(
        current_out.contains("projections: none"),
        "deleted projection should not be current: {current_out}"
    );
    assert!(
        !current_out.contains("resume: claude -r"),
        "current trail advertised a deleted projection: {current_out}"
    );

    let raw = run(&dir, &["trail", "--all", "--events"]);
    assert!(raw.status.success(), "{}", err(&raw));
    assert!(
        out(&raw).contains("claude"),
        "raw event ledger should preserve deleted projection event"
    );
}

#[test]
fn status_reports_runtime_trail_and_latest_sessions() {
    let dir = tmpdir();
    let fix = ir_fixture(&dir);
    let c = run(
        &dir,
        &[
            "carry",
            "--to",
            "claude",
            "--session",
            fix.to_str().unwrap(),
        ],
    );
    assert!(c.status.success(), "{}", err(&c));

    let s = run(&dir, &["status", "--all"]);
    assert!(s.status.success(), "{}", err(&s));
    let text = out(&s);
    for needle in ["constant status", "runtimes:", "trail:", "latest sessions:"] {
        assert!(text.contains(needle), "status missing {needle}: {text}");
    }
    assert!(
        !text.contains("hello-from-the-fixture"),
        "status should not print prompt-derived trail slugs: {text}"
    );
}

// --- with-tools: tool history carried, redacted, opt-in ---

fn ir_fixture_with_tools(dir: &Path) -> PathBuf {
    let path = dir.join("fixture-tools.json");
    let ir = r#"{
  "ir_version": "transession/v1",
  "metadata": {
    "session_id": "fixture-tools-0001",
    "source_format": "codex",
    "cwd": "/tmp/constant-cli-proj"
  },
  "events": [
    { "kind": "message", "role": "user",
      "blocks": [ { "kind": "text", "text": "deploy the service" } ] },
    { "kind": "tool_call", "call_id": "c1", "name": "bash",
      "arguments": { "command": "deploy --key sk-ARGSECRET1234567890ab" } },
    { "kind": "tool_result", "call_id": "c1",
      "output": "ok, used token: sk-OUTSECRET1234567890ab", "is_error": false },
    { "kind": "message", "role": "assistant",
      "blocks": [ { "kind": "text", "text": "deployed cleanly" } ] }
  ]
}"#;
    std::fs::write(&path, ir).unwrap();
    path
}

#[test]
fn carry_with_tools_keeps_redacted_tool_history() {
    let dir = tmpdir();
    let fix = ir_fixture_with_tools(&dir);

    // Default: conversation-only — tools dropped, receipt says so.
    let plain = run(
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
    assert!(plain.status.success(), "{}", err(&plain));
    let pv: serde_json::Value = serde_json::from_str(&out(&plain)).unwrap();
    assert_eq!(pv["receipt"]["tools"].as_u64(), Some(0));
    assert_eq!(pv["receipt"]["dropped_tools"].as_u64(), Some(2));

    // Opt-in: tools carried (as a fresh sibling), redacted, schema-complete.
    let with = run(
        &dir,
        &[
            "carry",
            "--to",
            "claude",
            "--session",
            fix.to_str().unwrap(),
            "--json",
            "--new",
            "--with-tools",
        ],
    );
    assert!(with.status.success(), "{}", err(&with));
    let wv: serde_json::Value = serde_json::from_str(&out(&with)).unwrap();
    assert_eq!(wv["receipt"]["tools"].as_u64(), Some(2), "{wv}");

    // Find the written claude session containing the tool events.
    let id = wv["id"].as_str().unwrap();
    let projects = dir.join("claude").join("projects");
    let mut session_file = None;
    let mut stack = vec![projects];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.file_name().unwrap().to_string_lossy() == format!("{id}.jsonl") {
                session_file = Some(p);
            }
        }
    }
    let text = std::fs::read_to_string(session_file.expect("tool projection missing")).unwrap();
    assert!(text.contains("tool_use"), "tool call not materialized");
    assert!(text.contains("tool_result"), "tool result not materialized");
    assert!(
        !text.contains("sk-ARGSECRET") && !text.contains("sk-OUTSECRET"),
        "tool payload secrets leaked"
    );
    // The strict resume schema applies to tool records too.
    let tool_line = text
        .lines()
        .find(|l| l.contains("tool_use"))
        .expect("tool_use line");
    assert!(
        tool_line.contains("\"entrypoint\":\"cli\"") && tool_line.contains("\"requestId\""),
        "tool record missing resume-strict fields: {tool_line}"
    );

    // The record volume for the with-tools hop holds the tools too.
    let snap = wv["snapshot"].as_str().expect("no snapshot");
    let snap_text = std::fs::read_to_string(snap).unwrap();
    assert!(
        snap_text.contains("tool_call") && !snap_text.contains("sk-ARGSECRET"),
        "record should hold redacted tools"
    );
}

// --- constant resume: the ledger as the front door ---

#[test]
fn resume_picks_the_latest_projection_and_prints_native_cmd_without_a_tty() {
    let dir = tmpdir();
    let fix = ir_fixture(&dir);
    let c = run(
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
    assert!(c.status.success(), "carry failed: {}", err(&c));
    let v: serde_json::Value = serde_json::from_str(&out(&c)).unwrap();
    let id = v["id"].as_str().unwrap();

    // No query: newest conversation. Tests run without a TTY, so resume must
    // degrade to printing the native command for exactly this projection.
    let r = run(&dir, &["resume", "--all"]);
    assert!(r.status.success(), "resume failed: {}", err(&r));
    let text = out(&r);
    assert!(
        text.contains(&format!("claude -r {id}")),
        "resume picked the wrong projection: {text}"
    );
    assert!(text.contains("not a terminal"), "{text}");

    // Slug query selects the same conversation; unknown query fails.
    let q = run(&dir, &["resume", "hello", "--all"]);
    assert!(q.status.success(), "{}", err(&q));
    assert!(out(&q).contains(&format!("claude -r {id}")));

    let bad = run(&dir, &["resume", "zzz-nope", "--all"]);
    assert!(!bad.status.success(), "unknown query should fail");
    assert!(err(&bad).contains("no conversation matches"), "{}", err(&bad));

    // --list shows the conversation and the command to resume it.
    let ls = run(&dir, &["resume", "--list", "--all"]);
    assert!(ls.status.success(), "{}", err(&ls));
    assert!(out(&ls).contains("hello-from-the-fixture"), "{}", out(&ls));
    assert!(out(&ls).contains("constant resume"), "{}", out(&ls));
}

#[test]
fn resume_with_ambiguous_query_lists_candidates_and_fails() {
    let dir = tmpdir();
    let one = ir_fixture(&dir);
    let two = ir_fixture_named(&dir, "fixture2.json", "fixture-0002", "goodbye fixture two");
    for fix in [&one, &two] {
        let c = run(
            &dir,
            &["carry", "--to", "claude", "--session", fix.to_str().unwrap()],
        );
        assert!(c.status.success(), "{}", err(&c));
    }

    let r = run(&dir, &["resume", "fixture", "--all"]);
    assert!(!r.status.success(), "ambiguous query should fail");
    assert!(err(&r).contains("narrow the query"), "{}", err(&r));
    let listed = out(&r);
    assert!(
        listed.contains("hello-from-the-fixture") && listed.contains("goodbye-fixture-two"),
        "candidates not listed: {listed}"
    );
}

#[test]
fn resume_restores_from_the_record_when_every_projection_is_gone() {
    let dir = tmpdir();
    let fix = ir_fixture(&dir);
    let c = run(
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
    assert!(c.status.success(), "carry failed: {}", err(&c));
    let v: serde_json::Value = serde_json::from_str(&out(&c)).unwrap();
    let carried_id = v["id"].as_str().unwrap();

    // Simulate cleanup wiping the live projection (isolated test store).
    std::fs::remove_dir_all(dir.join("claude").join("projects")).unwrap();

    // Lost-record doctrine: resume reprints from the latest record volume —
    // defaulting to the runtime the record came from (codex) — and offers it.
    let r = run(&dir, &["resume", "--all"]);
    assert!(r.status.success(), "resume failed: {}", err(&r));
    let text = out(&r);
    assert!(
        text.contains("restored from the record"),
        "no restore note: {text}"
    );
    assert!(text.contains("codex resume"), "no resume command: {text}");
    assert!(
        !text.contains(carried_id),
        "restore must mint a fresh id: {text}"
    );
}

// --- the record: per-hop IR snapshots + restore ---

#[test]
fn carry_writes_a_record_volume_and_restore_reprints_it() {
    let dir = tmpdir();
    let fix = ir_fixture(&dir);
    let c = run(
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
    assert!(c.status.success(), "carry failed: {}", err(&c));
    let v: serde_json::Value = serde_json::from_str(&out(&c)).expect("carry emitted invalid JSON");

    // The carry recorded a snapshot volume and reported its path.
    let snap = v["snapshot"].as_str().expect("carry reported no snapshot");
    let snap_path = std::path::PathBuf::from(snap);
    assert!(snap_path.exists(), "record volume missing: {snap}");
    assert!(
        snap.contains(".constant") && snap.contains("snapshots"),
        "record not in the vault: {snap}"
    );
    let content = std::fs::read_to_string(&snap_path).unwrap();
    assert!(content.contains("\"ir_version\""), "record is not IR");
    assert!(
        content.contains("hello from the fixture"),
        "record lost the conversation"
    );

    // `snapshots` lists it from the ledger.
    let ls = run(&dir, &["snapshots", "--all"]);
    assert!(ls.status.success(), "{}", err(&ls));
    let ls_out = out(&ls);
    assert!(ls_out.contains("t01"), "listing missing the volume: {ls_out}");
    assert!(ls_out.contains("ok"), "volume not marked ok: {ls_out}");
    assert!(
        ls_out.contains("restore latest:"),
        "listing missing restore hint: {ls_out}"
    );

    // Restore reprints a FRESH projection from the record — new id, record
    // untouched, lineage joined (the restore shows up as another event).
    let before = std::fs::read(&snap_path).unwrap();
    let carried_id = v["id"].as_str().unwrap();
    let r = run(&dir, &["restore", snap, "--to", "claude", "--json"]);
    assert!(r.status.success(), "restore failed: {}", err(&r));
    let rv: serde_json::Value =
        serde_json::from_str(&out(&r)).expect("restore emitted invalid JSON");
    let restored_id = rv["id"].as_str().expect("restore reported no id");
    assert_ne!(restored_id, carried_id, "restore must mint, not reuse");
    assert_eq!(
        before,
        std::fs::read(&snap_path).unwrap(),
        "restore modified the record"
    );
    assert_eq!(rv["receipt"]["turns"].as_u64(), Some(2));

    let events = run(&dir, &["trail", "--all", "--events"]);
    assert!(events.status.success(), "{}", err(&events));
    assert!(
        out(&events).contains("t02"),
        "restore not recorded in the ledger: {}",
        out(&events)
    );
}

#[test]
fn restore_defaults_to_the_record_source_runtime() {
    let dir = tmpdir();
    let fix = ir_fixture(&dir);
    // The fixture's IR declares source_format codex — restoring with no --to
    // must reprint a codex session.
    let r = run(&dir, &["restore", fix.to_str().unwrap(), "--json"]);
    assert!(r.status.success(), "restore failed: {}", err(&r));
    let rv: serde_json::Value =
        serde_json::from_str(&out(&r)).expect("restore emitted invalid JSON");
    assert_eq!(rv["to"].as_str(), Some("codex"));
    assert!(
        rv["resume"].as_str().unwrap_or("").starts_with("codex resume"),
        "wrong resume command: {rv}"
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
    assert!(
        std::fs::read_to_string(&dest)
            .unwrap()
            .contains("\"ir_version\"")
    );
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
