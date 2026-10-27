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
//!
//! OpenCode 2.x (`opencode session export|import`) replaced the shape: session
//! `info` carries `location.directory`, `cost` and `tokens`, and `messages` is
//! a flat tagged union (`{type: "user", text}`, `{type: "assistant", content:
//! [text | reasoning | tool]}`, plus runtime-only kinds like `idle`). A tool's
//! output is a `content` array, a failure a structured `error`. The reader
//! accepts both shapes; the writer emits the one the target's version takes
//! (read from `platform_version`, which alembic stamps from `--version`).

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

/// Parse export-shaped JSON (tolerating a trailing status line; a cut-off
/// export is an error, never a silently shorter conversation).
pub fn parse_export(text: &str) -> Result<UniversalSession> {
    let json = super::first_json_value(text).context("failed to parse export JSON")?;
    let v: Value = serde_json::from_str(json).context("failed to parse export JSON")?;
    let info = v.get("info").context("export missing `info`")?;

    let messages = v
        .get("messages")
        .and_then(Value::as_array)
        .map(|a| a.as_slice())
        .unwrap_or(&[]);
    let v2 = info.get("location").is_some()
        || messages
            .iter()
            .any(|m| m.get("info").is_none() && m.get("type").is_some());

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
        .get("location")
        .and_then(|l| l.get("directory"))
        .or_else(|| info.get("directory"))
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

    for m in messages {
        if v2 {
            import_message_v2(&mut session, m);
        } else {
            import_message(&mut session, m);
        }
    }

    Ok(session)
}

/// One 2.x message. Only `user` and `assistant` are conversation; the other
/// kinds (`synthetic`, `system`, `idle`, `compaction`, `shell`, selections…)
/// are opencode's own bookkeeping and injections, like 1.x synthetic parts.
fn import_message_v2(session: &mut UniversalSession, m: &Value) {
    let timestamp = m
        .get("time")
        .and_then(|t| t.get("created"))
        .and_then(Value::as_i64)
        .and_then(ms_to_datetime);
    let msg_id = m.get("id").and_then(Value::as_str).map(str::to_string);
    let message = |role: &str, blocks: Vec<ContentBlock>, metadata| {
        SessionEvent::Message(MessageEvent {
            id: msg_id.clone(),
            parent_id: None,
            role: role.to_string(),
            timestamp,
            blocks,
            metadata,
        })
    };

    match m.get("type").and_then(Value::as_str) {
        Some("user") => {
            if let Some(t) = m.get("text").and_then(Value::as_str)
                && !t.trim().is_empty()
            {
                let event = message("user", vec![ContentBlock::text("text", t)], BTreeMap::new());
                session.events.push(event);
            }
        }
        Some("assistant") => {
            let model = m.get("model").and_then(|x| x.get("id")).cloned();
            if session.metadata.model.is_none()
                && let Some(Value::String(id)) = &model
            {
                session.metadata.model = Some(id.clone());
            }
            let mut metadata = BTreeMap::new();
            if let Some(model) = model {
                metadata.insert("model".to_string(), model);
            }
            // Text runs flush before a tool, so the carried order is the
            // order the assistant actually produced them in.
            let mut text_blocks: Vec<ContentBlock> = Vec::new();
            for c in m
                .get("content")
                .and_then(Value::as_array)
                .map(|a| a.as_slice())
                .unwrap_or(&[])
            {
                match c.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        if let Some(t) = c.get("text").and_then(Value::as_str) {
                            text_blocks.push(ContentBlock::text("text", t));
                        }
                    }
                    Some("reasoning") => {
                        if let Some(t) = c.get("text").and_then(Value::as_str)
                            && !t.trim().is_empty()
                        {
                            session.events.push(SessionEvent::Reasoning(ReasoningEvent {
                                id: None,
                                parent_id: msg_id.clone(),
                                timestamp,
                                summary: vec![t.to_string()],
                                metadata: BTreeMap::new(),
                            }));
                        }
                    }
                    Some("tool") => {
                        if !text_blocks.is_empty() {
                            let blocks = std::mem::take(&mut text_blocks);
                            session.events.push(message("assistant", blocks, metadata.clone()));
                        }
                        import_tool_v2(session, c, msg_id.clone(), timestamp);
                    }
                    _ => {}
                }
            }
            if !text_blocks.is_empty() {
                session.events.push(message("assistant", text_blocks, metadata));
            }
        }
        _ => {}
    }
}

/// A 2.x tool: the call always; the result only once the tool settled
/// (`completed` / `error`) — a `running` or `streaming` tool has none yet.
fn import_tool_v2(
    session: &mut UniversalSession,
    c: &Value,
    msg_id: Option<String>,
    timestamp: Option<DateTime<Utc>>,
) {
    let call_id = c
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let state = c.get("state").cloned().unwrap_or(Value::Null);
    session.events.push(SessionEvent::ToolCall(ToolCallEvent {
        id: None,
        parent_id: msg_id.clone(),
        call_id: call_id.clone(),
        name: c
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
        timestamp,
        arguments: state.get("input").cloned().unwrap_or(Value::Null),
        metadata: BTreeMap::new(),
    }));
    let status = state.get("status").and_then(Value::as_str);
    if !matches!(status, Some("completed") | Some("error")) {
        return;
    }
    // Output is a content array; carry its text (the model-visible part).
    let text = |content: &Value| -> Option<Value> {
        let items = content.as_array()?;
        let texts: Vec<&str> = items
            .iter()
            .filter_map(|i| i.get("text").and_then(Value::as_str))
            .collect();
        Some(if texts.is_empty() {
            content.clone()
        } else {
            Value::String(texts.join("\n"))
        })
    };
    let output = match status {
        Some("error") => state
            .get("error")
            .and_then(|e| e.get("message").cloned().or_else(|| Some(e.clone())))
            .unwrap_or(Value::Null),
        _ => state.get("content").and_then(text).unwrap_or(Value::Null),
    };
    session.events.push(SessionEvent::ToolResult(ToolResultEvent {
        id: None,
        parent_id: msg_id,
        call_id,
        timestamp,
        output,
        is_error: status == Some("error"),
        metadata: BTreeMap::new(),
    }));
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
            // `synthetic` / `ignored` text is opencode's own injection (file
            // attachments rendered as text, notices) — not the conversation.
            Some("text")
                if p.get("synthetic").and_then(Value::as_bool) != Some(true)
                    && p.get("ignored").and_then(Value::as_bool) != Some(true) =>
            {
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
                    // A failed tool has `error` and no `output`.
                    output: state
                        .get("output")
                        .or_else(|| state.get("error"))
                        .cloned()
                        .unwrap_or(Value::Null),
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
    let doc = if is_v2(session.metadata.platform_version.as_deref()) {
        build_export_v2(session)
    } else {
        build_export(session)
    };
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
    // opencode orders messages by (created time, id) and parts by id: every
    // message gets a strictly increasing time in conversation order (an
    // untimed or out-of-order one would otherwise sort to the end), and ids
    // are minted ascending from that time, the way opencode mints its own.
    let mut clock = created - 1;
    let mut ids = IdClock::default();
    let mut current_ts = created;
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
                    .unwrap_or(clock)
                    .max(clock + 1);
                clock = ts;
                current_ts = ts;
                let msg_id = ids.next("msg", ts);
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
                    ids.next("prt", ts),
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
                // The exact state shapes `opencode import` validates: `input`
                // is always an object; a failure carries `error`, not `output`.
                let input = match input {
                    Value::Object(_) => input,
                    Value::Null => json!({}),
                    other => json!({ "input": other }),
                };
                let state = if result.is_error {
                    json!({
                        "status": "error",
                        "input": input,
                        "error": output,
                        "metadata": {},
                        "time": {"start": current_ts, "end": current_ts},
                    })
                } else {
                    json!({
                        "status": "completed",
                        "input": input,
                        "output": output,
                        "title": name,
                        "metadata": {},
                        "time": {"start": current_ts, "end": current_ts},
                    })
                };
                // Tool parts belong to an assistant message; open one if the
                // conversation hasn't produced it yet.
                let idx = match current_assistant {
                    Some(i) => i,
                    None => {
                        clock += 1;
                        current_ts = clock;
                        let msg_id = ids.next("msg", clock);
                        let parent = last_msg_id.clone().unwrap_or_else(|| msg_id.clone());
                        last_msg_id = Some(msg_id.clone());
                        messages.push(json!({
                            "info": merge_id(json!({
                                "role": "assistant",
                                "sessionID": session.metadata.session_id,
                                "parentID": parent,
                                "time": {"created": clock, "completed": clock},
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
                    ids.next("prt", current_ts),
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
            "time": {"created": created, "updated": updated.max(clock)},
        },
        "messages": messages,
    })
}

/// Whether an opencode version takes the 2.x session shape.
pub(crate) fn is_v2(version: Option<&str>) -> bool {
    version
        .and_then(|v| v.trim_start_matches('v').split('.').next()?.parse::<u32>().ok())
        .is_some_and(|major| major >= 2)
}

/// The 2.x import document (`SessionTransfer.Data`): session info + a flat
/// list of typed messages. Same ordering rules as 1.x — strictly increasing
/// times, ascending ids — and tool calls still wait for their result, then
/// ride on the surrounding assistant message as one `tool` content item.
fn build_export_v2(session: &UniversalSession) -> Value {
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
    let model = json!({
        "id": session.metadata.model.clone().unwrap_or_else(|| "carried".to_string()),
        "providerID": "constant",
    });
    let assistant = |id: String, ts: i64, content: Vec<Value>| {
        json!({
            "id": id,
            "time": {"created": ts, "completed": ts},
            "type": "assistant",
            "agent": "build",
            "model": model,
            "content": content,
        })
    };

    let mut messages: Vec<Value> = Vec::new();
    let mut clock = created - 1;
    let mut ids = IdClock::default();
    let mut current_ts = created;
    let mut pending_calls: BTreeMap<String, (String, Value)> = BTreeMap::new();
    let mut current_assistant: Option<usize> = None;

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
                    .unwrap_or(clock)
                    .max(clock + 1);
                clock = ts;
                current_ts = ts;
                let id = ids.next("msg", ts);
                if msg.role == "assistant" {
                    messages.push(assistant(id, ts, vec![json!({"type": "text", "text": text})]));
                    current_assistant = Some(messages.len() - 1);
                } else {
                    messages.push(json!({
                        "id": id,
                        "time": {"created": ts},
                        "text": text,
                        "files": [],
                        "type": "user",
                    }));
                    current_assistant = None;
                }
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
                let input = match input {
                    Value::Object(_) => input,
                    Value::Null => json!({}),
                    other => json!({ "input": other }),
                };
                let state = if result.is_error {
                    json!({
                        "status": "error",
                        "input": input,
                        "error": {"type": "tool", "message": output},
                    })
                } else {
                    json!({
                        "status": "completed",
                        "input": input,
                        "content": [{"type": "text", "text": output}],
                    })
                };
                let idx = match current_assistant {
                    Some(i) => i,
                    None => {
                        clock += 1;
                        current_ts = clock;
                        messages.push(assistant(ids.next("msg", clock), clock, Vec::new()));
                        current_assistant = Some(messages.len() - 1);
                        messages.len() - 1
                    }
                };
                messages[idx]["content"]
                    .as_array_mut()
                    .expect("content array")
                    .push(json!({
                        "type": "tool",
                        "id": result.call_id,
                        "name": name,
                        "state": state,
                        "time": {"created": current_ts, "completed": current_ts},
                    }));
            }
            // Reasoning never crosses runtimes.
            SessionEvent::Reasoning(_) => {}
        }
    }

    json!({
        "info": {
            "id": session.metadata.session_id,
            "projectID": "global",
            "title": title,
            "cost": 0,
            "tokens": {"input": 0, "output": 0, "reasoning": 0,
                       "cache": {"read": 0, "write": 0}},
            "time": {"created": created, "updated": updated.max(clock)},
            "location": {"directory": directory},
        },
        "messages": messages,
    })
}

fn part_json(kind: &str, id: String, session_id: &str, message_id: &str, body: Value) -> Value {
    let mut obj = Map::new();
    obj.insert("type".to_string(), Value::String(kind.to_string()));
    obj.insert("id".to_string(), Value::String(id));
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

/// opencode's ascending identifier (its `Identifier.ascending`): prefix + 12
/// hex digits of the low 48 bits of `ms * 4096 + counter` (the counter resets
/// when the millisecond moves) + 14 random base62 — ids sort by creation time.
#[derive(Default)]
struct IdClock {
    last_ms: i64,
    counter: u64,
}

impl IdClock {
    fn next(&mut self, prefix: &str, ms: i64) -> String {
        if ms != self.last_ms {
            self.last_ms = ms;
            self.counter = 0;
        }
        self.counter += 1;
        let stamp = (ms.max(0) as u64)
            .wrapping_mul(0x1000)
            .wrapping_add(self.counter)
            & 0xFFFF_FFFF_FFFF;
        const BASE62: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
        let tail: String = Uuid::new_v4().as_bytes()[..14]
            .iter()
            .map(|b| BASE62[(*b as usize) % 62] as char)
            .collect();
        format!("{prefix}_{stamp:012x}{tail}")
    }
}

/// Mint an opencode-shaped id (`ses_…`): a prefix + ~26 alphanumerics; hex
/// from a UUID fits the alphabet. (Messages and parts use [`IdClock`].)
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
    let v: Value = serde_json::from_str(super::first_json_value(&text).ok()?).ok()?;
    v.get("info")?.get("id")?.as_str().map(str::to_string)
}
