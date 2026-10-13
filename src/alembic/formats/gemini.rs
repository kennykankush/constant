//! Gemini CLI session codec — LOADER ONLY for now.
//!
//! Two on-disk shapes, both under `~/.gemini/tmp/<project>/chats/`:
//!
//! - **legacy** (≤ 0.38): one JSON document
//!   `{ sessionId, projectHash, startTime, lastUpdated, kind, messages[] }`.
//! - **log** (0.39+, `session-<ts>-<id8>.jsonl`): a header line
//!   `{ sessionId, projectHash, startTime, lastUpdated, kind }`, then records.
//!   A message record re-appended under the same `id` updates it in place;
//!   `{"$set": {lastUpdated?, messages?}}` patches the session (`messages` =
//!   gemini rebuilt its history, e.g. after compressing it);
//!   `{"$rewindTo": id}` deletes that message and everything after it.
//!
//! Message types: `user` (content = `[{text}|{functionResponse}|{inlineData}]`,
//! or a string), `gemini` (content = string or part array, plus `thoughts`,
//! `toolCalls`, `tokens`, `model`), and `info` (CLI notices — dropped).
//! Gemini's own loader ignores user turns that are empty or start with `/`,
//! `?`, `<session_context>` or `<hook_context>` (its injected preamble), and so
//! does this one.
//!
//! One deliberate difference: when a `$set.messages` rebuild drops turns from
//! gemini's own view, Constant keeps the turns it already read — compression is
//! a context-management artifact, not the user taking words back. A
//! `$rewindTo` IS the user taking words back, and is honored.
//!
//! `projectHash` is `sha256(cwd)`; the cwd is recovered from the project dir's
//! `.project_root` file (named dirs) or by hashing `~/.gemini/projects.json`.
//!
//! The WRITER is deliberately absent: gemini is a carry SOURCE.

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::alembic::ir::{
    ContentBlock, MessageEvent, ReasoningEvent, SessionEvent, SessionFormat, ToolCallEvent,
    ToolResultEvent, UniversalSession,
};
use crate::alembic::sha256;

/// A gemini session folded to its current state: the header fields and the
/// messages in order.
struct GeminiFile {
    header: Value,
    messages: Vec<Value>,
    /// `$set.messages` rebuilds that dropped turns from gemini's own view.
    rebuilds: usize,
}

fn read_file(path: &Path) -> Result<GeminiFile> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("failed to read Gemini session {}", path.display()))?;
    if let Ok(doc) = serde_json::from_str::<Value>(&text)
        && let Some(messages) = doc.get("messages").and_then(Value::as_array)
    {
        let messages = messages.clone();
        return Ok(GeminiFile {
            header: doc,
            messages,
            rebuilds: 0,
        });
    }

    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let mut header: Value = lines
        .next()
        .and_then(|l| serde_json::from_str(l).ok())
        .with_context(|| format!("{} is not a Gemini session (no header line)", path.display()))?;
    if header.get("sessionId").is_none() {
        bail!("{} is not a Gemini session (header has no sessionId)", path.display());
    }

    let mut messages: Vec<Value> = Vec::new();
    let mut rebuilds = 0;
    for line in lines {
        // A torn final line (gemini mid-write) is skipped, not fatal.
        let Ok(rec) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if let Some(target) = rec.get("$rewindTo").and_then(Value::as_str) {
            let cut = position(&messages, target).unwrap_or(0);
            messages.truncate(cut);
        } else if let Some(set) = rec.get("$set") {
            if let Some(ts) = set.get("lastUpdated") {
                header["lastUpdated"] = ts.clone();
            }
            if let Some(rebuilt) = set.get("messages").and_then(Value::as_array) {
                let kept: Vec<&str> = rebuilt
                    .iter()
                    .filter_map(|m| m.get("id").and_then(Value::as_str))
                    .collect();
                if messages
                    .iter()
                    .filter_map(|m| m.get("id").and_then(Value::as_str))
                    .any(|id| !kept.contains(&id))
                {
                    rebuilds += 1;
                }
                for m in rebuilt {
                    upsert(&mut messages, m.clone());
                }
            }
        } else if rec.get("type").is_some() {
            upsert(&mut messages, rec);
        }
    }
    Ok(GeminiFile {
        header,
        messages,
        rebuilds,
    })
}

fn position(messages: &[Value], id: &str) -> Option<usize> {
    messages
        .iter()
        .position(|m| m.get("id").and_then(Value::as_str) == Some(id))
}

/// A record whose id is already folded replaces it in place; a new one appends.
fn upsert(messages: &mut Vec<Value>, record: Value) {
    let existing = record
        .get("id")
        .and_then(Value::as_str)
        .and_then(|id| position(messages, id));
    match existing {
        Some(i) => messages[i] = record,
        None => messages.push(record),
    }
}

pub fn load(path: &Path) -> Result<UniversalSession> {
    let file = read_file(path)?;
    let v = &file.header;

    let session_id = v
        .get("sessionId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let mut session = UniversalSession::new(session_id.clone());
    session.metadata.source_format = Some(SessionFormat::Gemini);
    session.metadata.original_session_id = Some(session_id);
    session.metadata.created_at = v
        .get("startTime")
        .and_then(Value::as_str)
        .and_then(parse_datetime);
    session.metadata.updated_at = v
        .get("lastUpdated")
        .and_then(Value::as_str)
        .and_then(parse_datetime);
    if let Some(hash) = v.get("projectHash").and_then(Value::as_str) {
        session.metadata.cwd = project_root_of(path).or_else(|| reverse_project_hash(hash));
        session
            .metadata
            .extra
            .insert("gemini_project_hash".to_string(), Value::String(hash.to_string()));
    }
    if file.rebuilds > 0 {
        session
            .metadata
            .extra
            .insert("gemini_history_rebuilds".to_string(), Value::from(file.rebuilds));
    }

    for m in &file.messages {
        import_message(&mut session, m);
    }

    if session.metadata.model.is_none() {
        session.metadata.model = file
            .messages
            .iter()
            .find_map(|m| m.get("model").and_then(Value::as_str))
            .map(str::to_string);
    }

    Ok(session)
}

/// Whether a session file holds a real conversation — a main session (not a
/// subagent's) with at least one turn gemini itself would show.
pub(crate) fn has_conversation(path: &Path) -> bool {
    let Ok(file) = read_file(path) else {
        return false;
    };
    if file.header.get("kind").and_then(Value::as_str) == Some("subagent") {
        return false;
    }
    file.messages.iter().any(|m| match m.get("type").and_then(Value::as_str) {
        Some("user") => !ignored_user_text(&part_text(m.get("content"))),
        Some("gemini") => !part_text(m.get("content")).trim().is_empty(),
        _ => false,
    })
}

/// The visible text of a content field: a plain string, or the `text` parts of
/// a part array — never `thought` parts, never function calls/responses or
/// inline data (those are tool traffic and attachments, not words).
fn part_text(content: Option<&Value>) -> String {
    text_parts(content).join("\n")
}

/// The visible text parts of a content field, one entry per part.
fn text_parts(content: Option<&Value>) -> Vec<String> {
    match content {
        Some(Value::String(s)) => vec![s.clone()],
        Some(Value::Array(parts)) => parts
            .iter()
            .filter(|p| p.get("thought").and_then(Value::as_bool) != Some(true))
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

/// User turns gemini's own loader skips: empty input, slash commands, `?`
/// help, and its injected `<session_context>` / `<hook_context>` preambles.
fn ignored_user_text(text: &str) -> bool {
    let t = text.trim_start();
    t.is_empty()
        || t.starts_with('/')
        || t.starts_with('?')
        || t.starts_with("<session_context>")
        || t.starts_with("<hook_context>")
}

fn import_message(session: &mut UniversalSession, m: &Value) {
    let timestamp = m
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(parse_datetime);
    let id = m.get("id").and_then(Value::as_str).map(str::to_string);

    match m.get("type").and_then(Value::as_str) {
        Some("user") => {
            // Text parts carry the prompt; functionResponse parts are tool
            // traffic and inlineData parts are images (text-only IR blocks).
            let parts = text_parts(m.get("content"));
            if ignored_user_text(&parts.join("\n")) {
                return;
            }
            let blocks: Vec<ContentBlock> =
                parts.iter().map(|t| ContentBlock::text("text", t)).collect();
            session.events.push(SessionEvent::Message(MessageEvent {
                id,
                parent_id: None,
                role: "user".to_string(),
                timestamp,
                blocks,
                metadata: BTreeMap::new(),
            }));
        }
        Some("gemini") => {
            // Thoughts first (the model's reasoning), then tool activity, then
            // the visible reply — the order the turn actually happened in.
            if let Some(thoughts) = m.get("thoughts").and_then(Value::as_array) {
                let summary: Vec<String> = thoughts
                    .iter()
                    .filter_map(|t| {
                        let subject = t.get("subject").and_then(Value::as_str);
                        let description = t.get("description").and_then(Value::as_str);
                        match (subject, description) {
                            (Some(s), Some(d)) => Some(format!("{s}: {d}")),
                            (Some(s), None) => Some(s.to_string()),
                            (None, Some(d)) => Some(d.to_string()),
                            (None, None) => None,
                        }
                    })
                    .collect();
                if !summary.is_empty() {
                    session.events.push(SessionEvent::Reasoning(ReasoningEvent {
                        id: id.clone().map(|b| format!("{b}:thoughts")),
                        parent_id: None,
                        timestamp,
                        summary,
                        metadata: BTreeMap::new(),
                    }));
                }
            }
            if let Some(calls) = m.get("toolCalls").and_then(Value::as_array) {
                for call in calls {
                    let call_id = call
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    session.events.push(SessionEvent::ToolCall(ToolCallEvent {
                        id: None,
                        parent_id: None,
                        call_id: call_id.clone(),
                        name: call
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("unknown")
                            .to_string(),
                        timestamp,
                        arguments: call.get("args").cloned().unwrap_or(Value::Null),
                        metadata: BTreeMap::new(),
                    }));
                    if let Some(result) = call.get("result") {
                        session.events.push(SessionEvent::ToolResult(ToolResultEvent {
                            id: None,
                            parent_id: None,
                            call_id,
                            timestamp,
                            output: result.clone(),
                            is_error: call.get("status").and_then(Value::as_str) == Some("error"),
                            metadata: BTreeMap::new(),
                        }));
                    }
                }
            }
            // The visible reply: a plain string, or a part array after a
            // history rebuild.
            let text = part_text(m.get("content"));
            if !text.trim().is_empty() {
                let mut metadata = BTreeMap::new();
                if let Some(model) = m.get("model") {
                    metadata.insert("model".to_string(), model.clone());
                    if session.metadata.model.is_none() {
                        session.metadata.model =
                            model.as_str().map(str::to_string);
                    }
                }
                session.events.push(SessionEvent::Message(MessageEvent {
                    id,
                    parent_id: None,
                    role: "assistant".to_string(),
                    timestamp,
                    blocks: vec![ContentBlock::text("text", text)],
                    metadata,
                }));
            }
        }
        // `info` = CLI notices ("Update successful!" etc.) — scaffold, dropped.
        _ => {}
    }
}

/// The cwd a named project dir records in its `.project_root` file
/// (`tmp/<name>/.project_root`, the session being `tmp/<name>/chats/<file>`).
fn project_root_of(path: &Path) -> Option<PathBuf> {
    let project_dir = path.parent()?.parent()?;
    let root = fs::read_to_string(project_dir.join(".project_root")).ok()?;
    let root = root.trim();
    (!root.is_empty()).then(|| PathBuf::from(root))
}

/// The session's recorded cwd, recovered from `projectHash = sha256(cwd)` by
/// hashing the known project paths in `~/.gemini/projects.json`.
fn reverse_project_hash(hash: &str) -> Option<PathBuf> {
    let root = super::gemini_root().ok()?;
    let text = fs::read_to_string(root.join("projects.json")).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    let projects = v.get("projects")?.as_object()?;
    projects
        .keys()
        .find(|path| sha256::hex(path) == hash)
        .map(PathBuf::from)
}

/// The session's header fields without folding the whole log: the first line
/// of a `.jsonl` log, or the whole legacy document.
fn read_header(path: &Path) -> Option<Value> {
    let mut first = String::new();
    BufReader::new(fs::File::open(path).ok()?)
        .read_line(&mut first)
        .ok()?;
    if let Ok(v) = serde_json::from_str::<Value>(first.trim())
        && v.get("sessionId").is_some()
    {
        return Some(v);
    }
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

/// Read just the projectHash of a session file (for cwd-scoped discovery).
pub(crate) fn session_project_hash(path: &Path) -> Option<String> {
    read_header(path)?
        .get("projectHash")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Read just the sessionId of a session file.
pub(crate) fn session_id(path: &Path) -> Option<String> {
    read_header(path)?
        .get("sessionId")
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn parse_datetime(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}
