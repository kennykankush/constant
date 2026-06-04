//! Alembic — Constant's conversation still.
//!
//! It takes the most recent session from one runtime, distills it down to the
//! pure conversation (runtime scaffold stripped, secrets burned off, tool/
//! reasoning noise removed), transmutes it into the target runtime's native
//! session format, and registers it so the target can resume it natively.
//!
//! The low-level format codecs in `formats/` and the neutral IR in `ir.rs` are
//! vendored from transession (MIT, https://github.com/inmzhang/transession — see
//! LICENSE.transession). Alembic's contribution is the `distill` sanitize pass:
//! transession faithfully shovels everything across; Alembic carries only the
//! essence.

pub mod formats;
pub mod ir;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use uuid::Uuid;

use crate::runtime::Runtime;
use ir::{SessionEvent, SessionFormat, SourceFormat, UniversalSession};

/// Distill the conversation a runtime is CURRENTLY in — the newest matching
/// rollout in `host_cwd` (the one it's actively writing) — into `to`'s native
/// store. Using "currently active" (newest write) instead of capturing an id at
/// launch means it follows the user even if they `/resume` a different
/// conversation inside the child. Returns (new_session_id, cwd).
pub fn distill(
    from: Runtime,
    to: Runtime,
    host_cwd: Option<&Path>,
) -> Result<(String, Option<PathBuf>)> {
    let from_fmt = session_format(from);
    let src = find_child_session(from_fmt, host_cwd)
        .context("no conversation found in this directory to carry")?;
    let (id, _path, cwd) = distill_path(&src, to, None)?;
    Ok((id, cwd))
}

/// The session a runtime is currently in (the newest matching rollout it's
/// writing), as (file, session id).
pub fn active_session(from: Runtime, host_cwd: Option<&Path>) -> Option<(PathBuf, String)> {
    let from_fmt = session_format(from);
    let path = find_child_session(from_fmt, host_cwd)?;
    let id = session_id_of(&path, from_fmt)?;
    Some((path, id))
}

/// Distill a specific source session file into `to`'s native store. If
/// `target_id` is given, that session is OVERWRITTEN (so a switch syncs back
/// into this thread's existing counterpart rather than minting a new session
/// every time); otherwise a fresh id is minted. Returns (id, written_file, cwd).
pub fn distill_path(
    src: &Path,
    to: Runtime,
    reuse: Option<(&str, &Path)>,
) -> Result<(String, PathBuf, Option<PathBuf>)> {
    let to_fmt = session_format(to);

    let mut session = formats::load_session(src, SourceFormat::Auto)
        .with_context(|| format!("failed to read {}", src.display()))?;

    sanitize(&mut session);
    if !session
        .events
        .iter()
        .any(|e| matches!(e, SessionEvent::Message(_)))
    {
        bail!("no conversation to carry yet");
    }

    let id = match reuse {
        Some((id, _)) => id.to_string(),
        None => match to_fmt {
            SessionFormat::Claude => Uuid::new_v4().to_string(),
            SessionFormat::Codex => Uuid::now_v7().to_string(),
            SessionFormat::Ir => bail!("unsupported target"),
        },
    };
    session.metadata.session_id = id.clone();

    // Stamp the target's real CLI version into session_meta so neither resume
    // rejects it (claude) nor codex's /resume backfill treats it as foreign.
    session.metadata.platform_version = Some(match to {
        Runtime::Claude => detect_claude_version().unwrap_or_else(|| "2.1.154".to_string()),
        Runtime::Codex => detect_codex_version().unwrap_or_else(|| "0.137.0".to_string()),
    });

    // Reuse → overwrite the SAME file in place. This is the fix for the "fork on
    // disk" bug: codex names rollouts `rollout-<ts>-<id>.jsonl`, so writing to
    // the home root each sync produced a SECOND file with the same id and a new
    // timestamp, and `/resume` then loaded the wrong (original) one. Writing
    // straight to the existing file keeps exactly one file per id.
    let written = match reuse {
        Some((_, path)) => formats::materialize(&session, to_fmt, path)
            .with_context(|| format!("failed to overwrite {} session", to.label()))?,
        None => {
            let out_root = formats::default_output_root(to_fmt)?;
            formats::materialize(&session, to_fmt, &out_root)
                .with_context(|| format!("failed to write {} session", to.label()))?
        }
    };

    // Make codex sessions show up in `codex /resume`: its picker filters on
    // has_user_event=1 + native source, so stamp our row to look native.
    if to_fmt == SessionFormat::Codex {
        let cwd_str = session
            .metadata
            .cwd
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| ".".to_string());
        let title = first_user_text(&session).unwrap_or_else(|| "carried conversation".to_string());
        let _ = upsert_codex_thread(&id, &written, &cwd_str, &title);
    }

    Ok((id, written, session.metadata.cwd.clone()))
}

/// First user message text (for the codex thread title/preview).
fn first_user_text(session: &UniversalSession) -> Option<String> {
    session.events.iter().find_map(|event| {
        let SessionEvent::Message(message) = event else {
            return None;
        };
        if message.role != "user" {
            return None;
        }
        let text = message
            .blocks
            .iter()
            .filter_map(|b| b.text.as_deref())
            .collect::<Vec<_>>()
            .join(" ");
        let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if text.is_empty() {
            None
        } else {
            Some(text.chars().take(80).collect())
        }
    })
}

/// Detect the installed Codex CLI version, e.g. "0.137.0" from "codex-cli 0.137.0".
fn detect_codex_version() -> Option<String> {
    let output = std::process::Command::new("codex")
        .arg("--version")
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    text.split_whitespace()
        .find(|t| t.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false))
        .map(str::to_string)
}

/// Upsert a codex `threads` row so a synthetic session looks native and appears
/// in `codex /resume` (has_user_event=1, source=cli, real provider/version).
fn upsert_codex_thread(id: &str, rollout_path: &Path, cwd: &str, title: &str) -> Result<()> {
    use rusqlite::{params, Connection};
    let db = formats::default_output_root(SessionFormat::Codex)?.join("state_5.sqlite");
    if !db.exists() {
        return Ok(());
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let version = detect_codex_version().unwrap_or_else(|| "0.137.0".to_string());
    let conn = Connection::open(&db)?;
    conn.execute(
        "INSERT INTO threads
            (id, rollout_path, created_at, updated_at, source, model_provider, cwd, title,
             sandbox_policy, approval_mode, tokens_used, has_user_event, archived,
             cli_version, first_user_message, memory_mode, preview)
         VALUES (?1, ?2, ?3, ?3, 'cli', 'openai', ?4, ?5,
             '{\"type\":\"workspace-write\"}', 'on-request', 0, 1, 0,
             ?6, ?5, 'enabled', ?5)
         ON CONFLICT(id) DO UPDATE SET
             rollout_path = excluded.rollout_path,
             updated_at = excluded.updated_at,
             source = 'cli',
             model_provider = 'openai',
             has_user_event = 1,
             archived = 0,
             cli_version = excluded.cli_version,
             title = excluded.title,
             first_user_message = excluded.first_user_message,
             preview = excluded.preview",
        params![id, rollout_path.display().to_string(), now, cwd, title, version],
    )?;
    Ok(())
}

/// Read a session's own id: Claude = the file stem; Codex = session_meta.id.
fn session_id_of(path: &Path, fmt: SessionFormat) -> Option<String> {
    match fmt {
        SessionFormat::Claude => path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(str::to_string),
        SessionFormat::Codex => {
            use std::io::{BufRead, BufReader};
            let file = fs::File::open(path).ok()?;
            let mut first = String::new();
            BufReader::new(file).read_line(&mut first).ok()?;
            let value: serde_json::Value = serde_json::from_str(first.trim()).ok()?;
            value.get("payload")?.get("id")?.as_str().map(str::to_string)
        }
        SessionFormat::Ir => None,
    }
}

fn session_format(runtime: Runtime) -> SessionFormat {
    match runtime {
        Runtime::Codex => SessionFormat::Codex,
        Runtime::Claude => SessionFormat::Claude,
    }
}

/// Constant's taste: keep only genuine user/assistant conversation, drop runtime
/// scaffold + tool/reasoning noise, and redact secrets. This is the distillation
/// step transession does NOT do — it carries everything, cruft and credentials
/// included (which we saw leak a token into a fresh session).
fn sanitize(session: &mut UniversalSession) {
    let mut kept = Vec::new();
    for event in std::mem::take(&mut session.events) {
        // Drop reasoning, tool calls, and tool results — the agentic layer is
        // lossy across runtimes anyway; we carry the conversation, not the tools.
        let SessionEvent::Message(mut message) = event else {
            continue;
        };
        // Drop developer/system scaffold messages outright.
        if message.role != "user" && message.role != "assistant" {
            continue;
        }
        for block in &mut message.blocks {
            if let Some(text) = &block.text {
                block.text = Some(redact(text));
            }
        }
        let combined: String = message
            .blocks
            .iter()
            .filter_map(|b| b.text.as_deref())
            .collect::<Vec<_>>()
            .join("\n");
        if combined.trim().is_empty() {
            continue;
        }
        // Drop user turns that are pure runtime scaffold (injected env, perms,
        // skills, memory summaries, system reminders).
        if message.role == "user" && is_scaffold(&combined) {
            continue;
        }
        kept.push(SessionEvent::Message(message));
    }
    session.events = kept;
}

fn is_scaffold(text: &str) -> bool {
    let t = text.trim_start();
    const MARKERS: &[&str] = &[
        "<environment_context>",
        "<permissions instructions>",
        "<collaboration_mode>",
        "<apps_instructions>",
        "<skills_instructions>",
        "<plugins_instructions>",
        "<user_instructions>",
        "## Memory",
        "<system-reminder>",
        "<command-name>",
        "<command-message>",
        "Caveat: The messages below",
        "# AGENTS.md",
        "# CLAUDE.md",
        "<INSTRUCTIONS>",
        "Codebase and user instructions",
    ];
    MARKERS.iter().any(|m| t.starts_with(m)) || text.contains("MEMORY_SUMMARY")
}

/// Burn off secrets so we never carry credentials across a runtime boundary.
fn redact(text: &str) -> String {
    use regex::Regex;
    let mut out = text.to_string();
    let patterns: [(&str, &str); 4] = [
        (
            r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}\b",
            "[redacted-email]",
        ),
        (r"\bsk-[A-Za-z0-9_-]{16,}\b", "[redacted-key]"),
        (r"\bgh[pousr]_[A-Za-z0-9]{16,}\b", "[redacted-token]"),
        (r"\bxox[baprs]-[A-Za-z0-9-]{10,}\b", "[redacted-token]"),
    ];
    for (pat, repl) in patterns {
        if let Ok(re) = Regex::new(pat) {
            out = re.replace_all(&out, repl).into_owned();
        }
    }
    if let Ok(re) = Regex::new(
        r"(?i)\b(api[_-]?key|token|secret|password|authorization|bearer)\b(\s*[:=]\s*)\S+",
    ) {
        out = re.replace_all(&out, "$1$2[redacted]").into_owned();
    }
    // Strip runtime-internal noise blocks that ride along in message text.
    if let Ok(re) = Regex::new(r"(?s)\s*<oai-mem-citation>.*?</oai-mem-citation>\s*") {
        out = re.replace_all(&out, "").into_owned();
    }
    out.trim().to_string()
}

/// Detect the installed Claude CLI version (e.g. "2.1.154") for the session
/// `version` field, so resume accepts it as native.
fn detect_claude_version() -> Option<String> {
    let output = std::process::Command::new("claude")
        .arg("--version")
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    text.split_whitespace()
        .next()
        .filter(|t| t.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false))
        .map(str::to_string)
}

/// Find the session this harness is hosting: newest `*.jsonl` under the runtime
/// store whose recorded cwd matches `host_cwd` and whose mtime is at/after the
/// child was spawned (`since`). cwd-scoping is what stops us grabbing an
/// unrelated session from another directory.
fn find_child_session(from: SessionFormat, host_cwd: Option<&Path>) -> Option<PathBuf> {
    let root = formats::default_output_root(from).ok()?;
    let search = root.join(match from {
        SessionFormat::Codex => "sessions",
        SessionFormat::Claude => "projects",
        SessionFormat::Ir => return None,
    });
    let want_slug = host_cwd.map(cwd_slug);

    let mut best: Option<(SystemTime, PathBuf)> = None;
    let mut stack = vec![search];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let Ok(mtime) = entry.metadata().and_then(|m| m.modified()) else {
                continue;
            };
            if !has_conversation(&path, from) {
                continue;
            }
            let cwd_ok = match (from, host_cwd) {
                (_, None) => true,
                (SessionFormat::Codex, Some(c)) => {
                    codex_session_cwd(&path).as_deref() == Some(&*c.to_string_lossy())
                }
                (SessionFormat::Claude, Some(_)) => {
                    path.parent().and_then(|p| p.file_name()).and_then(|n| n.to_str())
                        == want_slug.as_deref()
                }
                (SessionFormat::Ir, _) => false,
            };
            if !cwd_ok {
                continue;
            }
            if best.as_ref().map(|(t, _)| mtime > *t).unwrap_or(true) {
                best = Some((mtime, path));
            }
        }
    }
    best.map(|(_, p)| p)
}

/// Does this session contain an actual user/assistant exchange (vs. a session a
/// fresh launch just opened with no real turns)?
fn has_conversation(path: &Path, from: SessionFormat) -> bool {
    use std::io::{BufRead, BufReader};
    let Ok(file) = fs::File::open(path) else {
        return false;
    };
    let (needle_a, needle_b) = match from {
        SessionFormat::Codex => ("\"role\":\"user\"", "\"role\":\"assistant\""),
        SessionFormat::Claude => ("\"type\":\"user\"", "\"type\":\"assistant\""),
        SessionFormat::Ir => return false,
    };
    BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .any(|line| line.contains(needle_a) || line.contains(needle_b))
}

/// Read a Codex rollout's recorded cwd from its first-line session_meta.
fn codex_session_cwd(path: &Path) -> Option<String> {
    use std::io::{BufRead, BufReader};
    let file = fs::File::open(path).ok()?;
    let mut first = String::new();
    BufReader::new(file).read_line(&mut first).ok()?;
    let value: serde_json::Value = serde_json::from_str(first.trim()).ok()?;
    value
        .get("payload")?
        .get("cwd")?
        .as_str()
        .map(str::to_string)
}

/// Encode a path the way Claude names its `projects/<slug>` directory: every
/// non-alphanumeric character becomes `-`, with a leading `-`.
fn cwd_slug(path: &Path) -> String {
    let rendered = path.to_string_lossy();
    let mut slug = String::with_capacity(rendered.len());
    for ch in rendered.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
        } else {
            slug.push('-');
        }
    }
    if slug.starts_with('-') {
        slug
    } else {
        format!("-{slug}")
    }
}
