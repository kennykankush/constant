//! Gemini CLI session codec — LOADER ONLY for now.
//!
//! A gemini session is a single JSON document:
//! `{ sessionId, projectHash, startTime, lastUpdated, kind, messages[] }`
//! with three message types: `user` (content = array of `{text}` /
//! `{inlineData}` blocks), `gemini` (content = plain string, plus `thoughts`,
//! `toolCalls`, `tokens`, `model`), and `info` (system notices — dropped).
//!
//! `projectHash` is `sha256(cwd)` — the file never stores the path itself, so
//! the cwd is recovered by hashing the candidates in `~/.gemini/projects.json`.
//!
//! The WRITER is deliberately absent: where gemini 0.40 expects materialized
//! sessions to land needs one live verification first (see
//! docs/third-runtime-formats.md). Until then gemini is a carry SOURCE.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::alembic::ir::{
    ContentBlock, MessageEvent, ReasoningEvent, SessionEvent, SessionFormat, ToolCallEvent,
    ToolResultEvent, UniversalSession,
};
use crate::alembic::sha256;

pub fn load(path: &Path) -> Result<UniversalSession> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("failed to read Gemini session {}", path.display()))?;
    let v: Value = serde_json::from_str(&text)
        .with_context(|| format!("invalid JSON in {}", path.display()))?;

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
        session.metadata.cwd = reverse_project_hash(hash);
        session
            .metadata
            .extra
            .insert("gemini_project_hash".to_string(), Value::String(hash.to_string()));
    }

    for m in v
        .get("messages")
        .and_then(Value::as_array)
        .map(|a| a.as_slice())
        .unwrap_or(&[])
    {
        import_message(&mut session, m);
    }

    if session.metadata.model.is_none() {
        session.metadata.model = v
            .get("messages")
            .and_then(Value::as_array)
            .and_then(|a| {
                a.iter()
                    .find_map(|m| m.get("model").and_then(Value::as_str))
            })
            .map(str::to_string);
    }

    Ok(session)
}

fn import_message(session: &mut UniversalSession, m: &Value) {
    let timestamp = m
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(parse_datetime);
    let id = m.get("id").and_then(Value::as_str).map(str::to_string);

    match m.get("type").and_then(Value::as_str) {
        Some("user") => {
            // content is an array of blocks; text blocks carry the prompt,
            // inlineData blocks are images (skipped — text-only IR blocks).
            let blocks: Vec<ContentBlock> = m
                .get("content")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|b| b.get("text").and_then(Value::as_str))
                        .map(|t| ContentBlock::text("text", t))
                        .collect()
                })
                .unwrap_or_default();
            if blocks.is_empty() {
                return;
            }
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
                            is_error: false,
                            metadata: BTreeMap::new(),
                        }));
                    }
                }
            }
            // The visible reply: a plain string (observed) — tolerate a block
            // array too, in case newer versions structure it.
            let text = match m.get("content") {
                Some(Value::String(s)) => s.clone(),
                Some(Value::Array(items)) => items
                    .iter()
                    .filter_map(|b| b.get("text").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join("\n"),
                _ => String::new(),
            };
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

/// Read just the projectHash of a session file (for cwd-scoped discovery).
pub(crate) fn session_project_hash(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    v.get("projectHash")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Read just the sessionId of a session file.
pub(crate) fn session_id(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    v.get("sessionId").and_then(Value::as_str).map(str::to_string)
}

fn parse_datetime(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}
