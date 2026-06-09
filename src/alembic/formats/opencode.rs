//! OpenCode session codec.
//!
//! OpenCode's store is sqlite, but the codec never touches it: the file format
//! here is the EXPORT shape (`opencode export <id>` output / `opencode import
//! <file>` input) — `{ info, messages: [{ info, parts[] }] }`. Import
//! PRESERVES session ids and re-import is a clean upsert (verified live), so
//! the writer builds this JSON and alembic runs `opencode import` on it as the
//! registry step.
//!
//! Loader tolerance: `opencode export` appends a status line AFTER the JSON on
//! stdout; files captured from it may carry that trailer — slice to the last
//! closing brace before parsing.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::alembic::ir::{
    ContentBlock, MessageEvent, ReasoningEvent, SessionEvent, SessionFormat, ToolCallEvent,
    ToolResultEvent, UniversalSession,
};

pub fn load(path: &Path) -> Result<UniversalSession> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("failed to read OpenCode session {}", path.display()))?;
    parse_export(&text).with_context(|| format!("invalid OpenCode export in {}", path.display()))
}

/// Parse export-shaped JSON (tolerating a trailing status line).
pub fn parse_export(text: &str) -> Result<UniversalSession> {
    let end = text.rfind('}').map(|i| i + 1).unwrap_or(text.len());
    let v: Value = serde_json::from_str(&text[..end]).context("failed to parse export JSON")?;
    let info = v.get("info").context("export missing `info`")?;

    let session_id = info
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let mut session = UniversalSession::new(session_id.clone());
    session.metadata.source_format = Some(SessionFormat::OpenCode);
    session.metadata.original_session_id = Some(session_id);
    session.metadata.title = info
        .get("title")
        .and_then(Value::as_str)
        .map(str::to_string);
    session.metadata.cwd = info
        .get("directory")
        .and_then(Value::as_str)
        .map(PathBuf::from);
    session.metadata.platform_version = info
        .get("version")
        .and_then(Value::as_str)
        .map(str::to_string);
    session.metadata.created_at = info
        .get("time")
        .and_then(|t| t.get("created"))
        .and_then(Value::as_i64)
        .and_then(ms_to_datetime);
    session.metadata.updated_at = info
        .get("time")
        .and_then(|t| t.get("updated"))
        .and_then(Value::as_i64)
        .and_then(ms_to_datetime);

    for m in v
        .get("messages")
        .and_then(Value::as_array)
        .map(|a| a.as_slice())
        .unwrap_or(&[])
    {
        import_message(&mut session, m);
    }

    Ok(session)
}

fn import_message(session: &mut UniversalSession, m: &Value) {
    let info = m.get("info").cloned().unwrap_or(Value::Null);
    let role = info
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or("assistant")
        .to_string();
    let timestamp = info
        .get("time")
        .and_then(|t| t.get("created"))
        .and_then(Value::as_i64)
        .and_then(ms_to_datetime);
    let msg_id = info.get("id").and_then(Value::as_str).map(str::to_string);

    if session.metadata.model.is_none()
        && let Some(model) = info.get("modelID").and_then(Value::as_str)
    {
        session.metadata.model = Some(model.to_string());
    }

    let mut metadata = BTreeMap::new();
    if let Some(model) = info.get("modelID") {
        metadata.insert("model".to_string(), model.clone());
    }

    let mut text_blocks: Vec<ContentBlock> = Vec::new();
    for p in m
        .get("parts")
        .and_then(Value::as_array)
        .map(|a| a.as_slice())
        .unwrap_or(&[])
    {
        match p.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(t) = p.get("text").and_then(Value::as_str) {
                    text_blocks.push(ContentBlock::text("text", t));
                }
            }
            Some("reasoning") => {
                if let Some(t) = p.get("text").and_then(Value::as_str)
                    && !t.trim().is_empty()
                {
                    session.events.push(SessionEvent::Reasoning(ReasoningEvent {
                        id: p.get("id").and_then(Value::as_str).map(str::to_string),
                        parent_id: msg_id.clone(),
                        timestamp,
                        summary: vec![t.to_string()],
                        metadata: BTreeMap::new(),
                    }));
                }
            }
            Some("tool") => {
                let call_id = p
                    .get("callID")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let state = p.get("state").cloned().unwrap_or(Value::Null);
                session.events.push(SessionEvent::ToolCall(ToolCallEvent {
                    id: p.get("id").and_then(Value::as_str).map(str::to_string),
                    parent_id: msg_id.clone(),
                    call_id: call_id.clone(),
                    name: p
                        .get("tool")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown")
                        .to_string(),
                    timestamp,
                    arguments: state.get("input").cloned().unwrap_or(Value::Null),
                    metadata: BTreeMap::new(),
                }));
                session.events.push(SessionEvent::ToolResult(ToolResultEvent {
                    id: None,
                    parent_id: msg_id.clone(),
                    call_id,
                    timestamp,
                    output: state.get("output").cloned().unwrap_or(Value::Null),
                    is_error: state.get("status").and_then(Value::as_str) != Some("completed"),
                    metadata: BTreeMap::new(),
                }));
            }
            // step-start / step-finish = turn lifecycle markers; regenerated
            // by opencode itself, not conversation content.
            _ => {}
        }
    }

    if !text_blocks.is_empty() {
        session.events.push(SessionEvent::Message(MessageEvent {
            id: msg_id,
            parent_id: None,
            role,
            timestamp,
            blocks: text_blocks,
            metadata,
        }));
    }
}

/// Write a session as import-ready export JSON. `output` must be a file path
/// (the OpenCode store is a database — alembic runs `opencode import` on this
/// file as the registry step; this file itself is the Constant-owned artifact).
pub fn write(session: &UniversalSession, output: &Path) -> Result<PathBuf> {
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let doc = build_export(session);
    let text = serde_json::to_string_pretty(&doc).context("failed to encode OpenCode export")?;

    // Atomic, like every materialization.
    let tmp = super::tmp_sibling(output);
    let mut guard = super::TmpCleanup::new(&tmp);
    use std::io::Write as _;
    let mut file =
        fs::File::create(&tmp).with_context(|| format!("failed to create {}", tmp.display()))?;
    file.write_all(text.as_bytes())
        .with_context(|| format!("failed to write {}", tmp.display()))?;
    file.sync_all()
        .with_context(|| format!("failed to flush {}", tmp.display()))?;
    drop(file);
    fs::rename(&tmp, output)
        .with_context(|| format!("failed to move {} into place", output.display()))?;
    guard.keep();
    Ok(output.to_path_buf())
}

fn build_export(session: &UniversalSession) -> Value {
    let created = session
        .metadata
        .created_at
        .map(|t| t.timestamp_millis())
        .unwrap_or_else(|| Utc::now().timestamp_millis());
    let updated = session
        .metadata
        .updated_at
        .map(|t| t.timestamp_millis())
        .unwrap_or(created);
    let directory = session
        .metadata
        .cwd
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| ".".to_string());
    let title = session
        .metadata
        .title
        .clone()
        .unwrap_or_else(|| "carried conversation".to_string());
    let model_id = session
        .metadata
        .model
        .clone()
        .unwrap_or_else(|| "carried".to_string());

    let mut messages: Vec<Value> = Vec::new();
    // Tool calls buffer until their result arrives, then ride as one tool part
    // on the surrounding assistant message (opencode's shape).
    let mut pending_calls: BTreeMap<String, (String, Value)> = BTreeMap::new();
    let mut current_assistant: Option<usize> = None;
    // opencode requires assistant messages to carry the parent message id.
    let mut last_msg_id: Option<String> = None;

    for event in &session.events {
        match event {
            SessionEvent::Message(msg) => {
                let text = msg
                    .blocks
                    .iter()
                    .filter_map(|b| b.text.as_deref())
                    .collect::<Vec<_>>()
                    .join("\n");
                if text.trim().is_empty() {
                    continue;
                }
                let ts = msg
                    .timestamp
                    .map(|t| t.timestamp_millis())
                    .unwrap_or(updated);
                let msg_id = mint_id("msg");
                let is_assistant = msg.role == "assistant";
                let parent = last_msg_id.clone().unwrap_or_else(|| msg_id.clone());
                let info = if is_assistant {
                    json!({
                        "role": "assistant",
                        "sessionID": session.metadata.session_id,
                        "parentID": parent,
                        "time": {"created": ts, "completed": ts},
                        "modelID": model_id,
                        "providerID": "constant",
                        "mode": "build",
                        "agent": "build",
                        "path": {"cwd": directory, "root": directory},
                        "cost": 0,
                        "tokens": {"input": 0, "output": 0, "reasoning": 0,
                                    "cache": {"read": 0, "write": 0}},
                    })
                } else {
                    json!({
                        "role": "user",
                        "sessionID": session.metadata.session_id,
                        "time": {"created": ts},
                        "summary": {"diffs": []},
                        "agent": "build",
                        "model": {"providerID": "constant", "modelID": model_id},
                    })
                };
                let part = part_json(
                    "text",
                    &session.metadata.session_id,
                    &msg_id,
                    json!({"text": text}),
                );
                messages.push(json!({"info": merge_id(info, &msg_id), "parts": [part]}));
                current_assistant = is_assistant.then_some(messages.len() - 1);
                last_msg_id = Some(msg_id);
            }
            SessionEvent::ToolCall(call) => {
                pending_calls.insert(
                    call.call_id.clone(),
                    (call.name.clone(), call.arguments.clone()),
                );
            }
            SessionEvent::ToolResult(result) => {
                let (name, input) = pending_calls
                    .remove(&result.call_id)
                    .unwrap_or_else(|| ("unknown".to_string(), Value::Null));
                let output = match &result.output {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                let state = json!({
                    "status": if result.is_error { "error" } else { "completed" },
                    "input": input,
                    "output": output,
                    "title": name,
                    "metadata": {},
                    "time": {"start": updated, "end": updated},
                });
                // Tool parts belong to an assistant message; open one if the
                // conversation hasn't produced it yet.
                let idx = match current_assistant {
                    Some(i) => i,
                    None => {
                        let msg_id = mint_id("msg");
                        let parent = last_msg_id.clone().unwrap_or_else(|| msg_id.clone());
                        last_msg_id = Some(msg_id.clone());
                        messages.push(json!({
                            "info": merge_id(json!({
                                "role": "assistant",
                                "sessionID": session.metadata.session_id,
                                "parentID": parent,
                                "time": {"created": updated, "completed": updated},
                                "modelID": model_id,
                                "providerID": "constant",
                                "mode": "build",
                                "agent": "build",
                                "path": {"cwd": directory, "root": directory},
                                "cost": 0,
                                "tokens": {"input": 0, "output": 0, "reasoning": 0,
                                            "cache": {"read": 0, "write": 0}},
                            }), &msg_id),
                            "parts": [],
                        }));
                        current_assistant = Some(messages.len() - 1);
                        messages.len() - 1
                    }
                };
                let msg_id = messages[idx]["info"]["id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                let part = part_json(
                    "tool",
                    &session.metadata.session_id,
                    &msg_id,
                    json!({"tool": name, "callID": result.call_id, "state": state}),
                );
                messages[idx]["parts"]
                    .as_array_mut()
                    .expect("parts array")
                    .push(part);
            }
            // Reasoning never crosses runtimes.
            SessionEvent::Reasoning(_) => {}
        }
    }

    json!({
        "info": {
            "id": session.metadata.session_id,
            "slug": slugify(&title),
            "projectID": "global",
            "directory": directory,
            "title": title,
            "version": session
                .metadata
                .platform_version
                .clone()
                .unwrap_or_else(|| "1.14.48".to_string()),
            "summary": {"additions": 0, "deletions": 0, "files": 0},
            "time": {"created": created, "updated": updated},
        },
        "messages": messages,
    })
}

fn part_json(kind: &str, session_id: &str, message_id: &str, body: Value) -> Value {
    let mut obj = Map::new();
    obj.insert("type".to_string(), Value::String(kind.to_string()));
    obj.insert("id".to_string(), Value::String(mint_id("prt")));
    obj.insert("sessionID".to_string(), Value::String(session_id.to_string()));
    obj.insert("messageID".to_string(), Value::String(message_id.to_string()));
    if let Value::Object(extra) = body {
        obj.extend(extra);
    }
    Value::Object(obj)
}

fn merge_id(info: Value, id: &str) -> Value {
    let mut obj = info.as_object().cloned().unwrap_or_default();
    obj.insert("id".to_string(), Value::String(id.to_string()));
    Value::Object(obj)
}

/// Mint an opencode-shaped id (`msg_…` / `prt_…` / `ses_…`): their ids are a
/// prefix + ~26 alphanumerics; hex from a UUID fits the alphabet.
pub fn mint_id(prefix: &str) -> String {
    let hex = Uuid::new_v4().simple().to_string();
    format!("{prefix}_{}", &hex[..26])
}

fn slugify(title: &str) -> String {
    let s: String = title
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = s.trim_matches('-');
    if trimmed.is_empty() {
        "carried-conversation".to_string()
    } else {
        trimmed.chars().take(40).collect()
    }
}

fn ms_to_datetime(ms: i64) -> Option<DateTime<Utc>> {
    DateTime::from_timestamp_millis(ms)
}

/// Read just the session id from an export-shaped file.
pub(crate) fn session_id(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    let end = text.rfind('}').map(|i| i + 1)?;
    let v: Value = serde_json::from_str(&text[..end]).ok()?;
    v.get("info")?.get("id")?.as_str().map(str::to_string)
}
