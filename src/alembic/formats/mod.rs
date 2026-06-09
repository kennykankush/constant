mod claude;
mod codex;
pub(crate) mod gemini;
pub(crate) mod opencode;

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
        if let Some(found) = classify_document(&value) {
            return Ok(found);
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

    // Not a recognizable JSONL first line — it may be a pretty-printed
    // whole-file document (IR, gemini, or an opencode export). Read it all.
    let text = fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let end = text.rfind('}').map(|i| i + 1).unwrap_or(text.len());
    if let Ok(value) = serde_json::from_str::<Value>(&text[..end])
        && let Some(found) = classify_document(&value)
    {
        return Ok(found);
    }

    bail!("could not detect format for {}", path.display())
}

/// Classify a whole-document JSON session: neutral IR, a gemini session
/// (`sessionId` + `projectHash` + `messages` — checked BEFORE claude, whose
/// records also carry a bare `sessionId`), or an opencode export
/// (`info.id = ses_…` + `messages`).
fn classify_document(value: &Value) -> Option<SessionFormat> {
    if value.get("ir_version").is_some() {
        return Some(SessionFormat::Ir);
    }
    if value.get("sessionId").is_some()
        && value.get("projectHash").is_some()
        && value.get("messages").is_some()
    {
        return Some(SessionFormat::Gemini);
    }
    if value.get("messages").is_some()
        && value
            .get("info")
            .and_then(|i| i.get("id"))
            .and_then(Value::as_str)
            .map(|id| id.starts_with("ses_"))
            .unwrap_or(false)
    {
        return Some(SessionFormat::OpenCode);
    }
    None
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
        Some(SessionFormat::Gemini) => {
            resolve_gemini_session_id(&session_id).map(|path| ResolvedInput {
                path,
                format: SessionFormat::Gemini,
            })
        }
        Some(SessionFormat::OpenCode) => {
            export_opencode_to_cache(&session_id).map(|path| ResolvedInput {
                path,
                format: SessionFormat::OpenCode,
            })
        }
        None => {
            // OpenCode ids are unmistakable; resolve them via export (their
            // supported read door) without scanning anyone's file stores.
            if session_id.starts_with("ses_") {
                return export_opencode_to_cache(&session_id).map(|path| ResolvedInput {
                    path,
                    format: SessionFormat::OpenCode,
                });
            }
            let mut hits: Vec<ResolvedInput> = Vec::new();
            if let Ok(path) = resolve_codex_session_id(&session_id) {
                hits.push(ResolvedInput {
                    path,
                    format: SessionFormat::Codex,
                });
            }
            if let Ok(path) = resolve_claude_session_id(&session_id) {
                hits.push(ResolvedInput {
                    path,
                    format: SessionFormat::Claude,
                });
            }
            if let Ok(path) = resolve_gemini_session_id(&session_id) {
                hits.push(ResolvedInput {
                    path,
                    format: SessionFormat::Gemini,
                });
            }
            match hits.len() {
                1 => Ok(hits.remove(0)),
                0 => bail!(
                    "could not resolve {session_id} as a path or native session id in the default stores"
                ),
                _ => bail!(
                    "session id {session_id} exists in more than one runtime's store; pass the file path"
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
        SessionFormat::Gemini => gemini::load(&resolved.path),
        SessionFormat::OpenCode => opencode::load(&resolved.path),
    }
}

pub fn write_ir(session: &UniversalSession, output: &Path) -> Result<()> {
    use std::io::Write;

    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!("failed to create parent directory for {}", output.display())
        })?;
    }

    let text = serde_json::to_string_pretty(session).context("failed to encode IR JSON")?;
    // Atomic, like every other materialization: an IR file is either the
    // previous complete document or the new one, never a torn half.
    let tmp = tmp_sibling(output);
    let mut guard = TmpCleanup::new(&tmp);
    let mut file = fs::File::create(&tmp)
        .with_context(|| format!("failed to create {}", tmp.display()))?;
    file.write_all(text.as_bytes())
        .with_context(|| format!("failed to write {}", tmp.display()))?;
    file.sync_all()
        .with_context(|| format!("failed to flush {}", tmp.display()))?;
    drop(file);
    fs::rename(&tmp, output)
        .with_context(|| format!("failed to move {} into place", output.display()))?;
    guard.keep();
    Ok(())
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
        SessionFormat::Gemini => bail!(
            "carrying INTO gemini isn't supported yet — gemini works as a carry source              (the writer lands after one live landing-pad verification)"
        ),
        SessionFormat::OpenCode => opencode::write(session, output),
    }
}

pub fn default_output_root(target: SessionFormat) -> Result<PathBuf> {
    match target {
        SessionFormat::Codex => codex_root(),
        SessionFormat::Claude => claude_root(),
        SessionFormat::Gemini => gemini_root(),
        SessionFormat::OpenCode => opencode_data_root(),
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

pub(crate) fn gemini_root() -> Result<PathBuf> {
    discover_root("CONSTANT_GEMINI_HOME", &[], ".gemini")
}

/// OpenCode's data root (the sqlite store lives here). Respects
/// `XDG_DATA_HOME` exactly like opencode itself (verified — this is also how
/// the tests isolate).
pub(crate) fn opencode_data_root() -> Result<PathBuf> {
    if let Some(xdg) = env_path("XDG_DATA_HOME") {
        return Ok(xdg.join("opencode"));
    }
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home)
        .join(".local")
        .join("share")
        .join("opencode"))
}

/// Constant's file-shaped shadow of an opencode db session: opencode sessions
/// aren't files, but the whole carry machinery is path-centric, so reads go
/// through a cache copy under `~/.constant/cache/opencode/<id>.json`
/// (refreshed on every access; never authoritative).
pub(crate) fn opencode_cache_path(id: &str) -> Result<PathBuf> {
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    let safe: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let dir = PathBuf::from(home)
        .join(".constant")
        .join("cache")
        .join("opencode");
    fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o700));
    }
    Ok(dir.join(format!("{safe}.json")))
}

/// Refresh the cache copy of an opencode session via `opencode export`
/// (their supported read door; stdout carries a status line after the JSON).
pub(crate) fn export_opencode_to_cache(id: &str) -> Result<PathBuf> {
    let out = std::process::Command::new("opencode")
        .args(["export", id])
        .output()
        .context("failed to run `opencode export` — is opencode on PATH?")?;
    let text = String::from_utf8_lossy(&out.stdout);
    if !out.status.success() {
        bail!(
            "opencode export {id} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let end = text
        .rfind('}')
        .with_context(|| format!("opencode export {id} produced no JSON"))?
        + 1;

    let path = opencode_cache_path(id)?;
    let tmp = tmp_sibling(&path);
    let mut guard = TmpCleanup::new(&tmp);
    fs::write(&tmp, &text[..end]).with_context(|| format!("failed to write {}", tmp.display()))?;
    fs::rename(&tmp, &path)
        .with_context(|| format!("failed to move {} into place", path.display()))?;
    guard.keep();
    Ok(path)
}

/// Find a gemini session file by id: filename stem == id (session-dir layout)
/// or the `session-<ts>-<id8>` convention; only files under a `chats` tree.
pub(crate) fn resolve_gemini_session_id(session_id: &str) -> Result<PathBuf> {
    let root = gemini_root()?.join("tmp");
    let short: String = session_id.chars().take(8).collect();
    find_newest_in_tree(&root, |path| {
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            return false;
        };
        if !(name.ends_with(".json") || name.ends_with(".jsonl")) {
            return false;
        }
        if !under_chats(path) {
            return false;
        }
        let stem = name.trim_end_matches(".jsonl").trim_end_matches(".json");
        stem == session_id || (!short.is_empty() && stem.ends_with(&format!("-{short}")))
    })
    .with_context(|| format!("could not find Gemini session {session_id} under {}", root.display()))
}

/// True when a file lives inside a `chats/` tree (directly, or one level deep
/// for the per-session-directory layout) — keeps `logs.json` etc. out.
pub(crate) fn under_chats(path: &Path) -> bool {
    let mut anc = path.parent();
    for _ in 0..2 {
        if let Some(dir) = anc {
            if dir.file_name().and_then(|n| n.to_str()) == Some("chats") {
                return true;
            }
            anc = dir.parent();
        }
    }
    false
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
