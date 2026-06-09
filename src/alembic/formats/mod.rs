mod claude;
mod codex;

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::alembic::ir::{SessionFormat, SourceFormat, UniversalSession};

#[derive(Debug)]
pub struct ResolvedInput {
    pub path: PathBuf,
    pub format: SessionFormat,
}

/// Sibling path used for atomic materialization: write here, fsync, then
/// rename over the real target. The name never ends in `.jsonl`, so neither
/// the runtimes' own session scanners nor ours can pick a half-written file.
pub(crate) fn tmp_sibling(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "session".to_string());
    path.with_file_name(format!(".{name}.constant-tmp"))
}

/// Removes the tmp file on drop unless `keep()` was called (the rename
/// happened), so a failed write never leaves debris in the store.
pub(crate) struct TmpCleanup {
    path: PathBuf,
    keep: bool,
}

impl TmpCleanup {
    pub(crate) fn new(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
            keep: false,
        }
    }

    pub(crate) fn keep(&mut self) {
        self.keep = true;
    }
}

impl Drop for TmpCleanup {
    fn drop(&mut self) {
        if !self.keep {
            let _ = fs::remove_file(&self.path);
        }
    }
}

pub fn detect_format(path: &Path) -> Result<SessionFormat> {
    use std::io::{BufRead, BufReader};

    // The JSONL formats (codex/claude) are decided by the first non-empty line
    // alone — don't read a potentially huge transcript just to classify it.
    let file = fs::File::open(path).with_context(|| {
        format!(
            "failed to read input for format detection: {}",
            path.display()
        )
    })?;
    let mut reader = BufReader::new(file);
    let mut first = String::new();
    loop {
        first.clear();
        let n = reader
            .read_line(&mut first)
            .with_context(|| format!("failed to read {}", path.display()))?;
        if n == 0 {
            bail!("input file is empty: {}", path.display());
        }
        if !first.trim().is_empty() {
            break;
        }
    }

    if let Ok(value) = serde_json::from_str::<Value>(first.trim()) {
        if value.get("ir_version").is_some() {
            return Ok(SessionFormat::Ir);
        }
        if matches!(
            value.get("type").and_then(Value::as_str),
            Some("session_meta")
        ) {
            return Ok(SessionFormat::Codex);
        }
        if value.get("sessionId").is_some() {
            return Ok(SessionFormat::Claude);
        }
    }

    // Not a recognizable JSONL first line — it may be a pretty-printed IR file
    // (a single whole-file JSON document). Only now read the full file.
    let text = fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    if let Ok(value) = serde_json::from_str::<Value>(&text)
        && value.get("ir_version").is_some()
    {
        return Ok(SessionFormat::Ir);
    }

    bail!("could not detect format for {}", path.display())
}

pub fn resolve_input(path: &Path, format: SourceFormat) -> Result<ResolvedInput> {
    if path.exists() {
        let resolved_format = match format.explicit() {
            Some(format) => format,
            None => detect_format(path)?,
        };
        return Ok(ResolvedInput {
            path: path.to_path_buf(),
            format: resolved_format,
        });
    }

    let session_id = path.to_string_lossy().trim().to_string();
    if session_id.is_empty() {
        bail!("input path is empty");
    }

    match format.explicit() {
        Some(SessionFormat::Ir) => bail!(
            "IR input must be addressed by file path; session-id lookup only works for Codex and Claude"
        ),
        Some(SessionFormat::Codex) => {
            resolve_codex_session_id(&session_id).map(|path| ResolvedInput {
                path,
                format: SessionFormat::Codex,
            })
        }
        Some(SessionFormat::Claude) => {
            resolve_claude_session_id(&session_id).map(|path| ResolvedInput {
                path,
                format: SessionFormat::Claude,
            })
        }
        None => {
            let codex = resolve_codex_session_id(&session_id).ok();
            let claude = resolve_claude_session_id(&session_id).ok();
            match (codex, claude) {
                (Some(path), None) => Ok(ResolvedInput {
                    path,
                    format: SessionFormat::Codex,
                }),
                (None, Some(path)) => Ok(ResolvedInput {
                    path,
                    format: SessionFormat::Claude,
                }),
                (Some(_), Some(_)) => bail!(
                    "session id {session_id} exists in both Codex and Claude stores; specify --from"
                ),
                (None, None) => bail!(
                    "could not resolve {session_id} as a path or native session id in the default Codex/Claude stores"
                ),
            }
        }
    }
}

pub fn load_session(path: &Path, format: SourceFormat) -> Result<UniversalSession> {
    let resolved = resolve_input(path, format)?;
    match resolved.format {
        SessionFormat::Ir => load_ir(&resolved.path),
        SessionFormat::Codex => codex::load(&resolved.path),
        SessionFormat::Claude => claude::load(&resolved.path),
    }
}

pub fn write_ir(session: &UniversalSession, output: &Path) -> Result<()> {
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!("failed to create parent directory for {}", output.display())
        })?;
    }

    let text = serde_json::to_string_pretty(session).context("failed to encode IR JSON")?;
    fs::write(output, text).with_context(|| format!("failed to write {}", output.display()))
}

pub fn load_ir(path: &Path) -> Result<UniversalSession> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("failed to read IR file {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("failed to parse {}", path.display()))
}

pub fn materialize(
    session: &UniversalSession,
    target: SessionFormat,
    output: &Path,
) -> Result<PathBuf> {
    match target {
        SessionFormat::Ir => {
            write_ir(session, output)?;
            Ok(output.to_path_buf())
        }
        SessionFormat::Codex => codex::write(session, output),
        SessionFormat::Claude => claude::write(session, output),
    }
}

pub fn default_output_root(target: SessionFormat) -> Result<PathBuf> {
    match target {
        SessionFormat::Codex => codex_root(),
        SessionFormat::Claude => claude_root(),
        SessionFormat::Ir => bail!("IR output requires an explicit file path"),
    }
}

pub(crate) fn resolve_codex_session_id(session_id: &str) -> Result<PathBuf> {
    let root = codex_root()?;
    let sessions_root = root.join("sessions");
    let suffix = format!("-{session_id}.jsonl");
    find_newest_in_tree(&sessions_root, |path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .map(|name| name.ends_with(&suffix))
            .unwrap_or(false)
    })
    .with_context(|| {
        format!(
            "could not find Codex session {session_id} under {}",
            sessions_root.display()
        )
    })
}

pub(crate) fn resolve_claude_session_id(session_id: &str) -> Result<PathBuf> {
    let root = claude_root()?;
    let projects_root = root.join("projects");
    let name_want = format!("{session_id}.jsonl");
    find_newest_in_tree(&projects_root, |path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .map(|name| name == name_want)
            .unwrap_or(false)
    })
    .with_context(|| {
        format!(
            "could not find Claude session {session_id} under {}",
            projects_root.display()
        )
    })
}

fn codex_root() -> Result<PathBuf> {
    discover_root("TRANSESSION_CODEX_HOME", &["CODEX_HOME"], ".codex")
}

fn claude_root() -> Result<PathBuf> {
    discover_root(
        "TRANSESSION_CLAUDE_HOME",
        &["CLAUDE_CONFIG_DIR", "CLAUDE_HOME"],
        ".claude",
    )
}

fn discover_root(primary_env: &str, secondary_envs: &[&str], suffix: &str) -> Result<PathBuf> {
    if let Some(path) = env_path(primary_env) {
        return Ok(path);
    }
    for env_name in secondary_envs {
        if let Some(path) = env_path(env_name) {
            return Ok(path);
        }
    }
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(suffix))
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).map(PathBuf::from)
}

/// Newest-mtime match wins: duplicate files for the same session id can exist
/// (an old fork bug left some), and resuming the stale one silently loses the
/// freshest turns. Directory recursion checks `entry.file_type()` — which does
/// NOT follow symlinks — so a symlink cycle in a store can't hang the walk.
fn find_newest_in_tree<F>(root: &Path, predicate: F) -> Result<PathBuf>
where
    F: Fn(&Path) -> bool + Copy,
{
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                stack.push(path);
            } else if predicate(&path) {
                let mtime = entry
                    .metadata()
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                if best.as_ref().map(|(t, _)| mtime >= *t).unwrap_or(true) {
                    best = Some((mtime, path));
                }
            }
        }
    }

    best.map(|(_, path)| path)
        .with_context(|| format!("could not find a matching session under {}", root.display()))
}
