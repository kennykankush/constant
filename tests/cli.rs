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
        .env("CONSTANT_NO_UPDATE_CHECK", "1")
        .env("XDG_DATA_HOME", dir.join("xdg"))
        .env_remove("TRANSESSION_CODEX_HOME")
        .env_remove("TRANSESSION_CLAUDE_HOME")
        .env_remove("CONSTANT_GEMINI_HOME")
        .output()
        .expect("failed to run constant")
}

/// Like `run`, but from a specific working directory (for cwd-scoped discovery).
fn run_in(dir: &Path, cwd: &Path, args: &[&str]) -> Output {
    Command::new(BIN)
        .args(args)
        .current_dir(cwd)
        .env("HOME", dir)
        .env("CODEX_HOME", dir.join("codex"))
        .env("CLAUDE_HOME", dir.join("claude"))
        .env("CLAUDE_CONFIG_DIR", dir.join("claude"))
        .env("CONSTANT_NO_UPDATE_CHECK", "1")
        .env("XDG_DATA_HOME", dir.join("xdg"))
        .env_remove("TRANSESSION_CODEX_HOME")
        .env_remove("TRANSESSION_CLAUDE_HOME")
        .env_remove("CONSTANT_GEMINI_HOME")
        .output()
        .expect("failed to run constant")
}

/// Find the claude projection file for a session id in the isolated store.
fn find_claude_projection(dir: &Path, id: &str) -> Option<PathBuf> {
    let mut stack = vec![dir.join("claude").join("projects")];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.file_name().map(|n| n.to_string_lossy() == format!("{id}.jsonl"))
                == Some(true)
            {
                return Some(p);
            }
        }
    }
    None
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

/// `--session` resolves the spellings a person actually types: the trail
/// HANDLE (globally unique) and the conversation NAME — and duplicate names
/// are refused with the candidates listed (claude/codex allow duplicates).
#[test]
fn carry_session_resolves_handles_and_names() {
    let dir = tmpdir();
    let fix = ir_fixture(&dir);
    let o = run(
        &dir,
        &["carry", "--to", "claude", "--session", fix.to_str().unwrap()],
    );
    assert!(o.status.success(), "{}", err(&o));

    // The handle the trail minted for this conversation.
    let trail_out = out(&run(&dir, &["trail", "--all", "--plain"]));
    let handle = trail_out
        .split_whitespace()
        .find(|w| {
            w.contains('-')
                && w.split_once('-')
                    .map(|(c, t)| c.chars().all(|ch| ch.is_ascii_lowercase())
                        && !c.is_empty()
                        && t.chars().all(|ch| ch.is_ascii_digit())
                        && !t.is_empty())
                    .unwrap_or(false)
        })
        .unwrap_or_else(|| panic!("no handle in trail output: {trail_out}"))
        .to_string();

    // Carry BY HANDLE: resolves to the conversation's latest projection.
    let o = run(&dir, &["carry", "--to", "codex", "--session", &handle, "--json"]);
    assert!(o.status.success(), "carry by handle failed: {}", err(&o));

    // Carry BY NAME: the projection's display name (the stamped trail name,
    // "from-the-fixture") — a unique contains-match resolves without a TTY.
    let o = run(
        &dir,
        &["carry", "--to", "codex", "--session", "from-the-fixture", "--json"],
    );
    assert!(o.status.success(), "carry by name failed: {}", err(&o));
}

#[test]
fn carry_session_refuses_duplicate_names_without_a_terminal() {
    let dir = tmpdir();
    let a = ir_fixture_named(&dir, "a.json", "dup-aaaa", "duplicate name here");
    let b = ir_fixture_named(&dir, "b.json", "dup-bbbb", "duplicate name here");
    for f in [&a, &b] {
        let o = run(
            &dir,
            &["carry", "--to", "claude", "--session", f.to_str().unwrap()],
        );
        assert!(o.status.success(), "{}", err(&o));
    }

    // Two claude projections now share the name. Piped (no TTY): list + bail.
    let o = run(
        &dir,
        &["carry", "--to", "codex", "--session", "duplicate-name-here"],
    );
    assert!(!o.status.success(), "duplicate names must not auto-pick");
    assert!(
        err(&o).contains("several sessions match"),
        "wrong error: {}",
        err(&o)
    );
    assert!(
        err(&o).contains("pick one by id"),
        "should teach the way out: {}",
        err(&o)
    );
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
        out(&o).contains("from-the-fixture"),
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

    let current = run(&dir, &["trail", "--all", "--full"]);
    assert!(current.status.success(), "{}", err(&current));
    let current_out = out(&current);
    assert!(
        current_out.contains("synced 2x"),
        "trail --full should collapse repeated writes to one projection: {current_out}"
    );
    // The default view stays compact: sync counts render as ×N.
    let compact = run(&dir, &["trail", "--all"]);
    assert!(
        out(&compact).contains("\u{d7}2"),
        "compact trail should show the refresh count: {}",
        out(&compact)
    );
    assert!(
        current_out.contains("events: 2"),
        "trail should point to raw events: {current_out}"
    );

    let raw = run(&dir, &["trail", "--all", "--events"]);
    assert!(raw.status.success(), "{}", err(&raw));
    let raw_out = out(&raw);
    assert!(
        raw_out.contains("ch01"),
        "raw ledger missing first event: {raw_out}"
    );
    assert!(
        raw_out.contains("ch02"),
        "raw ledger missing second event: {raw_out}"
    );
}

/// In a terminal `constant trail` opens the interactive explorer; everywhere
/// else (pipes, scripts, --plain) it MUST stay the printable card view. The
/// other trail tests already pipe bare `trail`, so this pins the explicit
/// opt-out: --plain is accepted and prints the same cards.
#[test]
fn trail_plain_and_sessions_plain_keep_the_printouts() {
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

    let plain = run(&dir, &["trail", "--all", "--plain"]);
    assert!(plain.status.success(), "{}", err(&plain));
    let plain_out = out(&plain);
    assert!(
        plain_out.contains("constant resume"),
        "trail --plain should print the card view: {plain_out}"
    );
    // Identical to the piped default — --plain is the same view, just forced.
    assert_eq!(plain_out, out(&run(&dir, &["trail", "--all"])));

    let sessions = run(&dir, &["sessions", "--all", "--plain"]);
    assert!(sessions.status.success(), "{}", err(&sessions));
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
fn route_json_emits_the_lineage_dag() {
    let dir = tmpdir();
    let fix = ir_fixture(&dir);
    let c = run(
        &dir,
        &["carry", "--to", "claude", "--session", fix.to_str().unwrap()],
    );
    assert!(c.status.success(), "{}", err(&c));

    let r = run(&dir, &["route", "--json", "--all"]);
    assert!(r.status.success(), "{}", err(&r));
    let v: serde_json::Value =
        serde_json::from_str(&out(&r)).expect("route --json emitted invalid JSON");
    let convs = v.as_array().expect("route --json is an array");
    assert!(!convs.is_empty(), "route --json empty after a carry");
    let nodes = convs[0]["nodes"].as_array().expect("nodes array");
    let node = &nodes[0];
    assert_eq!(node["runtime"], "claude");
    assert_eq!(node["parent"], "codex[1]", "parent alias missing from DAG");
    assert!(
        node["alias"].as_str().unwrap().starts_with("claude["),
        "node alias: {}",
        node["alias"]
    );
    assert!(
        node["resume"].as_str().unwrap().starts_with("claude -r "),
        "node resume: {}",
        node["resume"]
    );
    assert_eq!(node["active"], true);
}

#[test]
fn audit_reports_paged_render_stats_per_chapter() {
    let dir = tmpdir();
    let fix = ir_fixture(&dir);
    let c = run(
        &dir,
        &["carry", "--to", "claude", "--session", fix.to_str().unwrap()],
    );
    assert!(c.status.success(), "{}", err(&c));

    let a = run(&dir, &["audit", "--all", "--json"]);
    assert!(a.status.success(), "{}", err(&a));
    let v: serde_json::Value =
        serde_json::from_str(&out(&a)).expect("audit --json emitted invalid JSON");
    let convs = v.as_array().expect("audit --json is an array");
    assert!(
        !convs.is_empty(),
        "audit found no recorded chapters after a carry"
    );
    let chapters = convs[0]["chapters"].as_array().expect("chapters array");
    let ch = &chapters[0];
    // The 2-turn fixture fits the tail budget: everything stays verbatim, nothing filed.
    assert_eq!(ch["turns"], 2);
    assert_eq!(ch["verbatim"], 2);
    assert_eq!(ch["filed"], 0);
    assert!(ch["tail_chars"].as_u64().unwrap() > 0);
}

#[test]
fn route_json_excludes_rename_nodes() {
    let dir = tmpdir();
    let fix = ir_fixture(&dir);
    let c = run(
        &dir,
        &[
            "carry", "--to", "claude", "--session", fix.to_str().unwrap(), "--json",
        ],
    );
    assert!(c.status.success(), "{}", err(&c));
    let cj: serde_json::Value =
        serde_json::from_str(&out(&c)).expect("carry --json emitted invalid JSON");
    let handle = cj["handle"].as_str().expect("carry --json has no handle").to_string();

    // A rename is a naming event, not a carry — it must never become a DAG node.
    let r = run(&dir, &["rename", "--of", &handle, "renamed", "thread"]);
    assert!(r.status.success(), "rename failed: {}", err(&r));

    let j = run(&dir, &["route", "--json", "--all"]);
    assert!(j.status.success(), "{}", err(&j));
    let v: serde_json::Value =
        serde_json::from_str(&out(&j)).expect("route --json emitted invalid JSON");
    for conv in v.as_array().unwrap() {
        // The rename row (recorded the same second as the carry) must not become
        // the root either.
        assert_ne!(conv["root_runtime"], "?", "rename poisoned the DAG root: {conv}");
        assert_ne!(conv["root"], "?[1]", "rename poisoned the DAG root alias: {conv}");
        for node in conv["nodes"].as_array().unwrap() {
            assert_ne!(node["mode"], "rename", "rename leaked into the DAG: {node}");
            assert_ne!(node["runtime"], "?", "phantom node in the DAG: {node}");
        }
    }
}

#[test]
fn audit_surfaces_a_missing_record_volume() {
    let dir = tmpdir();
    let fix = ir_fixture(&dir);
    let c = run(
        &dir,
        &[
            "carry", "--to", "claude", "--session", fix.to_str().unwrap(), "--json",
        ],
    );
    assert!(c.status.success(), "{}", err(&c));
    let cj: serde_json::Value =
        serde_json::from_str(&out(&c)).expect("carry --json emitted invalid JSON");
    let snapshot = cj["snapshot"].as_str().expect("carry recorded no snapshot").to_string();
    assert!(Path::new(&snapshot).exists(), "snapshot volume was not written");

    // Simulate record loss (a cleanup or drift deleted the volume the ledger
    // still references) — in this isolated test store only.
    std::fs::remove_file(&snapshot).expect("could not remove test snapshot");

    let a = run(&dir, &["audit", "--all", "--json"]);
    assert!(a.status.success(), "{}", err(&a));
    let v: serde_json::Value =
        serde_json::from_str(&out(&a)).expect("audit --json emitted invalid JSON");
    // The chapter must be SURFACED with a missing-record status, not silently
    // dropped (declared lossiness — an audit can't hide a record-integrity hole).
    let statuses: Vec<String> = v
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|c| c["chapters"].as_array().unwrap())
        .map(|ch| ch["status"].as_str().unwrap_or("").to_string())
        .collect();
    assert!(
        statuses.iter().any(|s| s.contains("missing")),
        "a missing record volume was hidden by audit; statuses: {statuses:?}"
    );
}

#[test]
fn recall_reads_the_ledgers_snapshot_not_the_reconstructed_path() {
    // Regression: `recall <handle> chNN` must open the volume the LEDGER recorded,
    // never reconstruct `snapshot_path(conv, n, from)`. The physical
    // `chNN-from-<rt>.json` filename is a per-carry write counter; the chapter `n`
    // is the collapsed user-facing number. After a reseat / compaction / restore /
    // import the two diverge, and reconstruction then opens the WRONG volume
    // (proven live on iris-19: `recall ch01` reconstructed an orphaned 805-turn
    // first volume while the ledger's chapter 1 pointed at a 79-turn one). `audit`
    // and `latest_snapshot` already read the recorded path; recall must too.
    let dir = tmpdir();
    // A fixed user subject (so the conversation NAME never contains the markers)
    // plus a swappable assistant turn that carries the decoy / needle.
    let fix = dir.join("decoy.json");
    std::fs::write(
        &fix,
        r#"{
  "ir_version": "transession/v1",
  "metadata": { "session_id": "recall-divergence-0001", "source_format": "codex", "cwd": "/tmp/constant-cli-proj" },
  "events": [
    { "kind": "message", "role": "user", "blocks": [ { "kind": "text", "text": "recall divergence subject" } ] },
    { "kind": "message", "role": "assistant", "blocks": [ { "kind": "text", "text": "DECOY_at_reconstructed_path" } ] }
  ]
}"#,
    )
    .unwrap();

    let c = run(
        &dir,
        &["carry", "--to", "claude", "--session", fix.to_str().unwrap(), "--json"],
    );
    assert!(c.status.success(), "{}", err(&c));
    let cj: serde_json::Value =
        serde_json::from_str(&out(&c)).expect("carry --json emitted invalid JSON");
    let handle = cj["handle"].as_str().expect("carry has no handle").to_string();
    let recon = PathBuf::from(cj["snapshot"].as_str().expect("carry recorded no snapshot"));
    assert!(recon.exists(), "the volume at the reconstructed path was not written");

    // The volume the ledger will actually point at: same IR shape, distinct
    // content (NEEDLE), at a NON-conventional filename beside the decoy.
    let ledger_vol = recon.with_file_name("ledger-pointed-volume.json");
    let needle = std::fs::read_to_string(&recon)
        .expect("read the volume")
        .replace("DECOY_at_reconstructed_path", "NEEDLE_in_ledger_snapshot");
    assert!(needle.contains("NEEDLE_in_ledger_snapshot"), "marker not in the volume");
    std::fs::write(&ledger_vol, &needle).expect("write ledger-pointed volume");

    // Repoint the ledger's carry row at the non-conventional volume (the decoy
    // stays at the conventional reconstructed path). Capture the chapter number.
    let trail = dir.join(".constant").join("trail.jsonl");
    let recon_str = recon.to_string_lossy().to_string();
    let mut chapter = String::from("ch01");
    let mut repointed = false;
    let mut rewritten = String::new();
    for line in std::fs::read_to_string(&trail).expect("read ledger").lines() {
        if line.trim().is_empty() {
            continue;
        }
        let mut row: serde_json::Value = serde_json::from_str(line).expect("ledger row is JSON");
        if row.get("snapshot").and_then(|s| s.as_str()) == Some(recon_str.as_str()) {
            if let Some(n) = row.get("n").and_then(serde_json::Value::as_u64) {
                chapter = format!("ch{n:02}");
            }
            row["snapshot"] = serde_json::Value::String(ledger_vol.to_string_lossy().into_owned());
            repointed = true;
        }
        rewritten.push_str(&row.to_string());
        rewritten.push('\n');
    }
    assert!(repointed, "did not find the carry row to repoint in the ledger");
    std::fs::write(&trail, rewritten).expect("rewrite ledger");

    let r = run(&dir, &["recall", &handle, &chapter]);
    assert!(r.status.success(), "recall failed: {}", err(&r));
    let text = out(&r);
    assert!(
        text.contains("NEEDLE_in_ledger_snapshot"),
        "recall ignored the ledger's snapshot and read the reconstructed path; got:\n{text}"
    );
    assert!(
        !text.contains("DECOY_at_reconstructed_path"),
        "recall read the decoy at the reconstructed path instead of the ledger's volume; got:\n{text}"
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
        raw_out.contains("ch01"),
        "trail lost original event: {raw_out}"
    );
    assert!(
        !raw_out.contains("ch02"),
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
        current_out.contains("no live projections"),
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

#[test]
fn ps_json_is_valid_and_read_only() {
    let dir = tmpdir();
    let o = run(&dir, &["ps", "--json"]);
    assert!(o.status.success(), "ps failed: {}", err(&o));
    let v: serde_json::Value =
        serde_json::from_str(&out(&o)).expect("ps emitted invalid JSON");
    assert!(v.is_array());
    // Census must never create Constant state.
    assert!(
        !dir.join(".constant").exists(),
        "ps wrote state"
    );
}

// --- pack & carry: the conversation crosses machines ---

#[test]
fn pack_and_unpack_moves_a_conversation_between_machines() {
    // "Machine one": carry a conversation so it has a ledger + record volume.
    let dir1 = tmpdir();
    let fix = ir_fixture(&dir1);
    let c = run(
        &dir1,
        &["carry", "--to", "claude", "--session", fix.to_str().unwrap(), "--json"],
    );
    assert!(c.status.success(), "{}", err(&c));
    let cv: serde_json::Value = serde_json::from_str(&out(&c)).unwrap();
    let handle = cv["handle"].as_str().expect("carry reported no handle").to_string();

    // Pack it.
    let pack_file = dir1.join("travel.constant-pack.json");
    let p = run(
        &dir1,
        &["pack", &handle, "--out", pack_file.to_str().unwrap()],
    );
    assert!(p.status.success(), "pack failed: {}", err(&p));
    assert!(pack_file.exists());
    assert!(out(&p).contains(&handle), "{}", out(&p));

    // Refuses to overwrite an existing pack.
    let again = run(
        &dir1,
        &["pack", &handle, "--out", pack_file.to_str().unwrap()],
    );
    assert!(!again.status.success(), "pack overwrote a file");

    // "Machine two": fresh HOME, nothing but the pack file.
    let dir2 = tmpdir();
    let u = run(&dir2, &["unpack", pack_file.to_str().unwrap()]);
    assert!(u.status.success(), "unpack failed: {}", err(&u));
    let utext = out(&u);
    assert!(utext.contains(&handle), "handle lost in transit: {utext}");
    assert!(utext.contains("constant resume"), "{utext}");

    // The record arrived: volumes listed ok from the LOCAL vault.
    let snaps = run(&dir2, &["snapshots", "--all"]);
    assert!(snaps.status.success(), "{}", err(&snaps));
    let stext = out(&snaps);
    assert!(stext.contains("ch01"), "volume missing on machine two: {stext}");
    assert!(stext.contains("ok"), "{stext}");

    // And the conversation WAKES on machine two: no native projection exists
    // here, so resume reprints from the record (the lost-record doctrine,
    // now doing cross-machine duty).
    let r = run(&dir2, &["resume", &handle, "--all"]);
    assert!(r.status.success(), "resume on machine two failed: {}", err(&r));
    let rtext = out(&r);
    assert!(
        rtext.contains("restored from the record"),
        "no record restore: {rtext}"
    );

    // Idempotent: unpacking again adds nothing.
    let u2 = run(&dir2, &["unpack", pack_file.to_str().unwrap()]);
    assert!(u2.status.success(), "{}", err(&u2));
    assert!(
        out(&u2).contains("0 rows added") || out(&u2).contains("rows added, "),
        "{}",
        out(&u2)
    );
    let u2text = out(&u2);
    assert!(u2text.contains("0 rows added"), "not idempotent: {u2text}");
}

// --- integrity: adversarial inputs, durability, and trust boundaries ---

#[test]
fn hostile_session_id_cannot_escape_the_record_vault() {
    let dir = tmpdir();
    let fix = ir_fixture_named(
        &dir,
        "evil.json",
        "../../../../tmp/constant-escape-attempt",
        "trying to escape",
    );
    let o = run(
        &dir,
        &["carry", "--to", "claude", "--session", fix.to_str().unwrap(), "--json"],
    );
    assert!(o.status.success(), "{}", err(&o));
    let v: serde_json::Value = serde_json::from_str(&out(&o)).unwrap();
    let snap = v["snapshot"].as_str().expect("no snapshot");
    let vault = dir.join(".constant").join("snapshots");
    assert!(
        std::path::Path::new(snap).starts_with(&vault),
        "record escaped the vault: {snap}"
    );
    assert!(!snap.contains(".."), "traversal survived: {snap}");
    assert!(
        !std::path::Path::new("/tmp/constant-escape-attempt").exists(),
        "hostile id wrote outside the vault"
    );
}

#[test]
fn ansi_in_conversation_never_reaches_the_terminal() {
    let dir = tmpdir();
    // Title-bar set + color + BEL embedded in the first user message: every
    // human-output path must neutralize them.
    let fix = ir_fixture_named(
        &dir,
        "ansi.json",
        "ansi-0001",
        r"\u001b]0;own-your-title\u0007 hello \u001b[31mred\u001b[0m",
    );
    let dry = run(
        &dir,
        &["carry", "--to", "claude", "--session", fix.to_str().unwrap(), "--dry-run"],
    );
    assert!(dry.status.success(), "{}", err(&dry));
    assert!(
        !out(&dry).contains('\u{1b}') && !out(&dry).contains('\u{7}'),
        "dry-run leaked control bytes: {:?}",
        out(&dry)
    );

    let c = run(
        &dir,
        &["carry", "--to", "claude", "--session", fix.to_str().unwrap()],
    );
    assert!(c.status.success(), "{}", err(&c));
    for cmd in [vec!["trail", "--all"], vec!["sessions", "--all", "--titles"]] {
        let o = run(&dir, &cmd);
        assert!(o.status.success(), "{}", err(&o));
        assert!(
            !out(&o).contains('\u{1b}') && !out(&o).contains('\u{7}'),
            "{cmd:?} leaked control bytes: {:?}",
            out(&o)
        );
    }
}

#[test]
fn secrets_are_redacted_in_projection_and_record_alike() {
    let dir = tmpdir();
    let fix = ir_fixture_named(
        &dir,
        "leaky.json",
        "leaky-0001",
        "my key is sk-PLAINLEAK1234567890abcd please remember it",
    );
    let o = run(
        &dir,
        &["carry", "--to", "claude", "--session", fix.to_str().unwrap(), "--json"],
    );
    assert!(o.status.success(), "{}", err(&o));
    let v: serde_json::Value = serde_json::from_str(&out(&o)).unwrap();

    let projection = find_claude_projection(&dir, v["id"].as_str().unwrap())
        .expect("projection missing");
    let ptext = std::fs::read_to_string(projection).unwrap();
    assert!(!ptext.contains("sk-PLAINLEAK"), "projection leaked the secret");

    let stext = std::fs::read_to_string(v["snapshot"].as_str().unwrap()).unwrap();
    assert!(!stext.contains("sk-PLAINLEAK"), "record leaked the secret");
    assert_eq!(v["receipt"]["redactions"].as_u64(), Some(1), "{v}");
}

#[test]
#[cfg(unix)]
fn the_record_vault_is_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tmpdir();
    let fix = ir_fixture(&dir);
    let o = run(
        &dir,
        &["carry", "--to", "claude", "--session", fix.to_str().unwrap()],
    );
    assert!(o.status.success(), "{}", err(&o));
    for p in [dir.join(".constant"), dir.join(".constant").join("snapshots")] {
        let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "{} is not owner-only: {mode:o}", p.display());
    }
}

#[test]
fn corrupted_ledger_lines_are_tolerated() {
    let dir = tmpdir();
    let fix = ir_fixture(&dir);
    let first = run(
        &dir,
        &["carry", "--to", "claude", "--session", fix.to_str().unwrap(), "--json"],
    );
    assert!(first.status.success(), "{}", err(&first));
    let fv: serde_json::Value = serde_json::from_str(&out(&first)).unwrap();

    // Garbage and half-written lines in the ledger must not break anything.
    let ledger = dir.join(".constant").join("trail.jsonl");
    let mut text = std::fs::read_to_string(&ledger).unwrap();
    text.push_str("this is not json at all\n{\"ts\": 12, \"half\nnull\n");
    std::fs::write(&ledger, text).unwrap();

    let t = run(&dir, &["trail", "--all"]);
    assert!(t.status.success(), "trail broke on corrupt ledger: {}", err(&t));

    let second = run(
        &dir,
        &["carry", "--to", "claude", "--session", fix.to_str().unwrap(), "--json", "--debug"],
    );
    assert!(second.status.success(), "{}", err(&second));
    let sv: serde_json::Value = serde_json::from_str(&out(&second)).unwrap();
    assert_eq!(
        sv["id"].as_str(),
        fv["id"].as_str(),
        "stable pair lost after ledger corruption"
    );
}

#[test]
fn blocked_record_warns_but_never_blocks_the_carry() {
    let dir = tmpdir();
    let fix = ir_fixture(&dir);
    // Sabotage: the snapshots location is a FILE, so no volume can be written.
    std::fs::create_dir_all(dir.join(".constant")).unwrap();
    std::fs::write(dir.join(".constant").join("snapshots"), "in the way").unwrap();

    let o = run(
        &dir,
        &["carry", "--to", "claude", "--session", fix.to_str().unwrap(), "--json"],
    );
    assert!(o.status.success(), "carry must proceed without the record: {}", err(&o));
    assert!(
        err(&o).contains("record not written"),
        "the gap must be announced: {}",
        err(&o)
    );
    let v: serde_json::Value = serde_json::from_str(&out(&o)).unwrap();
    assert!(v["snapshot"].is_null(), "{v}");
}

#[test]
fn codex_discovery_picks_the_right_cwd_not_the_newest_file() {
    let dir = tmpdir();
    let proj = dir.join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    let proj_str = proj.canonicalize().unwrap().display().to_string();

    let day = dir.join("codex").join("sessions").join("2026").join("06").join("01");
    std::fs::create_dir_all(&day).unwrap();
    let mk = |name: &str, id: &str, cwd: &str, text: &str| {
        let body = format!(
            "{{\"timestamp\":\"2026-06-01T10:00:00.000Z\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"{id}\",\"timestamp\":\"2026-06-01T10:00:00.000Z\",\"cwd\":\"{cwd}\",\"originator\":\"codex-tui\",\"cli_version\":\"0.137.0\",\"source\":\"cli\"}}}}\n{{\"timestamp\":\"2026-06-01T10:00:01.000Z\",\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"user\",\"content\":[{{\"type\":\"input_text\",\"text\":\"{text}\"}}]}}}}\n"
        );
        std::fs::write(day.join(name), body).unwrap();
    };
    // Right cwd, written FIRST (older mtime).
    mk(
        "rollout-2026-06-01T10-00-00-01970000-0000-7000-8000-00000000000b.jsonl",
        "01970000-0000-7000-8000-00000000000b",
        &proj_str,
        "pick me from the right cwd",
    );
    std::thread::sleep(std::time::Duration::from_millis(1100));
    // Wrong cwd, NEWER — newest-mtime alone would wrongly pick this one.
    mk(
        "rollout-2026-06-01T10-00-02-01970000-0000-7000-8000-00000000000a.jsonl",
        "01970000-0000-7000-8000-00000000000a",
        "/somewhere/else",
        "do not pick me",
    );

    let o = run_in(
        &dir,
        &proj,
        &["carry", "--from", "codex", "--to", "claude", "--dry-run", "--json"],
    );
    assert!(o.status.success(), "{}", err(&o));
    let v: serde_json::Value = serde_json::from_str(&out(&o)).unwrap();
    assert_eq!(
        v["root"].as_str(),
        Some("pick me from the right cwd"),
        "cwd scoping failed: {v}"
    );
}

#[test]
fn ambiguous_session_id_across_stores_is_refused() {
    let dir = tmpdir();
    let id = "99999999-9999-9999-9999-999999999999";
    let day = dir.join("codex").join("sessions").join("2026").join("06").join("01");
    std::fs::create_dir_all(&day).unwrap();
    std::fs::write(day.join(format!("rollout-2026-06-01T10-00-00-{id}.jsonl")), "x").unwrap();
    let proj = dir.join("claude").join("projects").join("-tmp-p");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(proj.join(format!("{id}.jsonl")), "x").unwrap();

    let o = run(&dir, &["carry", "--to", "claude", "--session", id]);
    assert!(!o.status.success(), "ambiguous id must be refused");
    assert!(
        err(&o).contains("more than one"),
        "wrong error: {}",
        err(&o)
    );
}

#[test]
fn carrying_into_gemini_is_refused_with_a_clear_error() {
    let dir = tmpdir();
    let fix = ir_fixture(&dir);
    let o = run(
        &dir,
        &["carry", "--to", "gemini", "--session", fix.to_str().unwrap()],
    );
    assert!(!o.status.success(), "gemini target must be gated");
    assert!(
        err(&o).contains("isn't supported yet"),
        "wrong error: {}",
        err(&o)
    );
}

#[test]
fn restore_from_nonexistent_source_errors_cleanly() {
    let dir = tmpdir();
    let o = run(&dir, &["restore", "/nope/does-not-exist.json"]);
    assert!(!o.status.success());
    assert!(
        err(&o).contains("could not resolve") || err(&o).contains("Error"),
        "{}",
        err(&o)
    );
}

// --- third runtime: gemini + opencode sources ---

fn gemini_fixture(dir: &Path) -> PathBuf {
    // The real gemini session shape (verified on disk): whole-file JSON,
    // user content = block array, gemini content = string, tools + thoughts.
    let path = dir.join("gemini-session.json");
    let ir = r#"{
  "sessionId": "9d2f7c30-aaaa-bbbb-cccc-1234567890ab",
  "projectHash": "cf84c1cae18c9e9c3a83a4e9e6dfa0af71650ac22d7c58cf89f0c76bb2c8425b",
  "startTime": "2026-06-01T10:00:00.000Z",
  "lastUpdated": "2026-06-01T10:05:00.000Z",
  "kind": "main",
  "messages": [
    { "id": "m1", "timestamp": "2026-06-01T10:00:01.000Z", "type": "user",
      "content": [ { "text": "hello from gemini land" } ] },
    { "id": "m2", "timestamp": "2026-06-01T10:00:05.000Z", "type": "gemini",
      "content": "greetings, carried one",
      "thoughts": [ { "subject": "Pondering", "description": "what to say" } ],
      "toolCalls": [ { "id": "t1", "name": "run_shell_command",
                        "args": { "command": "echo sk-GEMSECRET1234567890ab" },
                        "result": [ { "ok": true } ] } ],
      "tokens": { "input": 10, "output": 5, "total": 15 },
      "model": "gemini-3-pro" },
    { "id": "m3", "timestamp": "2026-06-01T10:01:00.000Z", "type": "info",
      "content": "Update successful!" }
  ]
}"#;
    std::fs::write(&path, ir).unwrap();
    path
}

#[test]
fn gemini_session_carries_into_claude() {
    let dir = tmpdir();
    let fix = gemini_fixture(&dir);
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
    assert!(o.status.success(), "gemini carry failed: {}", err(&o));
    let v: serde_json::Value = serde_json::from_str(&out(&o)).unwrap();
    assert_eq!(v["from"].as_str(), Some("gemini"), "{v}");
    assert_eq!(v["receipt"]["turns"].as_u64(), Some(2), "{v}");
    // info message is scaffold; tools dropped by default; thoughts are reasoning.
    assert_eq!(v["receipt"]["dropped_tools"].as_u64(), Some(2), "{v}");
    assert_eq!(v["receipt"]["dropped_reasoning"].as_u64(), Some(1), "{v}");
    assert_eq!(before, std::fs::read(&fix).unwrap(), "source modified");

    // with tools: kept + payload secret redacted in the materialized session.
    let w = run(
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
    assert!(w.status.success(), "{}", err(&w));
    let wv: serde_json::Value = serde_json::from_str(&out(&w)).unwrap();
    assert_eq!(wv["receipt"]["tools"].as_u64(), Some(2), "{wv}");
    let snap = wv["snapshot"].as_str().unwrap();
    let snap_text = std::fs::read_to_string(snap).unwrap();
    assert!(
        !snap_text.contains("sk-GEMSECRET"),
        "gemini tool secret leaked into the record"
    );
}

fn opencode_export_fixture(dir: &Path) -> PathBuf {
    // The export shape `opencode export` emits (with its trailing status line,
    // which the loader must tolerate).
    let path = dir.join("opencode-export.json");
    let body = r#"{
  "info": {
    "id": "ses_0123456789abcdefSOURCE01",
    "slug": "test-session",
    "projectID": "global",
    "directory": "/tmp/constant-cli-proj",
    "title": "opencode test session",
    "version": "1.14.48",
    "summary": {"additions": 0, "deletions": 0, "files": 0},
    "time": {"created": 1778707096618, "updated": 1778707193869}
  },
  "messages": [
    { "info": { "id": "msg_a", "role": "user", "time": {"created": 1778707096634},
                "summary": {"diffs": []}, "agent": "build",
                "model": {"providerID": "test", "modelID": "test-model"} },
      "parts": [ { "type": "text", "text": "hello from opencode" } ] },
    { "info": { "id": "msg_b", "role": "assistant",
                "time": {"created": 1778707097000, "completed": 1778707099000},
                "modelID": "test-model", "providerID": "test", "mode": "build",
                "agent": "build", "path": {"cwd": "/tmp/constant-cli-proj", "root": "/tmp/constant-cli-proj"},
                "cost": 0, "tokens": {"input": 1, "output": 1, "reasoning": 0, "cache": {"read": 0, "write": 0}} },
      "parts": [
        { "type": "step-start", "id": "prt_1", "sessionID": "ses_0123456789abcdefSOURCE01", "messageID": "msg_b" },
        { "type": "reasoning", "text": "thinking it over", "id": "prt_2" },
        { "type": "tool", "tool": "bash", "callID": "call_1", "id": "prt_3",
          "state": {"status": "completed", "input": {"command": "ls"}, "output": "files: token: sk-OCSECRET1234567890ab"} },
        { "type": "text", "text": "answered from opencode", "id": "prt_4" }
      ] }
  ]
}
Exporting session: ses_0123456789abcdefSOURCE01"#;
    std::fs::write(&path, body).unwrap();
    path
}

#[test]
fn opencode_export_file_carries_into_codex() {
    let dir = tmpdir();
    let fix = opencode_export_fixture(&dir);
    let o = run(
        &dir,
        &[
            "carry",
            "--to",
            "codex",
            "--session",
            fix.to_str().unwrap(),
            "--json",
        ],
    );
    assert!(o.status.success(), "opencode carry failed: {}", err(&o));
    let v: serde_json::Value = serde_json::from_str(&out(&o)).unwrap();
    assert_eq!(v["from"].as_str(), Some("opencode"), "{v}");
    assert_eq!(v["receipt"]["turns"].as_u64(), Some(2), "{v}");
    assert_eq!(v["receipt"]["dropped_tools"].as_u64(), Some(2), "{v}");
    assert_eq!(v["receipt"]["dropped_reasoning"].as_u64(), Some(1), "{v}");
}

/// Full round trip THROUGH the real opencode binary: carry an IR fixture INTO
/// opencode (writer = export-shaped JSON + `opencode import`), then read it
/// back out via `opencode export`. Skips when opencode isn't installed (CI).
#[test]
fn carry_into_opencode_round_trips_through_import() {
    if std::process::Command::new("opencode")
        .arg("--version")
        .output()
        .map(|o| !o.status.success())
        .unwrap_or(true)
    {
        eprintln!("skipping: opencode binary not available");
        return;
    }

    let dir = tmpdir();
    let fix = ir_fixture(&dir);
    let o = run(
        &dir,
        &[
            "carry",
            "--to",
            "opencode",
            "--session",
            fix.to_str().unwrap(),
            "--json",
        ],
    );
    assert!(o.status.success(), "carry to opencode failed: {}", err(&o));
    let v: serde_json::Value = serde_json::from_str(&out(&o)).unwrap();
    let id = v["id"].as_str().expect("no id");
    assert!(id.starts_with("ses_"), "unexpected opencode id: {id}");
    assert!(
        v["resume"].as_str().unwrap_or("").contains("opencode -s"),
        "{v}"
    );

    // The isolated opencode store (XDG_DATA_HOME) must now hold the session.
    let export = std::process::Command::new("opencode")
        .args(["export", id])
        .env("XDG_DATA_HOME", dir.join("xdg"))
        .output()
        .expect("run opencode export");
    assert!(export.status.success(), "export-back failed");
    let text = String::from_utf8_lossy(&export.stdout).to_string();
    assert!(
        text.contains("hello from the fixture"),
        "round trip lost the conversation: {text}"
    );

    // Refresh (stable pair): same id again, upserted through their door.
    let again = run(
        &dir,
        &[
            "carry",
            "--to",
            "opencode",
            "--session",
            fix.to_str().unwrap(),
            "--json",
        ],
    );
    assert!(again.status.success(), "{}", err(&again));
    let av: serde_json::Value = serde_json::from_str(&out(&again)).unwrap();
    assert_eq!(av["id"].as_str(), Some(id), "stable pair broke");
    assert_eq!(av["debug"].get("mode"), None); // not in non-debug json
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

    // Name query selects the same conversation; unknown query fails.
    let q = run(&dir, &["resume", "fixture", "--all"]);
    assert!(q.status.success(), "{}", err(&q));
    assert!(out(&q).contains(&format!("claude -r {id}")));

    let bad = run(&dir, &["resume", "zzz-nope", "--all"]);
    assert!(!bad.status.success(), "unknown query should fail");
    assert!(err(&bad).contains("no conversation matches"), "{}", err(&bad));

    // --list shows the conversation and the command to resume it.
    let ls = run(&dir, &["resume", "--list", "--all"]);
    assert!(ls.status.success(), "{}", err(&ls));
    assert!(out(&ls).contains("from-the-fixture"), "{}", out(&ls));
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
        listed.contains("from-the-fixture") && listed.contains("goodbye-fixture-two"),
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
    assert!(ls_out.contains("ch01"), "listing missing the volume: {ls_out}");
    assert!(ls_out.contains("ok"), "volume not marked ok: {ls_out}");
    assert!(
        ls_out.contains("constant restore"),
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
        out(&events).contains("ch02"),
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

/// A long IR fixture: `pairs` user/assistant exchanges, each turn padded so a
/// paged render must file most of the conversation.
fn ir_fixture_long(dir: &Path, id: &str, pairs: usize) -> PathBuf {
    let path = dir.join(format!("{id}.json"));
    let pad = "lorem ".repeat(220); // ~1.3k chars per turn
    let mut events = String::new();
    for i in 0..pairs {
        if i > 0 {
            events.push_str(",\n");
        }
        events.push_str(&format!(
            r#"    {{ "kind": "message", "role": "user",
      "blocks": [ {{ "kind": "text", "text": "question {i} {pad}" }} ] }},
    {{ "kind": "message", "role": "assistant",
      "blocks": [ {{ "kind": "text", "text": "answer {i} {pad}" }} ] }}"#
        ));
    }
    let ir = format!(
        r#"{{
  "ir_version": "transession/v1",
  "metadata": {{ "session_id": "{id}", "source_format": "codex", "cwd": "/tmp/constant-cli-proj" }},
  "events": [
{events}
  ]
}}"#
    );
    std::fs::write(&path, ir).unwrap();
    path
}

/// Move 1+2 round trip: a paged carry files old turns behind addresses, the
/// projection wakes on head card + index + verbatim tail, and `constant
/// recall` resolves any filed address back to the exact original words.
#[test]
fn paged_render_files_turns_and_recall_reads_them_back() {
    let dir = tmpdir();
    let src = ir_fixture_long(&dir, "paged-0001", 20); // ~52k chars: must file

    let o = run(
        &dir,
        &[
            "carry", "--session", src.to_str().unwrap(), "--to", "claude",
            "--render", "paged", "--json",
        ],
    );
    assert!(o.status.success(), "paged carry failed: {}", err(&o));
    let v: serde_json::Value = serde_json::from_str(&out(&o)).unwrap();
    let handle = v["handle"].as_str().unwrap().to_string();
    let indexed = v["receipt"]["indexed"].as_u64().unwrap();
    assert!(indexed > 0, "long conversation was not filed: {v}");

    // The projection: head card + index + verbatim tail, older turns ABSENT.
    let projection = std::fs::read_to_string(v["path"].as_str().unwrap()).unwrap();
    assert!(projection.contains("[constant: taking over]"), "no head card");
    assert!(projection.contains("index of filed turns"), "no index");
    assert!(
        projection.contains(&format!("constant recall {handle}")),
        "head card does not teach recall"
    );
    assert!(projection.contains("answer 19"), "tail not verbatim");
    let full_first_turn = format!("question 0 {}", "lorem ".repeat(220));
    assert!(
        !projection.contains(full_first_turn.trim_end()),
        "filed turn leaked verbatim into the projection"
    );

    // The record still holds the FULL thread (indexed is never lost)...
    let record = std::fs::read_to_string(v["snapshot"].as_str().unwrap()).unwrap();
    assert!(record.contains("question 0"), "record lost a filed turn");

    // ...and recall resolves a filed address to the exact words.
    let o = run(&dir, &["recall", &handle, "1-2"]);
    assert!(o.status.success(), "recall failed: {}", err(&o));
    let text = out(&o);
    assert!(text.contains("question 0"), "recall missing turn 1: {text}");
    assert!(text.contains("answer 0"), "recall missing turn 2");
    assert!(text.contains("\u{b7}1] user:"), "recall lost addressing: {text}");

    // Single-turn recall stays scoped.
    let o = run(&dir, &["recall", &handle, "3"]);
    assert!(o.status.success());
    let text = out(&o);
    assert!(text.contains("question 1"));
    assert!(!text.contains("answer 1\n"), "range leaked: {text}");
}

/// The paged view's desk furniture is scaffold: a RE-carry of a paged
/// projection strips the head card and index instead of carrying them
/// forward as fake user turns.
#[test]
fn paged_desk_furniture_self_cleans_on_recarry() {
    let dir = tmpdir();
    let src = ir_fixture_long(&dir, "paged-0002", 20);

    let o = run(
        &dir,
        &[
            "carry", "--session", src.to_str().unwrap(), "--to", "claude",
            "--render", "paged", "--json",
        ],
    );
    assert!(o.status.success(), "{}", err(&o));
    let v: serde_json::Value = serde_json::from_str(&out(&o)).unwrap();
    let projection = v["path"].as_str().unwrap().to_string();

    // Re-carry the paged projection (full render this time).
    let o = run(
        &dir,
        &["carry", "--session", &projection, "--to", "codex", "--json"],
    );
    assert!(o.status.success(), "re-carry failed: {}", err(&o));
    let v2: serde_json::Value = serde_json::from_str(&out(&o)).unwrap();
    let reprojection = std::fs::read_to_string(v2["path"].as_str().unwrap()).unwrap();
    assert!(
        !reprojection.contains("[constant: taking over]"),
        "head card carried forward as a conversation turn"
    );
    assert!(
        !reprojection.contains("index of filed turns"),
        "index carried forward as a conversation turn"
    );
    assert!(
        v2["receipt"]["dropped_scaffold"].as_u64().unwrap() >= 2,
        "receipt did not declare the stripped desk furniture: {v2}"
    );
}

/// Short conversations never get filed — paged mode degrades to
/// orientation-only, and recall still works against the record.
#[test]
fn paged_render_keeps_short_conversations_whole() {
    let dir = tmpdir();
    let src = ir_fixture(&dir);

    let o = run(
        &dir,
        &[
            "carry", "--session", src.to_str().unwrap(), "--to", "claude",
            "--render", "paged", "--json",
        ],
    );
    assert!(o.status.success(), "{}", err(&o));
    let v: serde_json::Value = serde_json::from_str(&out(&o)).unwrap();
    assert_eq!(v["receipt"]["indexed"].as_u64().unwrap(), 0);
    let projection = std::fs::read_to_string(v["path"].as_str().unwrap()).unwrap();
    assert!(projection.contains("[constant: taking over]"));
    assert!(!projection.contains("index of filed turns"));
    assert!(projection.contains("hello from the fixture"), "turns must stay verbatim");
}
