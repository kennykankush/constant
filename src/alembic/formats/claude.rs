use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::alembic::ir::{
    ContentBlock, MessageEvent, ReasoningEvent, SessionEvent, SessionFormat, SessionMetadata,
    ToolCallEvent, ToolResultEvent, UniversalSession,
};

pub struct ClaudeMaterialization {
    pub session_file: PathBuf,
    pub history_file: Option<PathBuf>,
}

/// Scratch key for the latest `aiTitle` while loading (never leaves `load`).
const AI_TITLE: &str = "claude_ai_title";

/// The `model` stamped on every assistant record Constant writes. Claude Code
/// resumes on the model named by the session's last assistant record when it
/// is a full model id, and otherwise on the user's own default (settings or
/// `/model`). A carried turn was not produced by this Claude call, so it gets
/// Claude Code's own marker for that, and the resume follows the user's
/// current default instead of a model pinned at carry time. Verified on
/// 2.1.289: a full id is honored; `<synthetic>`, an alias, or no model falls
/// back to the default, with the carried turns still in context. To pin a
/// model, `CONSTANT_CLAUDE_MODEL` adds `--model` at launch.
const CARRIED_MODEL: &str = "<synthetic>";

pub fn load(path: &Path) -> Result<UniversalSession> {
    let file = File::open(path)
        .with_context(|| format!("failed to open Claude session {}", path.display()))?;
    let reader = BufReader::new(file);

    let mut session = UniversalSession::new(Uuid::new_v4().to_string());
    session.metadata.source_format = Some(SessionFormat::Claude);

    // A torn FINAL line (a child killed mid-flush) must not void the whole
    // conversation: tolerate it. A bad line FOLLOWED by valid data is real
    // corruption and still fails loudly.
    let mut torn: Option<anyhow::Error> = None;
    for line in reader.lines() {
        let line = line.with_context(|| format!("failed to read {}", path.display()))?;
        if line.trim().is_empty() {
            continue;
        }

        let value: Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(e) => {
                torn = Some(anyhow::anyhow!(
                    "invalid JSONL in {}: {e}",
                    path.display()
                ));
                continue;
            }
        };
        if let Some(e) = torn.take() {
            return Err(e.context("corrupt line followed by valid data"));
        }
        import_metadata(&mut session.metadata, &value);

        match value.get("type").and_then(Value::as_str) {
            Some("user") => import_user_entry(&mut session.events, &value),
            Some("assistant") => import_assistant_entry(&mut session.events, &value),
            Some("attachment") => import_queued_prompt(&mut session.events, &value),
            _ => {}
        }
    }

    if session.metadata.title.is_none() {
        session.metadata.title = session
            .metadata
            .extra
            .remove(AI_TITLE)
            .and_then(|v| v.as_str().map(str::to_string))
            .or_else(|| derive_title(&session));
    }
    session.metadata.extra.remove(AI_TITLE);

    Ok(session)
}

fn import_metadata(metadata: &mut SessionMetadata, value: &Value) {
    if let Some(session_id) = value.get("sessionId").and_then(Value::as_str) {
        metadata.session_id = session_id.to_string();
        metadata.original_session_id = Some(session_id.to_string());
        metadata.source_format = Some(SessionFormat::Claude);
    }
    if let Some(cwd) = value.get("cwd").and_then(Value::as_str) {
        metadata.cwd = Some(PathBuf::from(cwd));
    }
    if let Some(branch) = value.get("gitBranch").and_then(Value::as_str) {
        metadata.git_branch = Some(branch.to_string());
    }
    if let Some(version) = value.get("version").and_then(Value::as_str) {
        metadata.platform_version = Some(version.to_string());
    }
    // A `/rename` inside Claude Code appends a custom-title record — the
    // strongest naming signal there is. Latest one wins.
    if let Some(custom) = value.get("customTitle").and_then(Value::as_str)
        && !custom.trim().is_empty()
    {
        metadata.title = Some(custom.trim().to_string());
    }
    // Unrenamed sessions get an `ai-title` record (re-appended as the
    // conversation evolves) — Claude's own picker name after a custom title.
    if let Some(ai) = value.get("aiTitle").and_then(Value::as_str)
        && !ai.trim().is_empty()
    {
        metadata
            .extra
            .insert(AI_TITLE.to_string(), Value::String(ai.trim().to_string()));
    }
    let timestamp = value
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(parse_datetime);
    update_time_bounds(metadata, timestamp);
}

fn import_user_entry(events: &mut Vec<SessionEvent>, value: &Value) {
    let timestamp = value
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(parse_datetime);
    let uuid = value
        .get("uuid")
        .and_then(Value::as_str)
        .map(str::to_string);
    let parent_uuid = value
        .get("parentUuid")
        .and_then(Value::as_str)
        .map(str::to_string);

    let Some(message) = value.get("message") else {
        return;
    };
    let metadata = injected_metadata(value);

    let content = message.get("content").cloned().unwrap_or(Value::Null);
    match content {
        Value::String(text) => {
            if text.trim().is_empty() {
                return;
            }
            events.push(SessionEvent::Message(MessageEvent {
                id: uuid,
                parent_id: parent_uuid,
                role: "user".to_string(),
                timestamp,
                blocks: vec![ContentBlock::text("text", text)],
                metadata,
            }));
        }
        Value::Array(items) => {
            let mut message_blocks = Vec::new();

            for item in items {
                match item.get("type").and_then(Value::as_str) {
                    Some("tool_result") => {
                        if !message_blocks.is_empty() {
                            events.push(SessionEvent::Message(MessageEvent {
                                id: uuid.clone(),
                                parent_id: parent_uuid.clone(),
                                role: "user".to_string(),
                                timestamp,
                                blocks: std::mem::take(&mut message_blocks),
                                metadata: metadata.clone(),
                            }));
                        }

                        import_tool_result_block(
                            events,
                            &item,
                            timestamp,
                            uuid.clone(),
                            parent_uuid.clone(),
                        );
                    }
                    _ => message_blocks.push(normalize_block(&item)),
                }
            }

            if !message_blocks.is_empty() {
                events.push(SessionEvent::Message(MessageEvent {
                    id: uuid,
                    parent_id: parent_uuid,
                    role: "user".to_string(),
                    timestamp,
                    blocks: message_blocks,
                    metadata,
                }));
            }
        }
        _ => {}
    }
}

/// Claude Code marks the user turns IT wrote — command caveats and output,
/// skill bodies, hook feedback, compaction summaries, task notifications, peer
/// messages, auto-continuations — with `isMeta`, `isCompactSummary`,
/// `isVisibleInTranscriptOnly`, or a non-human `origin.kind`. Tag them so
/// `sanitize` drops AND counts them (invariant 5: declared, never silent).
/// Typed prompts carry `origin: {kind: "human"}` (2.1.2xx) or no origin at all
/// (≤ 2.1.170).
fn injected_metadata(value: &Value) -> BTreeMap<String, Value> {
    let flag = |k: &str| value.get(k).and_then(Value::as_bool) == Some(true);
    let foreign_origin = value
        .get("origin")
        .and_then(|o| o.get("kind"))
        .and_then(Value::as_str)
        .is_some_and(|kind| kind != "human");
    let mut metadata = BTreeMap::new();
    if flag("isMeta") || flag("isCompactSummary") || flag("isVisibleInTranscriptOnly") || foreign_origin
    {
        metadata.insert(crate::alembic::RUNTIME_INJECTED.to_string(), Value::Bool(true));
    }
    metadata
}

/// A prompt the user typed while the agent was mid-turn: Claude Code files it
/// as an `attachment` record (`queued_command`, `commandMode: "prompt"`, from a
/// human) instead of a user record. It is the user's own words — carry it.
/// Background-task notices ride the same channel and are not. Claude sometimes
/// also files the same text as a regular user turn; the repeat is skipped.
fn import_queued_prompt(events: &mut Vec<SessionEvent>, value: &Value) {
    let Some(att) = value.get("attachment") else {
        return;
    };
    let human = att.get("humanTurn").and_then(Value::as_bool) == Some(true)
        || att
            .get("origin")
            .and_then(|o| o.get("kind"))
            .and_then(Value::as_str)
            == Some("human");
    if att.get("type").and_then(Value::as_str) != Some("queued_command")
        || att.get("commandMode").and_then(Value::as_str) != Some("prompt")
        || !human
    {
        return;
    }
    let Some(prompt) = att.get("prompt").and_then(Value::as_str) else {
        return;
    };
    let prompt = prompt.trim();
    if prompt.is_empty() {
        return;
    }
    // The last turn the USER wrote — runtime-injected turns (a skill body the
    // agent loaded in between, say) don't count.
    let repeats_last_user_turn = events.iter().rev().find_map(|e| match e {
        SessionEvent::Message(m)
            if m.role == "user" && !m.metadata.contains_key(crate::alembic::RUNTIME_INJECTED) =>
        {
            Some(
            m.blocks
                .iter()
                .filter_map(|b| b.text.as_deref())
                .collect::<String>()
                .trim()
                    == prompt,
            )
        }
        _ => None,
    });
    if repeats_last_user_turn == Some(true) {
        return;
    }
    let timestamp = att
        .get("timestamp")
        .or_else(|| value.get("timestamp"))
        .and_then(Value::as_str)
        .and_then(parse_datetime);
    events.push(SessionEvent::Message(MessageEvent {
        id: value.get("uuid").and_then(Value::as_str).map(str::to_string),
        parent_id: value
            .get("parentUuid")
            .and_then(Value::as_str)
            .map(str::to_string),
        role: "user".to_string(),
        timestamp,
        blocks: vec![ContentBlock::text("text", prompt)],
        metadata: BTreeMap::new(),
    }));
}

fn import_assistant_entry(events: &mut Vec<SessionEvent>, value: &Value) {
    let timestamp = value
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(parse_datetime);
    let uuid = value
        .get("uuid")
        .and_then(Value::as_str)
        .map(str::to_string);
    let parent_uuid = value
        .get("parentUuid")
        .and_then(Value::as_str)
        .map(str::to_string);
    let Some(message) = value.get("message") else {
        return;
    };

    let mut shared_metadata = BTreeMap::new();
    if let Some(model) = message.get("model") {
        shared_metadata.insert("model".to_string(), model.clone());
    }
    if let Some(stop_reason) = message.get("stop_reason") {
        shared_metadata.insert("stop_reason".to_string(), stop_reason.clone());
    }

    let content = message.get("content").and_then(Value::as_array).cloned();
    let Some(content) = content else {
        return;
    };

    let mut message_blocks = Vec::new();
    let mut reasoning_blocks = Vec::new();

    for (index, item) in content.into_iter().enumerate() {
        match item.get("type").and_then(Value::as_str) {
            Some("tool_use") => {
                flush_reasoning(
                    events,
                    &mut reasoning_blocks,
                    uuid.clone().map(|base| format!("{base}:reasoning:{index}")),
                    parent_uuid.clone(),
                    timestamp,
                    &shared_metadata,
                );
                flush_message(
                    events,
                    &mut message_blocks,
                    uuid.clone().map(|base| format!("{base}:msg:{index}")),
                    parent_uuid.clone(),
                    timestamp,
                    &shared_metadata,
                );

                import_tool_use_block(events, &item, timestamp, uuid.clone(), parent_uuid.clone());
            }
            Some("thinking") => {
                flush_message(
                    events,
                    &mut message_blocks,
                    uuid.clone().map(|base| format!("{base}:msg:{index}")),
                    parent_uuid.clone(),
                    timestamp,
                    &shared_metadata,
                );
                if let Some(text) = item.get("thinking").and_then(Value::as_str) {
                    reasoning_blocks.push(text.to_string());
                }
            }
            _ => {
                flush_reasoning(
                    events,
                    &mut reasoning_blocks,
                    uuid.clone().map(|base| format!("{base}:reasoning:{index}")),
                    parent_uuid.clone(),
                    timestamp,
                    &shared_metadata,
                );
                message_blocks.push(normalize_block(&item));
            }
        }
    }

    flush_reasoning(
        events,
        &mut reasoning_blocks,
        uuid.clone().map(|base| format!("{base}:reasoning")),
        parent_uuid.clone(),
        timestamp,
        &shared_metadata,
    );
    flush_message(
        events,
        &mut message_blocks,
        uuid,
        parent_uuid,
        timestamp,
        &shared_metadata,
    );
}

fn flush_message(
    events: &mut Vec<SessionEvent>,
    blocks: &mut Vec<ContentBlock>,
    id: Option<String>,
    parent_id: Option<String>,
    timestamp: Option<DateTime<Utc>>,
    metadata: &BTreeMap<String, Value>,
) {
    if blocks.is_empty() {
        return;
    }

    events.push(SessionEvent::Message(MessageEvent {
        id,
        parent_id,
        role: "assistant".to_string(),
        timestamp,
        blocks: std::mem::take(blocks),
        metadata: metadata.clone(),
    }));
}

fn flush_reasoning(
    events: &mut Vec<SessionEvent>,
    summary: &mut Vec<String>,
    id: Option<String>,
    parent_id: Option<String>,
    timestamp: Option<DateTime<Utc>>,
    metadata: &BTreeMap<String, Value>,
) {
    if summary.is_empty() {
        return;
    }

    events.push(SessionEvent::Reasoning(ReasoningEvent {
        id,
        parent_id,
        timestamp,
        summary: std::mem::take(summary),
        metadata: metadata.clone(),
    }));
}

fn import_tool_use_block(
    events: &mut Vec<SessionEvent>,
    item: &Value,
    timestamp: Option<DateTime<Utc>>,
    parent_id: Option<String>,
    source_parent: Option<String>,
) {
    let call_id = item
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let name = item
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    let arguments = item.get("input").cloned().unwrap_or(Value::Null);

    let mut metadata = BTreeMap::new();
    if let Some(caller) = item.get("caller") {
        metadata.insert("caller".to_string(), caller.clone());
    }

    events.push(SessionEvent::ToolCall(ToolCallEvent {
        id: parent_id,
        parent_id: source_parent,
        call_id,
        name,
        timestamp,
        arguments,
        metadata,
    }));
}

fn import_tool_result_block(
    events: &mut Vec<SessionEvent>,
    item: &Value,
    timestamp: Option<DateTime<Utc>>,
    event_id: Option<String>,
    parent_id: Option<String>,
) {
    let output = item.get("content").cloned().unwrap_or(Value::Null);
    let is_error = item
        .get("is_error")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    events.push(SessionEvent::ToolResult(ToolResultEvent {
        id: event_id,
        parent_id,
        call_id: item
            .get("tool_use_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        timestamp,
        output,
        is_error,
        metadata: BTreeMap::new(),
    }));
}

pub fn write(session: &UniversalSession, output: &Path) -> Result<PathBuf> {
    let materialization = plan_output(session, output);
    if let Some(parent) = materialization.session_file.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    let session_id = claude_session_id(&session.metadata.session_id);
    let cwd = session
        .metadata
        .cwd
        .clone()
        .unwrap_or_else(|| PathBuf::from("."));
    let git_branch = session
        .metadata
        .git_branch
        .clone()
        .unwrap_or_else(|| "HEAD".to_string());
    let created_at = session.metadata.created_at.or_else(|| {
        session
            .events
            .iter()
            .filter_map(SessionEvent::timestamp)
            .min()
    });
    // Use the real Claude version when known (alembic sets platform_version to
    // the installed `claude --version`). A foreign/old `version` makes
    // `claude -r` reject the session as incompatible.
    let version = session
        .metadata
        .platform_version
        .clone()
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());
    let model = CARRIED_MODEL;

    // Atomic materialization (B1): the projection may be the only copy of the
    // conversation's newest turns, so never truncate it in place. Write a
    // sibling tmp file, fsync, then rename over the target — a crash mid-write
    // leaves the previous projection intact.
    let tmp_path = super::tmp_sibling(&materialization.session_file);
    let mut tmp_guard = super::TmpCleanup::new(&tmp_path);
    let mut file = File::create(&tmp_path).with_context(|| {
        format!(
            "failed to create Claude session {}",
            materialization.session_file.display()
        )
    })?;

    // Leading meta record that real Claude sessions begin with.
    write_json_line(
        &mut file,
        &json!({
            "type": "permission-mode",
            "permissionMode": "default",
            "sessionId": session_id,
        }),
    )?;

    let ctx = EmitCtx {
        session_id: &session_id,
        cwd: cwd.as_path(),
        git_branch: &git_branch,
        version: &version,
        model,
    };
    let mut previous_uuid: Option<String> = None;
    let mut tool_call_to_uuid = BTreeMap::new();
    for event in &session.events {
        emit_event_line(&mut file, event, &ctx, &mut previous_uuid, &mut tool_call_to_uuid)?;
    }

    file.sync_all()
        .with_context(|| format!("failed to flush {}", tmp_path.display()))?;
    drop(file);
    fs::rename(&tmp_path, &materialization.session_file).with_context(|| {
        format!(
            "failed to move session into place at {}",
            materialization.session_file.display()
        )
    })?;
    tmp_guard.keep();

    if let Some(history_file) = &materialization.history_file {
        if let Some(parent) = history_file.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }

        let mut history = OpenOptions::new()
            .create(true)
            .append(true)
            .open(history_file)
            .with_context(|| format!("failed to open {}", history_file.display()))?;
        write_json_line(
            &mut history,
            &json!({
                "display": derive_title(session).unwrap_or_else(|| "Imported session".to_string()),
                "pastedContents": {},
                "timestamp": created_at
                    .unwrap_or_else(Utc::now)
                    .timestamp_millis(),
                "project": cwd.display().to_string(),
                "sessionId": session_id,
            }),
        )?;
    }

    Ok(materialization.session_file)
}

/// The per-session context every emitted line shares. Cheap (all borrowed),
/// so both `write` (whole session) and `append` (a delta) hand the same value
/// to [`emit_event_line`] and the line shape can't drift between them.
#[derive(Clone, Copy)]
struct EmitCtx<'a> {
    session_id: &'a str,
    cwd: &'a Path,
    git_branch: &'a str,
    version: &'a str,
    model: &'a str,
}

/// Emit one IR event as a Claude JSONL line, chaining `parentUuid` from
/// `previous_uuid` and advancing it. Factored out of `write` so `append` reuses
/// the exact same chaining and line shape.
fn emit_event_line(
    file: &mut File,
    event: &SessionEvent,
    ctx: &EmitCtx,
    previous_uuid: &mut Option<String>,
    tool_call_to_uuid: &mut BTreeMap<String, String>,
) -> Result<()> {
    let EmitCtx {
        session_id,
        cwd,
        git_branch,
        version,
        model,
    } = *ctx;
    let parent = previous_uuid.clone();
    match event {
        SessionEvent::Message(message) => {
            let event_uuid = Uuid::new_v4().to_string();
            let (projected_role, projected_blocks) = project_message_for_claude(message);
            let content = encode_message_blocks(&projected_blocks);
            if content.is_null() {
                return Ok(());
            }

            let line = if projected_role == "assistant" {
                let assistant_message = claude_assistant_message(model, content, Value::Null);
                json!({
                    "parentUuid": parent,
                    "isSidechain": false,
                    "userType": "external",
                    "cwd": cwd,
                    "sessionId": session_id,
                    "version": version,
                    "gitBranch": git_branch,
                    "entrypoint": "cli",
                    "requestId": format!("req_{}", Uuid::new_v4().simple()),
                    "message": assistant_message,
                    "type": "assistant",
                    "uuid": event_uuid,
                    "timestamp": event_timestamp(message.timestamp),
                })
            } else {
                json!({
                    "parentUuid": parent,
                    "isSidechain": false,
                    "userType": "external",
                    "cwd": cwd,
                    "sessionId": session_id,
                    "version": version,
                    "gitBranch": git_branch,
                    "entrypoint": "cli",
                    "type": "user",
                    "message": {
                        "role": "user",
                        "content": user_text_content(&projected_blocks),
                    },
                    "uuid": event_uuid,
                    "timestamp": event_timestamp(message.timestamp),
                })
            };

            write_json_line(file, &line)?;
            *previous_uuid = line.get("uuid").and_then(Value::as_str).map(str::to_string);
        }
        SessionEvent::Reasoning(reasoning) => {
            let event_uuid = Uuid::new_v4().to_string();
            let content = reasoning
                .summary
                .iter()
                .map(|text| {
                    json!({
                        "type": "thinking",
                        "thinking": text,
                    })
                })
                .collect::<Vec<_>>();
            let assistant_message =
                claude_assistant_message(model, Value::Array(content), Value::Null);
            let line = json!({
                "parentUuid": parent,
                "isSidechain": false,
                "userType": "external",
                "cwd": cwd,
                "sessionId": session_id,
                "version": version,
                "gitBranch": git_branch,
                "entrypoint": "cli",
                "requestId": format!("req_{}", Uuid::new_v4().simple()),
                "message": assistant_message,
                "type": "assistant",
                "uuid": event_uuid,
                "timestamp": event_timestamp(reasoning.timestamp),
            });
            write_json_line(file, &line)?;
            *previous_uuid = line.get("uuid").and_then(Value::as_str).map(str::to_string);
        }
        SessionEvent::ToolCall(call) => {
            let event_uuid = Uuid::new_v4().to_string();
            // The API rejects a non-object `tool_use.input` on resume; codex
            // freeform tools carry a raw string, so wrap anything else.
            let input = match &call.arguments {
                Value::Object(_) => call.arguments.clone(),
                Value::Null => json!({}),
                other => json!({ "input": other }),
            };
            let assistant_message = claude_assistant_message(
                model,
                json!([{
                    "type": "tool_use",
                    "id": call.call_id,
                    "name": call.name,
                    "input": input,
                    "caller": { "type": "direct" },
                }]),
                Value::String("tool_use".to_string()),
            );
            let line = json!({
                "parentUuid": parent,
                "isSidechain": false,
                "userType": "external",
                "cwd": cwd,
                "sessionId": session_id,
                "version": version,
                "gitBranch": git_branch,
                "entrypoint": "cli",
                "requestId": format!("req_{}", Uuid::new_v4().simple()),
                "message": assistant_message,
                "type": "assistant",
                "uuid": event_uuid,
                "timestamp": event_timestamp(call.timestamp),
            });
            write_json_line(file, &line)?;
            tool_call_to_uuid.insert(call.call_id.clone(), event_uuid.clone());
            *previous_uuid = Some(event_uuid);
        }
        SessionEvent::ToolResult(result) => {
            let event_uuid = Uuid::new_v4().to_string();
            let source_uuid = tool_call_to_uuid.get(&result.call_id).cloned();
            let line = json!({
                "parentUuid": parent,
                "isSidechain": false,
                "userType": "external",
                "cwd": cwd,
                "sessionId": session_id,
                "version": version,
                "gitBranch": git_branch,
                "entrypoint": "cli",
                "type": "user",
                "message": {
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": result.call_id,
                        "content": encode_tool_result_output(&result.output),
                        "is_error": result.is_error,
                    }]
                },
                "uuid": event_uuid,
                "timestamp": event_timestamp(result.timestamp),
                "toolUseResult": tool_result_summary(&result.output, result.is_error),
                "sourceToolAssistantUUID": source_uuid,
            });
            write_json_line(file, &line)?;
            *previous_uuid = Some(event_uuid);
        }
    }
    Ok(())
}

/// Append `delta`'s events to an existing Claude projection, preserving every
/// existing line byte-for-byte (the return-switch append-refresh path). The
/// existing file's `sessionId`/`cwd`/`version`/`gitBranch` are reused so
/// `claude -r` still accepts the session, and the appended lines chain their
/// `parentUuid` from the last existing line's `uuid`. Atomic (tmp + rename), so
/// a torn append never replaces the good file.
pub fn append(existing: &Path, delta: &UniversalSession) -> Result<PathBuf> {
    let body = fs::read_to_string(existing)
        .with_context(|| format!("failed to read Claude session {}", existing.display()))?;
    let lines: Vec<&str> = body.lines().filter(|l| !l.trim().is_empty()).collect();

    // Recover the projection's own identity from its existing lines — never
    // mint a fresh sessionId or borrow the source's.
    let field = |key: &str| -> Option<String> {
        lines.iter().rev().find_map(|l| {
            serde_json::from_str::<Value>(l)
                .ok()
                .and_then(|v| v.get(key).and_then(Value::as_str).map(str::to_string))
        })
    };
    let session_id = field("sessionId")
        .with_context(|| format!("Claude session {} has no sessionId", existing.display()))?;
    let cwd = PathBuf::from(field("cwd").unwrap_or_else(|| ".".to_string()));
    let git_branch = field("gitBranch").unwrap_or_else(|| "HEAD".to_string());
    let version = field("version").unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());
    let model = CARRIED_MODEL;
    // Chain from the last line that actually carries a uuid — a Claude
    // projection's final line is often a non-turn record (the custom-title), and
    // chaining `parentUuid` to it would orphan the appended turns.
    let mut previous_uuid = lines.iter().rev().find_map(|l| {
        serde_json::from_str::<Value>(l)
            .ok()
            .and_then(|v| v.get("uuid").and_then(Value::as_str).map(str::to_string))
    });

    let ctx = EmitCtx {
        session_id: &session_id,
        cwd: cwd.as_path(),
        git_branch: &git_branch,
        version: &version,
        model,
    };

    // Atomic append: copy the existing bytes verbatim, then emit the delta — the
    // existing projection is never truncated in place (same B1 law as `write`).
    let tmp_path = super::tmp_sibling(existing);
    let mut tmp_guard = super::TmpCleanup::new(&tmp_path);
    let mut file = File::create(&tmp_path)
        .with_context(|| format!("failed to create {}", tmp_path.display()))?;
    file.write_all(body.as_bytes())
        .with_context(|| format!("failed to write {}", tmp_path.display()))?;
    if !body.ends_with('\n') {
        file.write_all(b"\n")?;
    }
    let mut tool_call_to_uuid = BTreeMap::new();
    for event in &delta.events {
        emit_event_line(&mut file, event, &ctx, &mut previous_uuid, &mut tool_call_to_uuid)?;
    }
    file.sync_all()
        .with_context(|| format!("failed to flush {}", tmp_path.display()))?;
    drop(file);
    fs::rename(&tmp_path, existing).with_context(|| {
        format!("failed to move session into place at {}", existing.display())
    })?;
    tmp_guard.keep();
    Ok(existing.to_path_buf())
}

fn plan_output(session: &UniversalSession, output: &Path) -> ClaudeMaterialization {
    if output.extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
        return ClaudeMaterialization {
            session_file: output.to_path_buf(),
            history_file: None,
        };
    }

    let cwd = session
        .metadata
        .cwd
        .as_deref()
        .unwrap_or_else(|| Path::new("."));
    let slug = path_to_claude_slug(cwd);
    let session_id = claude_session_id(&session.metadata.session_id);
    ClaudeMaterialization {
        session_file: output
            .join("projects")
            .join(slug)
            .join(format!("{session_id}.jsonl")),
        history_file: Some(output.join("history.jsonl")),
    }
}

/// The `projects/<name>` directory Claude Code files a cwd under — Claude's
/// own rule (2.1.2xx, read from the binary): every UTF-16 unit that is not
/// `[A-Za-z0-9]` becomes `-`; a name longer than 200 is cut to 200 and suffixed
/// with `-` + base36(|h|), `h` being the 32-bit `(h << 5) - h + unit` hash of
/// the raw path. (A character outside the BMP is two UTF-16 units, so two
/// dashes — not one.)
pub(crate) fn path_to_claude_slug(path: &Path) -> String {
    const MAX: usize = 200;
    let rendered = path.to_string_lossy();
    let units: Vec<u16> = rendered.encode_utf16().collect();
    let mut slug: String = units
        .iter()
        .map(|&u| {
            if u < 128 && (u as u8).is_ascii_alphanumeric() {
                u as u8 as char
            } else {
                '-'
            }
        })
        .collect();
    if !slug.starts_with('-') {
        slug.insert(0, '-');
    }
    if slug.len() <= MAX {
        return slug;
    }
    let hash = units.iter().fold(0i32, |h, &u| {
        h.wrapping_shl(5).wrapping_sub(h).wrapping_add(i32::from(u))
    });
    format!("{}-{}", &slug[..MAX], base36(hash.unsigned_abs()))
}

fn base36(mut n: u32) -> String {
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if n == 0 {
        return "0".to_string();
    }
    let mut out = Vec::new();
    while n > 0 {
        out.push(DIGITS[(n % 36) as usize]);
        n /= 36;
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

fn encode_message_blocks(blocks: &[ContentBlock]) -> Value {
    if blocks.is_empty() {
        return Value::Null;
    }

    let encoded = blocks
        .iter()
        .map(|block| {
            let mut object = Map::new();
            object.insert(
                "type".to_string(),
                Value::String(claude_block_kind(&block.kind).to_string()),
            );
            if let Some(text) = &block.text {
                let text_key = if block.kind == "thinking" {
                    "thinking"
                } else {
                    "text"
                };
                object.insert(text_key.to_string(), Value::String(text.clone()));
            }
            if let Some(data) = &block.data {
                if let Value::Object(extra) = data {
                    object.extend(extra.clone());
                } else {
                    object.insert("data".to_string(), data.clone());
                }
            }
            Value::Object(object)
        })
        .collect::<Vec<_>>();
    Value::Array(encoded)
}

/// Real Claude user turns store the prompt as a plain string, not a blocks
/// array. After alembic's sanitize, user messages are text-only, so emit a
/// string to match the native shape `claude -r` expects.
fn user_text_content(blocks: &[ContentBlock]) -> Value {
    if !blocks.is_empty() && blocks.iter().all(|b| b.text.is_some()) {
        let text = blocks
            .iter()
            .filter_map(|b| b.text.clone())
            .collect::<Vec<_>>()
            .join("\n");
        Value::String(text)
    } else {
        encode_message_blocks(blocks)
    }
}

fn claude_assistant_message(model: &str, content: Value, stop_reason: Value) -> Value {
    let mut message = Map::new();
    message.insert(
        "id".to_string(),
        Value::String(format!("msg_{}", Uuid::new_v4().simple())),
    );
    message.insert("type".to_string(), Value::String("message".to_string()));
    message.insert("role".to_string(), Value::String("assistant".to_string()));
    message.insert("model".to_string(), Value::String(model.to_string()));
    message.insert("content".to_string(), content);
    message.insert("stop_reason".to_string(), stop_reason);
    message.insert("stop_sequence".to_string(), Value::Null);
    message.insert("stop_details".to_string(), Value::Null);
    message.insert(
        "usage".to_string(),
        json!({
            "input_tokens": 0,
            "output_tokens": 0,
            "cache_creation_input_tokens": 0,
            "cache_read_input_tokens": 0,
            "service_tier": null,
        }),
    );
    Value::Object(message)
}

fn encode_tool_result_output(output: &Value) -> Value {
    match output {
        Value::Array(_) | Value::Object(_) => output.clone(),
        Value::String(text) => Value::String(text.clone()),
        other => Value::String(other.to_string()),
    }
}

fn tool_result_summary(output: &Value, is_error: bool) -> Value {
    if is_error {
        return Value::String(json_to_string(output));
    }

    match output {
        Value::String(text) => json!({
            "stdout": text,
            "stderr": "",
            "interrupted": false,
            "isImage": false,
            "noOutputExpected": false,
        }),
        other => json!({
            "value": other,
        }),
    }
}

fn normalize_block(value: &Value) -> ContentBlock {
    let kind = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("text")
        .to_string();
    let text = ["text", "thinking", "content"]
        .iter()
        .find_map(|key| value.get(key).and_then(Value::as_str))
        .map(str::to_string);
    let mut object = value.as_object().cloned().unwrap_or_default();
    object.remove("type");
    object.remove("text");
    object.remove("thinking");
    object.remove("content");
    let data = (!object.is_empty()).then_some(Value::Object(object));

    ContentBlock { kind, text, data }
}

fn parse_datetime(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

fn event_timestamp(timestamp: Option<DateTime<Utc>>) -> String {
    timestamp
        .unwrap_or_else(Utc::now)
        .to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn write_json_line(file: &mut impl Write, value: &Value) -> Result<()> {
    serde_json::to_writer(&mut *file, value).context("failed to encode JSONL line")?;
    file.write_all(b"\n").context("failed to write newline")
}

fn update_time_bounds(metadata: &mut SessionMetadata, timestamp: Option<DateTime<Utc>>) {
    let Some(timestamp) = timestamp else {
        return;
    };
    metadata.created_at = Some(match metadata.created_at {
        Some(current) => current.min(timestamp),
        None => timestamp,
    });
    metadata.updated_at = Some(match metadata.updated_at {
        Some(current) => current.max(timestamp),
        None => timestamp,
    });
}

fn derive_title(session: &UniversalSession) -> Option<String> {
    session.events.iter().find_map(|event| {
        let SessionEvent::Message(message) = event else {
            return None;
        };
        if message.role != "user" || message.metadata.contains_key(crate::alembic::RUNTIME_INJECTED) {
            return None;
        }
        message
            .blocks
            .iter()
            .filter_map(|block| block.text.as_deref())
            .filter(|text| !crate::alembic::is_scaffold(text))
            .map(|text| collapse_whitespace(crate::alembic::unwrap_paste(text)))
            .find(|text| !text.is_empty())
    })
}

fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(80)
        .collect()
}

fn claude_session_id(candidate: &str) -> String {
    Uuid::parse_str(candidate)
        .map(|uuid| uuid.to_string())
        .unwrap_or_else(|_| Uuid::new_v4().to_string())
}

fn json_to_string(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => serde_json::to_string(other).unwrap_or_else(|_| other.to_string()),
    }
}

fn project_message_for_claude(message: &MessageEvent) -> (&'static str, Vec<ContentBlock>) {
    match message.role.as_str() {
        "assistant" => ("assistant", message.blocks.clone()),
        "user" => ("user", message.blocks.clone()),
        other => {
            let mut blocks = message.blocks.clone();
            let prefix = format!("[transession imported {other} message]");
            match blocks.first_mut() {
                Some(block) if block.text.is_some() => {
                    let text = block.text.take().unwrap_or_default();
                    block.text = Some(format!("{prefix}\n{text}"));
                }
                _ => blocks.insert(0, ContentBlock::text("text", prefix)),
            }
            ("user", blocks)
        }
    }
}

fn claude_block_kind(kind: &str) -> &'static str {
    match kind {
        "thinking" => "thinking",
        "tool_use" => "tool_use",
        "tool_result" => "tool_result",
        "input_text" | "output_text" | "text" => "text",
        _ => "text",
    }
}
