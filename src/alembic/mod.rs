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

use anyhow::{bail, Context, Result};
use uuid::Uuid;

use crate::runtime::Runtime;
use ir::{SessionEvent, SessionFormat, SourceFormat, UniversalSession};

/// Distill the most recent `from` session into `to`'s native store — sanitized,
/// rekeyed, and registered for native resume. Returns (new_session_id, cwd).
pub fn distill(from: Runtime, to: Runtime) -> Result<(String, Option<PathBuf>)> {
    let from_fmt = session_format(from);
    let from_root = formats::default_output_root(from_fmt)?;
    let search = from_root.join(match from_fmt {
        SessionFormat::Codex => "sessions",
        SessionFormat::Claude => "projects",
        SessionFormat::Ir => bail!("unsupported source"),
    });
    let src = latest_jsonl(&search).context("no recent session found to carry")?;
    distill_path(&src, to)
}

/// Distill a specific source session file into `to`'s native store.
pub fn distill_path(src: &Path, to: Runtime) -> Result<(String, Option<PathBuf>)> {
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

    // Rekey so we never collide with or overwrite the source session.
    let new_id = match to_fmt {
        SessionFormat::Claude => Uuid::new_v4().to_string(),
        SessionFormat::Codex => Uuid::now_v7().to_string(),
        SessionFormat::Ir => bail!("unsupported target"),
    };
    session.metadata.session_id = new_id.clone();

    let out_root = formats::default_output_root(to_fmt)?;
    formats::materialize(&session, to_fmt, &out_root)
        .with_context(|| format!("failed to write {} session", to.label()))?;

    Ok((new_id, session.metadata.cwd.clone()))
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

/// Newest `*.jsonl` under `root` by modification time.
fn latest_jsonl(root: &Path) -> Option<PathBuf> {
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                if let Ok(mtime) = entry.metadata().and_then(|m| m.modified()) {
                    if best.as_ref().map(|(t, _)| mtime > *t).unwrap_or(true) {
                        best = Some((mtime, path));
                    }
                }
            }
        }
    }
    best.map(|(_, p)| p)
}
