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
pub(crate) mod sha256;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use uuid::Uuid;

use crate::runtime::Runtime;
use ir::{SessionEvent, SessionFormat, SourceFormat, UniversalSession};

/// Identify a session file: which runtime it belongs to and its native id.
/// Used by the headless `carry` to label the trail correctly for any source.
/// A neutral IR file is accepted when it records its origin runtime in
/// `source_format` (so an exported master can be re-hydrated into a runtime).
pub fn identify(path: &Path) -> Option<(Runtime, String)> {
    let fmt = formats::detect_format(path).ok()?;
    match fmt {
        SessionFormat::Codex => Some((Runtime::Codex, session_id_of(path, fmt)?)),
        SessionFormat::Claude => Some((Runtime::Claude, session_id_of(path, fmt)?)),
        SessionFormat::Gemini => Some((Runtime::Gemini, session_id_of(path, fmt)?)),
        SessionFormat::OpenCode => Some((Runtime::OpenCode, session_id_of(path, fmt)?)),
        SessionFormat::Ir => {
            let session = formats::load_ir(path).ok()?;
            let runtime = match session.metadata.source_format? {
                SessionFormat::Codex => Runtime::Codex,
                SessionFormat::Claude => Runtime::Claude,
                SessionFormat::Gemini => Runtime::Gemini,
                SessionFormat::OpenCode => Runtime::OpenCode,
                SessionFormat::Ir => return None,
            };
            Some((runtime, session.metadata.session_id))
        }
    }
}

/// Resolve a `--session` argument (a file path OR a native session id) to the
/// actual file on disk — the same resolution the loader uses — so callers read
/// from and guard against the *real* path, not the raw argument.
pub fn resolve_session(arg: &Path) -> Result<PathBuf> {
    Ok(formats::resolve_input(arg, SourceFormat::Auto)?.path)
}

/// The session a runtime is currently in (the newest matching rollout it's
/// writing), as (file, session id). `since` fences detection to files touched
/// at/after the hosted child was spawned, so an older unrelated session in the
/// same directory is never adopted as the seed.
pub fn active_session(
    from: Runtime,
    host_cwd: Option<&Path>,
    since: Option<SystemTime>,
) -> Option<(PathBuf, String)> {
    if from == Runtime::OpenCode {
        // OpenCode sessions live in its sqlite store, not files: find the
        // newest cwd-matching session there, then read it through a cache
        // copy so the path-centric carry machinery applies unchanged.
        let id = opencode_newest_session(host_cwd, since)?;
        let path = formats::export_opencode_to_cache(&id).ok()?;
        return Some((path, id));
    }
    let from_fmt = session_format(from);
    let path = find_child_session(from_fmt, host_cwd, since)?;
    let id = session_id_of(&path, from_fmt)?;
    Some((path, id))
}

/// Newest opencode session for a directory (read-only sqlite), respecting the
/// spawn-time fence and requiring at least one real message.
fn opencode_newest_session(host_cwd: Option<&Path>, since: Option<SystemTime>) -> Option<String> {
    use rusqlite::{Connection, OpenFlags};
    let db = formats::opencode_data_root().ok()?.join("opencode.db");
    if !db.exists() {
        return None;
    }
    let conn = Connection::open_with_flags(
        &db,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    let fence_ms = since
        .map(|s| s.checked_sub(Duration::from_secs(2)).unwrap_or(s))
        .and_then(|s| s.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let cwd = host_cwd.map(|p| p.display().to_string());
    let cwd_canon = host_cwd
        .and_then(|p| p.canonicalize().ok())
        .map(|p| p.display().to_string());
    conn.query_row(
        "SELECT s.id FROM session s
         WHERE (?1 IS NULL OR s.directory = ?1 OR s.directory = ?2)
           AND s.time_updated >= ?3
           AND EXISTS (SELECT 1 FROM message m WHERE m.session_id = s.id)
         ORDER BY s.time_updated DESC LIMIT 1",
        rusqlite::params![cwd, cwd_canon, fence_ms],
        |r| r.get(0),
    )
    .ok()
}

/// Resolve a session the harness POSITIVELY knows the child owns — by its
/// declared id (the id we resumed, or the `--session-id` we minted at spawn).
/// Newest file wins when duplicates exist for one id (an old fork bug left
/// some), so we never carry a stale twin. Returns None until the session
/// exists on disk with an actual conversation in it.
pub fn session_by_id(rt: Runtime, id: &str) -> Option<(PathBuf, String)> {
    let fmt = session_format(rt);
    let path = match fmt {
        SessionFormat::Codex => formats::resolve_codex_session_id(id).ok()?,
        SessionFormat::Claude => formats::resolve_claude_session_id(id).ok()?,
        SessionFormat::Gemini => formats::resolve_gemini_session_id(id).ok()?,
        SessionFormat::OpenCode => formats::export_opencode_to_cache(id).ok()?,
        SessionFormat::Ir => return None,
    };
    has_conversation(&path, fmt).then(|| (path, id.to_string()))
}

/// Whether the installed claude CLI accepts `--session-id` on a fresh launch
/// (declared identity — lets the harness KNOW the child's session instead of
/// inferring it from the filesystem). Cached per process.
pub fn claude_supports_session_id() -> bool {
    static CACHE: OnceLock<bool> = OnceLock::new();
    *CACHE.get_or_init(|| {
        std::process::Command::new("claude")
            .arg("--help")
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains("--session-id"))
            .unwrap_or(false)
    })
}

/// What a carry did to the conversation — surfaced to the user so the
/// distillation's lossiness is declared, never silent.
#[derive(Clone, Copy, Debug, Default)]
pub struct CarryReceipt {
    /// User/assistant turns carried.
    pub turns: usize,
    /// Tool calls/results carried (only in `--with-tools` mode).
    pub tools: usize,
    /// Tool calls/results dropped (the default, conversation-only mode).
    pub dropped_tools: usize,
    /// Reasoning events dropped (always — model-internal, never carried).
    pub dropped_reasoning: usize,
    /// Runtime-scaffold user turns dropped.
    pub dropped_scaffold: usize,
    /// Secrets burned off by redaction.
    pub redactions: usize,
}

impl CarryReceipt {
    /// One human line, e.g. `carried 14 turns · kept 6 tool events · 2 redactions`.
    pub fn summary(&self) -> String {
        let mut parts = vec![format!(
            "carried {} turn{}",
            self.turns,
            if self.turns == 1 { "" } else { "s" }
        )];
        if self.tools > 0 {
            parts.push(format!("kept {} tool events", self.tools));
        }
        if self.dropped_tools > 0 {
            parts.push(format!("dropped {} tool events", self.dropped_tools));
        }
        if self.dropped_reasoning > 0 {
            parts.push(format!("dropped {} reasoning", self.dropped_reasoning));
        }
        if self.dropped_scaffold > 0 {
            parts.push(format!("dropped {} scaffold", self.dropped_scaffold));
        }
        if self.redactions > 0 {
            parts.push(format!(
                "{} redaction{}",
                self.redactions,
                if self.redactions == 1 { "" } else { "s" }
            ));
        }
        parts.join(" · ")
    }
}

/// A source conversation loaded and distilled once, ready to be named and
/// materialized — so a switch reads the transcript a single time instead of
/// re-parsing it for the name and again for the carry.
pub struct Distilled {
    pub session: UniversalSession,
    pub receipt: CarryReceipt,
}

impl Distilled {
    /// The conversation's root handle (first real user message), for naming.
    pub fn root_name(&self) -> Option<String> {
        first_user_text(&self.session)
    }
}

/// Write a distilled conversation as a record volume (neutral IR snapshot).
/// Called BEFORE materializing any native copy — the record comes first; the
/// copies are derived. The snapshot is the post-sanitize payload (sealed
/// before it enters the record), and the write is atomic.
pub fn write_snapshot(session: &UniversalSession, path: &Path) -> Result<()> {
    formats::write_ir(session, path)
}

/// Phase 1: load a source session and distill it (sanitize + redact).
/// `keep_tools` carries (redacted, size-capped) tool calls/results instead of
/// dropping the agentic layer — the conversation-only default.
pub fn distill_source(src: &Path, keep_tools: bool) -> Result<Distilled> {
    let mut session = formats::load_session(src, SourceFormat::Auto)
        .with_context(|| format!("failed to read {}", src.display()))?;
    let receipt = sanitize(&mut session, keep_tools);
    Ok(Distilled { session, receipt })
}

/// Phase 2: materialize a distilled conversation into `to`'s native store. If
/// `reuse` is given, that session is OVERWRITTEN (so a switch syncs back into
/// this thread's existing counterpart rather than minting a new session every
/// time); otherwise a fresh id is minted. `src` is the file the distillation
/// came from, re-checked here so no caller can direct the write at its own
/// source. Returns (id, written_file, cwd).
pub fn distill_write(
    distilled: &mut Distilled,
    src: &Path,
    to: Runtime,
    reuse: Option<(&str, &Path)>,
    title: Option<&str>,
) -> Result<(String, PathBuf, Option<PathBuf>)> {
    let to_fmt = session_format(to);
    let session = &mut distilled.session;

    if let Some((_, path)) = reuse
        && same_file(src, path)
    {
        bail!("refusing to overwrite source session at {}", src.display());
    }

    if !session
        .events
        .iter()
        .any(|e| matches!(e, SessionEvent::Message(_)))
    {
        bail!("no conversation to carry yet");
    }

    if !supports_target(to) {
        bail!(
            "carrying INTO {} isn't supported yet — it works as a carry source",
            to.label()
        );
    }

    let id = match reuse {
        Some((id, _)) => id.to_string(),
        None => match to_fmt {
            SessionFormat::Claude => Uuid::new_v4().to_string(),
            SessionFormat::Codex => Uuid::now_v7().to_string(),
            SessionFormat::OpenCode => formats::opencode::mint_id("ses"),
            SessionFormat::Gemini | SessionFormat::Ir => bail!("unsupported target"),
        },
    };
    session.metadata.session_id = id.clone();

    // Stamp the target's real CLI version into session_meta so neither resume
    // rejects it (claude) nor codex's /resume backfill treats it as foreign.
    session.metadata.platform_version = Some(match to {
        Runtime::Claude => detect_claude_version().unwrap_or_else(|| "2.1.154".to_string()),
        Runtime::Codex => detect_codex_version().unwrap_or_else(|| "0.137.0".to_string()),
        Runtime::OpenCode => detect_opencode_version().unwrap_or_else(|| "1.14.48".to_string()),
        Runtime::Gemini => detect_gemini_version().unwrap_or_else(|| "0.40.0".to_string()),
    });
    // OpenCode shows info.title in its picker — stamp the trail name there.
    if to_fmt == SessionFormat::OpenCode
        && let Some(t) = title
    {
        session.metadata.title = Some(t.to_string());
    }

    // Reuse → overwrite the SAME file in place. This is the fix for the "fork on
    // disk" bug: codex names rollouts `rollout-<ts>-<id>.jsonl`, so writing to
    // the home root each sync produced a SECOND file with the same id and a new
    // timestamp, and `/resume` then loaded the wrong (original) one. Writing
    // straight to the existing file keeps exactly one file per id.
    let written = match (reuse, to_fmt) {
        (Some((_, path)), _) => formats::materialize(session, to_fmt, path)
            .with_context(|| format!("failed to overwrite {} session", to.label()))?,
        // OpenCode's store is a database; the Constant-owned file artifact is
        // the cache copy, and `opencode import` (below) is the real write.
        (None, SessionFormat::OpenCode) => {
            let cache = formats::opencode_cache_path(&id)?;
            formats::materialize(session, to_fmt, &cache)
                .with_context(|| format!("failed to write {} session", to.label()))?
        }
        (None, _) => {
            let out_root = formats::default_output_root(to_fmt)?;
            formats::materialize(session, to_fmt, &out_root)
                .with_context(|| format!("failed to write {} session", to.label()))?
        }
    };

    // Stamp the target's native resume-picker name. `title` is the trail name
    // (`constant·tNN·from-…`) when carried by the harness; otherwise we fall
    // back to the conversation's first user message.
    let first_msg = first_user_text(session).unwrap_or_else(|| "carried conversation".to_string());
    match to_fmt {
        // Codex's picker filters on has_user_event=1 + native source, so stamp
        // our row to look native; set `title`/`preview` to the trail name.
        // Codex resume REQUIRES this registry row — if it can't be written the
        // carry has failed, loudly, instead of producing a session resume will
        // never find (the caller falls back to a fresh launch).
        SessionFormat::Codex => {
            let display = title.unwrap_or(first_msg.as_str());
            upsert_codex_thread(session, &id, &written, display)
                .context("failed to register the codex session (resume would not find it)")?;
        }
        // Claude's `/rename` appends a `custom-title` record to the session
        // jsonl; we do the same so the projection shows the trail name in `-r`.
        SessionFormat::Claude => {
            if let Some(t) = title {
                let _ = stamp_claude_title(&written, &id, t);
            }
        }
        // OpenCode's registry step IS `opencode import` — their supported
        // door, which validates, preserves the id, and upserts on refresh.
        // Like codex's sqlite row, this is REQUIRED for resume: failure fails
        // the carry loudly (callers fall back to fresh).
        SessionFormat::OpenCode => {
            import_opencode(&written, &id)
                .context("failed to register the opencode session (import)")?;
        }
        SessionFormat::Gemini | SessionFormat::Ir => {}
    }

    Ok((id, written, session.metadata.cwd.clone()))
}

/// Run `opencode import` on a materialized export file and verify it landed
/// under the id we wrote (import preserves ids — verified live).
fn import_opencode(file: &Path, expected: &str) -> Result<()> {
    let out = std::process::Command::new("opencode")
        .arg("import")
        .arg(file)
        .output()
        .context("failed to run `opencode import` — is opencode on PATH?")?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    if !out.status.success() {
        bail!(
            "opencode import failed: {} {}",
            stdout.trim(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    match stdout
        .lines()
        .rev()
        .find_map(|l| l.trim().strip_prefix("Imported session: "))
        .map(str::trim)
    {
        Some(id) if id == expected => Ok(()),
        Some(id) => bail!("opencode import changed the session id ({id} != {expected})"),
        None => bail!("opencode import did not confirm a session id"),
    }
}

/// One-shot carry: load + distill + materialize. Returns (id, file, cwd, receipt).
/// (The interactive harness and CLI use the two-phase API to share one load;
/// this stays as the convenience entry for tests and future callers.)
#[cfg_attr(not(test), allow(dead_code))]
pub fn distill_path(
    src: &Path,
    to: Runtime,
    reuse: Option<(&str, &Path)>,
    title: Option<&str>,
) -> Result<(String, PathBuf, Option<PathBuf>, CarryReceipt)> {
    let mut distilled = distill_source(src, false)?;
    let (id, written, cwd) = distill_write(&mut distilled, src, to, reuse, title)?;
    Ok((id, written, cwd, distilled.receipt))
}

/// A runtime-generated title worth harvesting (opencode's generated titles,
/// a claude `/rename`), cleaned: never our own stamps (old `constant·…` or
/// new `… · chNN ← …` formats), never empty.
pub fn harvested_title(session: &UniversalSession) -> Option<String> {
    let t = session.metadata.title.as_deref()?.trim();
    if t.is_empty()
        || t.starts_with("constant\u{b7}")
        || t.contains(" \u{b7} ch")
        || t.contains("\u{2190}")
    {
        return None;
    }
    Some(t.chars().take(80).collect())
}

/// Re-stamp a projection's native picker title after a rename. Claude gets a
/// `custom-title` record; codex gets a registry UPDATE. OpenCode and gemini
/// pick the new name up at the next carry.
pub fn restamp_title(rt: Runtime, id: &str, path: &Path, title: &str) -> Result<()> {
    match rt {
        Runtime::Claude => stamp_claude_title(path, id, title),
        Runtime::Codex => retitle_codex(id, title),
        Runtime::OpenCode | Runtime::Gemini => Ok(()),
    }
}

fn retitle_codex(id: &str, title: &str) -> Result<()> {
    use rusqlite::{Connection, params};
    let db = formats::default_output_root(SessionFormat::Codex)?.join("state_5.sqlite");
    if !db.exists() {
        return Ok(());
    }
    let conn = Connection::open(&db)?;
    conn.busy_timeout(std::time::Duration::from_secs(3))?;
    conn.execute(
        "UPDATE threads SET title = ?2, preview = ?2 WHERE id = ?1",
        params![id, title],
    )?;
    Ok(())
}

/// The conversation's root handle: the first real user message in `path`,
/// sanitized the same way a carry would. Used to name the trail.
pub fn root_name(path: &Path, _from: Runtime) -> Option<String> {
    distill_source(path, false).ok().and_then(|d| d.root_name())
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
/// Cached for the process (M2): the version can't change mid-run, so we spawn the
/// subprocess once instead of on every switch.
fn detect_codex_version() -> Option<String> {
    static CACHE: OnceLock<Option<String>> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            let output = std::process::Command::new("codex")
                .arg("--version")
                .output()
                .ok()?;
            let text = String::from_utf8_lossy(&output.stdout);
            text.split_whitespace()
                .find(|t| {
                    t.chars()
                        .next()
                        .map(|c| c.is_ascii_digit())
                        .unwrap_or(false)
                })
                .map(str::to_string)
        })
        .clone()
}

/// Upsert a codex `threads` row so a synthetic session looks native and appears
/// in `codex /resume` (has_user_event=1, source=cli, real provider/version).
/// This is the ONLY place the codex registry is written — one statement, one
/// column set, for both mint and refresh — and its failure is a real error:
/// codex resume cannot find a session without its registry row.
fn upsert_codex_thread(
    session: &UniversalSession,
    id: &str,
    rollout_path: &Path,
    title: &str,
) -> Result<()> {
    use rusqlite::{Connection, params};
    let db = formats::default_output_root(SessionFormat::Codex)?.join("state_5.sqlite");
    if !db.exists() {
        // Codex has never run on this machine: nothing to register (and nothing
        // that could resume anyway).
        return Ok(());
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let created = session
        .metadata
        .created_at
        .map(|t| t.timestamp())
        .unwrap_or(now);
    let cwd = session
        .metadata
        .cwd
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| ".".to_string());
    let sandbox_policy = session
        .metadata
        .extra
        .get("codex_sandbox_policy")
        .map(|v| match v {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        })
        .unwrap_or_else(|| "{\"type\":\"workspace-write\"}".to_string());
    let approval_mode = session
        .metadata
        .extra
        .get("codex_approval_policy")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("on-request")
        .to_string();
    let first_msg = first_user_text(session).unwrap_or_else(|| title.to_string());
    let has_user_event = session
        .events
        .iter()
        .any(|e| matches!(e, SessionEvent::Message(m) if m.role == "user"))
        as i64;
    let version = detect_codex_version().unwrap_or_else(|| "0.137.0".to_string());
    let conn = Connection::open(&db)?;
    conn.busy_timeout(std::time::Duration::from_secs(3))?;
    conn.execute(
        "INSERT INTO threads
            (id, rollout_path, created_at, updated_at, source, model_provider, cwd, title,
             sandbox_policy, approval_mode, tokens_used, has_user_event, archived,
             git_branch, cli_version, first_user_message, memory_mode, preview)
         VALUES (?1, ?2, ?3, ?4, 'cli', 'openai', ?5, ?6,
             ?7, ?8, 0, ?9, 0,
             ?10, ?11, ?12, 'enabled', ?6)
         ON CONFLICT(id) DO UPDATE SET
             rollout_path = excluded.rollout_path,
             updated_at = excluded.updated_at,
             source = 'cli',
             model_provider = 'openai',
             cwd = excluded.cwd,
             sandbox_policy = excluded.sandbox_policy,
             approval_mode = excluded.approval_mode,
             has_user_event = excluded.has_user_event,
             archived = 0,
             git_branch = excluded.git_branch,
             cli_version = excluded.cli_version,
             title = excluded.title,
             first_user_message = excluded.first_user_message,
             preview = excluded.preview",
        params![
            id,
            rollout_path.display().to_string(),
            created,
            now,
            cwd,
            title,
            sandbox_policy,
            approval_mode,
            has_user_event,
            session.metadata.git_branch,
            version,
            first_msg
        ],
    )?;
    Ok(())
}

/// Append a Claude `custom-title` record to a session jsonl — exactly what the
/// in-app `/rename` does — so the projection shows the trail name in `claude -r`.
fn stamp_claude_title(path: &Path, id: &str, title: &str) -> Result<()> {
    use std::io::Write;
    let rec = serde_json::json!({
        "type": "custom-title",
        "customTitle": title,
        "sessionId": id,
    });
    let mut f = fs::OpenOptions::new().append(true).open(path)?;
    writeln!(f, "{}", serde_json::to_string(&rec)?)?;
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
            value
                .get("payload")?
                .get("id")?
                .as_str()
                .map(str::to_string)
        }
        SessionFormat::Gemini => formats::gemini::session_id(path),
        SessionFormat::OpenCode => formats::opencode::session_id(path),
        SessionFormat::Ir => None,
    }
}

fn session_format(runtime: Runtime) -> SessionFormat {
    match runtime {
        Runtime::Codex => SessionFormat::Codex,
        Runtime::Claude => SessionFormat::Claude,
        Runtime::Gemini => SessionFormat::Gemini,
        Runtime::OpenCode => SessionFormat::OpenCode,
    }
}

/// Whether a runtime can be a carry TARGET. Gemini is source-only until its
/// 0.40 session landing pad is verified live (see docs/third-runtime-formats.md).
pub fn supports_target(runtime: Runtime) -> bool {
    !matches!(runtime, Runtime::Gemini)
}

/// True when `b` resolves to the same file as `a` — the core F1 guard: even if
/// a caller passes a bad reuse/output target, alembic refuses to materialize
/// over the donor conversation. For existing files this compares device+inode
/// (catching hard links AND symlinks, which path canonicalization alone
/// misses); for a not-yet-existing `b` it compares resolved parent + filename.
/// This is THE one implementation — used by the carry guard and `export --out`.
pub fn same_file(a: &Path, b: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let (Ok(a), Ok(b)) = (fs::metadata(a), fs::metadata(b)) {
            return a.dev() == b.dev() && a.ino() == b.ino();
        }
    }

    let Ok(a_c) = a.canonicalize() else {
        return false;
    };
    if let Ok(b_c) = b.canonicalize() {
        return a_c == b_c;
    }
    // `b` doesn't exist yet: resolve its parent dir + filename.
    match (b.parent(), b.file_name()) {
        (Some(parent), Some(name)) => {
            let parent = if parent.as_os_str().is_empty() {
                Path::new(".")
            } else {
                parent
            };
            parent
                .canonicalize()
                .map(|p| p.join(name) == a_c)
                .unwrap_or(false)
        }
        _ => false,
    }
}

/// Constant's taste: keep only genuine user/assistant conversation, drop runtime
/// scaffold + tool/reasoning noise, and redact secrets. This is the distillation
/// step transession does NOT do — it carries everything, cruft and credentials
/// included (which we saw leak a token into a fresh session).
///
/// The redaction invariant is enforced HERE, on the path every carry takes:
/// `block.data` payloads and per-message metadata — which the loaders preserve
/// and could carry secrets the text redactor never sees — are dropped outright,
/// exactly as the IR export does. Returns a receipt of what was done.
///
/// `keep_tools` carries tool calls/results too — every string anywhere in
/// their JSON payloads is redacted, and oversized tool outputs are capped (a
/// file dump must not ride along into the target's context). Reasoning is
/// dropped either way: it's model-internal, not conversation or tool history.
fn sanitize(session: &mut UniversalSession, keep_tools: bool) -> CarryReceipt {
    let mut receipt = CarryReceipt::default();
    let mut kept = Vec::new();
    for event in std::mem::take(&mut session.events) {
        let message = match event {
            SessionEvent::Message(message) => message,
            SessionEvent::Reasoning(_) => {
                receipt.dropped_reasoning += 1;
                continue;
            }
            SessionEvent::ToolCall(mut call) if keep_tools => {
                call.metadata.clear();
                receipt.redactions += redact_value(&mut call.arguments);
                receipt.tools += 1;
                kept.push(SessionEvent::ToolCall(call));
                continue;
            }
            SessionEvent::ToolResult(mut result) if keep_tools => {
                result.metadata.clear();
                receipt.redactions += redact_value(&mut result.output);
                truncate_tool_output(&mut result.output);
                receipt.tools += 1;
                kept.push(SessionEvent::ToolResult(result));
                continue;
            }
            SessionEvent::ToolCall(_) | SessionEvent::ToolResult(_) => {
                receipt.dropped_tools += 1;
                continue;
            }
        };
        let mut message = message;
        // Drop developer/system scaffold messages outright.
        if message.role != "user" && message.role != "assistant" {
            receipt.dropped_scaffold += 1;
            continue;
        }
        // Hoist the real model name before metadata is cleared, so the claude
        // writer can stamp records with the model that actually produced them.
        if message.role == "assistant"
            && !session.metadata.extra.contains_key("claude_model")
            && let Some(serde_json::Value::String(m)) = message.metadata.get("model")
            && m.starts_with("claude")
        {
            session.metadata.extra.insert(
                "claude_model".to_string(),
                serde_json::Value::String(m.clone()),
            );
        }
        message.metadata.clear();
        for block in &mut message.blocks {
            block.data = None;
            if let Some(text) = &block.text {
                let (clean, n) = redact(text);
                receipt.redactions += n;
                block.text = Some(clean);
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
            receipt.dropped_scaffold += 1;
            continue;
        }
        receipt.turns += 1;
        kept.push(SessionEvent::Message(message));
    }
    session.events = kept;
    receipt
}

/// Redact every string anywhere inside a JSON value (tool arguments/outputs
/// are arbitrary nested JSON — the text redactor must reach all of it).
/// Returns how many secrets were burned.
fn redact_value(value: &mut serde_json::Value) -> usize {
    match value {
        serde_json::Value::String(s) => {
            let (out, n) = redact(s);
            *s = out;
            n
        }
        serde_json::Value::Array(items) => items.iter_mut().map(redact_value).sum(),
        serde_json::Value::Object(map) => map.values_mut().map(redact_value).sum(),
        _ => 0,
    }
}

/// Cap a carried tool output. A multi-megabyte file dump in one tool result
/// would ride straight into the target runtime's context window on resume —
/// clean over comprehensive.
const TOOL_OUTPUT_CAP: usize = 10_000;

fn truncate_tool_output(output: &mut serde_json::Value) {
    let rendered_len = match &*output {
        serde_json::Value::String(s) => s.len(),
        other => serde_json::to_string(other).map(|t| t.len()).unwrap_or(0),
    };
    if rendered_len <= TOOL_OUTPUT_CAP {
        return;
    }
    let text = match &*output {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    let cut: String = text.chars().take(TOOL_OUTPUT_CAP).collect();
    *output = serde_json::Value::String(format!(
        "{cut}\n… [truncated by constant: tool output exceeded {TOOL_OUTPUT_CAP} chars]"
    ));
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

/// Secret/credential redactors, compiled ONCE (M1). Order matters: specific
/// token shapes first, then generic key=value, then the oai-citation noise block.
///
/// Known accepted residual (M4): the email and generic key=value patterns are
/// deliberately broad — they can black out a legitimate `a@b.com` or `token: x`
/// in prose. We accept over-redaction over under-redaction: carrying a credential
/// into another model is a real safety failure; a blacked-out word is cosmetic.
static REDACTORS: LazyLock<Vec<(regex::Regex, &'static str, bool)>> = LazyLock::new(|| {
    // (pattern, replacement, counts-as-redaction). The citation strip is
    // cosmetic noise removal, not a secret — it doesn't count in the receipt.
    let specs: [(&str, &str, bool); 12] = [
        (
            r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}\b",
            "[redacted-email]",
            true,
        ),
        (r"\bsk-[A-Za-z0-9_-]{16,}\b", "[redacted-key]", true),
        (r"\bgh[pousr]_[A-Za-z0-9]{16,}\b", "[redacted-token]", true),
        (r"\bxox[baprs]-[A-Za-z0-9-]{10,}\b", "[redacted-token]", true),
        // AWS access key ids have a fixed, unmistakable shape.
        (r"\bAKIA[0-9A-Z]{16}\b", "[redacted-aws-key]", true),
        // PEM private key blocks — redact the whole block, including a torn one
        // with no END marker (better to over-redact than leak half a key).
        (
            r"(?s)-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----.*?(-----END [A-Z0-9 ]*PRIVATE KEY-----|\z)",
            "[redacted-private-key]",
            true,
        ),
        // Bare JWTs in prose (three base64url segments starting with `eyJ`).
        (
            r"\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{4,}\b",
            "[redacted-jwt]",
            true,
        ),
        // Quoted JSON/object form: `"authorization": "Basic ..."` — a quote sits
        // between the key and the colon, so the plain rule below misses it. Redact
        // the quoted value (scheme + credential) and keep the quotes/structure.
        (
            r#"(?i)(["']authorization["']\s*[:=]\s*["'])[^"']*(["'])"#,
            "${1}[redacted]${2}",
            true,
        ),
        // Whole `Authorization:` value (scheme + credential, to end of line) —
        // covers Bearer, Basic, ApiKey, etc. The generic key=value rule below
        // only consumes the scheme word and would leave the credential, so this
        // must run first and redact the rest of the header line.
        (r"(?i)(\bauthorization\b\s*[:=]\s*).*", "${1}[redacted]", true),
        // Bare `Bearer <token>` outside an Authorization header.
        (
            r"(?i)\bbearer\s+[A-Za-z0-9._~+/=-]+",
            "Bearer [redacted]",
            true,
        ),
        (
            r"(?i)\b(api[_-]?key|token|secret|password|authorization|bearer)\b(\s*[:=]\s*)\S+",
            "$1$2[redacted]",
            true,
        ),
        (r"(?s)\s*<oai-mem-citation>.*?</oai-mem-citation>\s*", "", false),
    ];
    specs
        .into_iter()
        .filter_map(|(p, r, c)| regex::Regex::new(p).ok().map(|re| (re, r, c)))
        .collect()
});

/// Burn off secrets so we never carry credentials across a runtime boundary.
/// Returns the cleaned text and how many secrets were redacted (for the receipt).
fn redact(text: &str) -> (String, usize) {
    let mut out = text.to_string();
    let mut burned = 0usize;
    for (re, repl, counts) in REDACTORS.iter() {
        if *counts {
            burned += re.find_iter(&out).count();
        }
        out = re.replace_all(&out, *repl).into_owned();
    }
    (out.trim().to_string(), burned)
}

/// Detect the installed Claude CLI version (e.g. "2.1.154") for the session
/// `version` field, so resume accepts it as native. Cached per process (M2).
fn detect_claude_version() -> Option<String> {
    static CACHE: OnceLock<Option<String>> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            let output = std::process::Command::new("claude")
                .arg("--version")
                .output()
                .ok()?;
            let text = String::from_utf8_lossy(&output.stdout);
            text.split_whitespace()
                .next()
                .filter(|t| {
                    t.chars()
                        .next()
                        .map(|c| c.is_ascii_digit())
                        .unwrap_or(false)
                })
                .map(str::to_string)
        })
        .clone()
}

/// Detect the installed gemini CLI version (e.g. "0.40.0"). Cached per process.
fn detect_gemini_version() -> Option<String> {
    static CACHE: OnceLock<Option<String>> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            let output = std::process::Command::new("gemini")
                .arg("--version")
                .output()
                .ok()?;
            let text = String::from_utf8_lossy(&output.stdout);
            text.split_whitespace()
                .find(|t| t.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false))
                .map(str::to_string)
        })
        .clone()
}

/// Detect the installed opencode CLI version (e.g. "1.14.48"). Cached per process.
fn detect_opencode_version() -> Option<String> {
    static CACHE: OnceLock<Option<String>> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            let output = std::process::Command::new("opencode")
                .arg("--version")
                .output()
                .ok()?;
            let text = String::from_utf8_lossy(&output.stdout);
            text.split_whitespace()
                .find(|t| t.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false))
                .map(str::to_string)
        })
        .clone()
}

/// Find the session this harness is hosting: newest `*.jsonl` under the runtime
/// store whose recorded cwd matches `host_cwd` and whose mtime is at/after the
/// child was spawned (`since`, with a small slack for filesystem timestamp
/// granularity). cwd-scoping stops us grabbing an unrelated session from
/// another directory; the spawn-time fence stops us resurrecting an OLD session
/// from this directory — or adopting a concurrent one — as the carry seed.
fn find_child_session(
    from: SessionFormat,
    host_cwd: Option<&Path>,
    since: Option<SystemTime>,
) -> Option<PathBuf> {
    let root = formats::default_output_root(from).ok()?;
    let search = root.join(match from {
        SessionFormat::Codex => "sessions",
        SessionFormat::Claude => "projects",
        SessionFormat::Gemini => "tmp",
        // OpenCode is db-backed — discovery happens in opencode_newest_session.
        SessionFormat::OpenCode | SessionFormat::Ir => return None,
    });
    let want_slug = host_cwd.map(cwd_slug);
    let want_hash = host_cwd.map(|p| {
        sha256::hex(
            &p.canonicalize()
                .unwrap_or_else(|_| p.to_path_buf())
                .display()
                .to_string(),
        )
    });
    let fence = since.map(|s| s.checked_sub(Duration::from_secs(2)).unwrap_or(s));

    // Gather candidates by mtime only (cheap — no content reads), then check
    // newest-first and stop at the first match. Avoids fully reading every
    // session file on disk on every switch (F2): we typically open exactly one.
    let mut candidates: Vec<(SystemTime, PathBuf)> = Vec::new();
    let mut stack = vec![search];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            // file_type() does NOT follow symlinks: a symlink cycle inside a
            // store can't hang the switch.
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                stack.push(path);
                continue;
            }
            let ext = path.extension().and_then(|e| e.to_str());
            let ext_ok = match from {
                // Gemini sessions are .json (or .jsonl in newer versions) and
                // live only under chats/ (logs.json etc. must not match).
                SessionFormat::Gemini => {
                    matches!(ext, Some("json") | Some("jsonl")) && formats::under_chats(&path)
                }
                _ => ext == Some("jsonl"),
            };
            if !ext_ok {
                continue;
            }
            let Ok(mtime) = entry.metadata().and_then(|m| m.modified()) else {
                continue;
            };
            if let Some(f) = fence
                && mtime < f
            {
                continue; // predates the hosted child: not its session
            }
            candidates.push((mtime, path));
        }
    }
    candidates.sort_by_key(|b| std::cmp::Reverse(b.0)); // newest first

    for (_, path) in candidates {
        // Cheap cwd filter before the full-content conversation check.
        let cwd_ok = match (from, host_cwd) {
            (_, None) => true,
            (SessionFormat::Codex, Some(c)) => codex_session_cwd(&path)
                .map(|rec| same_dir(&rec, c))
                .unwrap_or(false),
            (SessionFormat::Claude, Some(_)) => {
                path.parent()
                    .and_then(|p| p.file_name())
                    .and_then(|n| n.to_str())
                    == want_slug.as_deref()
            }
            // Gemini files record sha256(cwd) — scheme-independent evidence.
            (SessionFormat::Gemini, Some(_)) => {
                formats::gemini::session_project_hash(&path).as_deref() == want_hash.as_deref()
            }
            (SessionFormat::OpenCode, _) | (SessionFormat::Ir, _) => false,
        };
        if !cwd_ok {
            continue;
        }
        if has_conversation(&path, from) {
            return Some(path);
        }
    }
    None
}

/// Does this session contain an actual user/assistant exchange (vs. a session a
/// fresh launch just opened with no real turns)? Each line is PARSED, not
/// substring-matched — embedded instruction text (an AGENTS.md inside codex's
/// session_meta, say) that merely contains `"role":"user"` must not make an
/// empty session look conversational.
fn has_conversation(path: &Path, from: SessionFormat) -> bool {
    use std::io::{BufRead, BufReader};
    let Ok(file) = fs::File::open(path) else {
        return false;
    };
    BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .any(|line| {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
                return false;
            };
            match from {
                SessionFormat::Codex => {
                    v.get("type").and_then(serde_json::Value::as_str) == Some("response_item")
                        && v.get("payload")
                            .map(|p| {
                                p.get("type").and_then(serde_json::Value::as_str)
                                    == Some("message")
                                    && matches!(
                                        p.get("role").and_then(serde_json::Value::as_str),
                                        Some("user") | Some("assistant")
                                    )
                            })
                            .unwrap_or(false)
                }
                SessionFormat::Claude => {
                    matches!(
                        v.get("type").and_then(serde_json::Value::as_str),
                        Some("user") | Some("assistant")
                    ) && v.get("message").is_some()
                }
                // Whole-document formats: the line-reader hands us the full
                // text only for single-line files; parse the whole file instead.
                SessionFormat::Gemini | SessionFormat::OpenCode => false,
                SessionFormat::Ir => false,
            }
        })
        || match from {
            // Whole-document formats need a document parse, not a line scan.
            SessionFormat::Gemini => fs::read_to_string(path)
                .ok()
                .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
                .and_then(|v| {
                    v.get("messages").and_then(|m| {
                        m.as_array().map(|a| {
                            a.iter().any(|msg| {
                                matches!(
                                    msg.get("type").and_then(serde_json::Value::as_str),
                                    Some("user") | Some("gemini")
                                )
                            })
                        })
                    })
                })
                .unwrap_or(false),
            SessionFormat::OpenCode => fs::read_to_string(path)
                .ok()
                .and_then(|t| {
                    let end = t.rfind('}').map(|i| i + 1)?;
                    serde_json::from_str::<serde_json::Value>(&t[..end]).ok()
                })
                .and_then(|v| v.get("messages").and_then(|m| m.as_array().map(|a| !a.is_empty())))
                .unwrap_or(false),
            _ => false,
        }
}

/// Compare a recorded cwd string to the host cwd, tolerant of trailing slashes
/// and symlinks (L2): canonicalize both when they exist on disk, else fall back
/// to a trimmed-string match.
fn same_dir(recorded: &str, here: &Path) -> bool {
    let r = Path::new(recorded);
    if let (Ok(a), Ok(b)) = (r.canonicalize(), here.canonicalize()) {
        return a == b;
    }
    recorded.trim_end_matches('/') == here.to_string_lossy().trim_end_matches('/')
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

// ---- Programmatic / headless surface ----------------------------------------

/// A session discoverable in a runtime's store (for `constant sessions`).
pub struct SessionSummary {
    pub runtime: &'static str,
    pub id: String,
    pub path: PathBuf,
    pub cwd: Option<String>,
    pub mtime: SystemTime,
    /// `None` when not checked (the fast, metadata-only default — checking
    /// requires reading the file); `Some(bool)` only when titles are requested.
    pub has_conversation: Option<bool>,
    pub title: Option<String>,
}

/// List the sessions in a runtime's store, newest first, optionally scoped to a
/// working directory (the cwd the session was recorded in / Claude project slug).
///
/// `with_titles` is opt-in because deriving a title fully loads + sanitizes each
/// transcript; on a large store that's expensive, so bulk discovery defaults to
/// metadata only and titles are computed only when explicitly requested.
pub fn list_sessions(
    runtime: Runtime,
    cwd: Option<&Path>,
    with_titles: bool,
) -> Vec<SessionSummary> {
    let fmt = session_format(runtime);
    if fmt == SessionFormat::OpenCode {
        return list_opencode_sessions(cwd);
    }
    let Ok(root) = formats::default_output_root(fmt) else {
        return Vec::new();
    };
    let search = root.join(match fmt {
        SessionFormat::Codex => "sessions",
        SessionFormat::Claude => "projects",
        SessionFormat::Gemini => "tmp",
        SessionFormat::OpenCode | SessionFormat::Ir => return Vec::new(),
    });
    let want_slug = cwd.map(cwd_slug);
    let want_hash = cwd.map(|p| {
        sha256::hex(
            &p.canonicalize()
                .unwrap_or_else(|_| p.to_path_buf())
                .display()
                .to_string(),
        )
    });

    let mut out = Vec::new();
    let mut stack = vec![search];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            // No symlink-following on recursion (cycle safety).
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                stack.push(path);
                continue;
            }
            let ext = path.extension().and_then(|e| e.to_str());
            let ext_ok = match fmt {
                SessionFormat::Gemini => {
                    matches!(ext, Some("json") | Some("jsonl")) && formats::under_chats(&path)
                }
                _ => ext == Some("jsonl"),
            };
            if !ext_ok {
                continue;
            }
            let Ok(mtime) = entry.metadata().and_then(|m| m.modified()) else {
                continue;
            };
            let rec_cwd = match fmt {
                SessionFormat::Codex => codex_session_cwd(&path),
                _ => None,
            };
            let cwd_ok = match (fmt, cwd) {
                (_, None) => true,
                (SessionFormat::Codex, Some(c)) => {
                    rec_cwd.as_deref().map(|r| same_dir(r, c)).unwrap_or(false)
                }
                (SessionFormat::Claude, Some(_)) => {
                    path.parent()
                        .and_then(|p| p.file_name())
                        .and_then(|n| n.to_str())
                        == want_slug.as_deref()
                }
                (SessionFormat::Gemini, Some(_)) => {
                    formats::gemini::session_project_hash(&path).as_deref()
                        == want_hash.as_deref()
                }
                (SessionFormat::OpenCode, _) | (SessionFormat::Ir, _) => false,
            };
            if !cwd_ok {
                continue;
            }
            let id = session_id_of(&path, fmt).unwrap_or_default();
            // Reading the file body (has_conversation + title) is opt-in: the
            // default listing stays metadata-only so it doesn't scan every
            // transcript on a large store.
            let (has_conversation, title) = if with_titles {
                let has = has_conversation(&path, fmt);
                (
                    Some(has),
                    if has { root_name(&path, runtime) } else { None },
                )
            } else {
                (None, None)
            };
            out.push(SessionSummary {
                runtime: runtime.label(),
                id,
                path,
                cwd: rec_cwd,
                mtime,
                has_conversation,
                title,
            });
        }
    }
    out.sort_by_key(|b| std::cmp::Reverse(b.mtime));
    out
}

/// CLI versions Constant's codec is validated against. The native session
/// formats are undocumented and can drift between releases (S1); these are the
/// versions the round-trip tests exercise.
/// OpenCode session listing straight from its db (read-only) — titles come
/// from the `session.title` column, so even `--titles` costs no transcript read.
fn list_opencode_sessions(cwd: Option<&Path>) -> Vec<SessionSummary> {
    use rusqlite::{Connection, OpenFlags};
    let Ok(root) = formats::opencode_data_root() else {
        return Vec::new();
    };
    let db = root.join("opencode.db");
    if !db.exists() {
        return Vec::new();
    }
    let Ok(conn) = Connection::open_with_flags(
        &db,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) else {
        return Vec::new();
    };
    let cwd_s = cwd.map(|p| p.display().to_string());
    let cwd_c = cwd
        .and_then(|p| p.canonicalize().ok())
        .map(|p| p.display().to_string());
    let Ok(mut stmt) = conn.prepare(
        "SELECT s.id, s.directory, s.title, s.time_updated,
                EXISTS (SELECT 1 FROM message m WHERE m.session_id = s.id)
         FROM session s
         WHERE (?1 IS NULL OR s.directory = ?1 OR s.directory = ?2)
         ORDER BY s.time_updated DESC",
    ) else {
        return Vec::new();
    };
    let rows = stmt.query_map(rusqlite::params![cwd_s, cwd_c], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, i64>(3)?,
            r.get::<_, bool>(4)?,
        ))
    });
    let Ok(rows) = rows else {
        return Vec::new();
    };
    rows.flatten()
        .map(|(id, directory, title, updated_ms, has)| SessionSummary {
            runtime: "opencode",
            id,
            path: PathBuf::new(), // db-backed; no file until exported
            cwd: Some(directory),
            mtime: UNIX_EPOCH + Duration::from_millis(updated_ms.max(0) as u64),
            has_conversation: Some(has),
            title: (!title.is_empty()).then_some(title),
        })
        .collect()
}

pub const SUPPORTED_CODEX: &str = "0.137";
pub const SUPPORTED_CLAUDE: &str = "2.1";
pub const SUPPORTED_GEMINI: &str = "0.40";
pub const SUPPORTED_OPENCODE: &str = "1.14";

/// Environment preflight (for `constant doctor`).
pub struct DoctorReport {
    pub codex_version: Option<String>,
    pub claude_version: Option<String>,
    pub gemini_version: Option<String>,
    pub opencode_version: Option<String>,
    pub codex_store: bool,
    pub claude_store: bool,
    pub gemini_store: bool,
    pub opencode_db: bool,
    pub codex_db: bool,
}

/// Probe the local environment: which CLIs are installed, their versions, and
/// whether their session stores are present.
pub fn doctor() -> DoctorReport {
    let codex_root = formats::default_output_root(SessionFormat::Codex).ok();
    let claude_root = formats::default_output_root(SessionFormat::Claude).ok();
    let gemini_root = formats::default_output_root(SessionFormat::Gemini).ok();
    let opencode_root = formats::default_output_root(SessionFormat::OpenCode).ok();
    DoctorReport {
        codex_version: detect_codex_version(),
        claude_version: detect_claude_version(),
        gemini_version: detect_gemini_version(),
        opencode_version: detect_opencode_version(),
        codex_store: codex_root
            .as_ref()
            .map(|r| r.join("sessions").exists())
            .unwrap_or(false),
        claude_store: claude_root
            .as_ref()
            .map(|r| r.join("projects").exists())
            .unwrap_or(false),
        gemini_store: gemini_root
            .as_ref()
            .map(|r| r.join("tmp").exists())
            .unwrap_or(false),
        opencode_db: opencode_root
            .as_ref()
            .map(|r| r.join("opencode.db").exists())
            .unwrap_or(false),
        codex_db: codex_root
            .as_ref()
            .map(|r| r.join("state_5.sqlite").exists())
            .unwrap_or(false),
    }
}

/// Export a source session to the neutral IR — the portable, runtime-agnostic
/// "master copy" of a conversation. Sanitized + redacted (the same distilled
/// payload a carry produces), so it never contains secrets. Returns the pretty
/// JSON plus the message count.
pub fn export_ir(src: &Path) -> Result<(String, usize)> {
    let mut session = formats::load_session(src, SourceFormat::Auto)
        .with_context(|| format!("failed to read {}", src.display()))?;
    let _ = sanitize(&mut session, false);
    // Defense in depth: `sanitize` already drops nested `block.data` payloads
    // and per-message metadata (the redaction invariant on every path), but a
    // master file is the artifact people share — strip again explicitly.
    for event in &mut session.events {
        if let SessionEvent::Message(message) = event {
            message.metadata.clear();
            for block in &mut message.blocks {
                block.data = None;
            }
        }
    }
    // A portable master is the *conversation*, not the runtime's scaffold: drop
    // the runtime-specific metadata blobs (system prompt, sandbox/approval config,
    // collaboration mode) that ride in `extra`, and give it a clean human title
    // from the first real user message instead of the injected AGENTS.md preamble.
    session.metadata.title = first_user_text(&session);
    session.metadata.extra.clear();
    let messages = session
        .events
        .iter()
        .filter(|e| matches!(e, SessionEvent::Message(_)))
        .count();
    let json = serde_json::to_string_pretty(&session).context("failed to encode IR")?;
    Ok((json, messages))
}

#[cfg(test)]
mod tests {
    use super::ir::{ContentBlock, MessageEvent};
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn msg(role: &str, text: &str) -> SessionEvent {
        SessionEvent::Message(MessageEvent {
            id: None,
            parent_id: None,
            role: role.to_string(),
            timestamp: None,
            blocks: vec![ContentBlock::text("text", text)],
            metadata: BTreeMap::new(),
        })
    }

    fn unique_tmp() -> PathBuf {
        static N: AtomicU32 = AtomicU32::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "constant-test-{}-{}-{}",
            std::process::id(),
            nanos,
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::create_dir_all(&dir);
        dir
    }

    // --- redaction (M4 residual is accepted: over-redaction beats leakage) ---

    fn r(text: &str) -> String {
        redact(text).0
    }

    #[test]
    fn redact_burns_secrets() {
        let (out, burned) =
            redact("key sk-ABCDEFGHIJKLMNOPqrstuvwx, mail a@b.com, token: hunter2value");
        assert!(out.contains("[redacted-key]"), "{out}");
        assert!(out.contains("[redacted-email]"), "{out}");
        assert!(out.contains("[redacted]"), "{out}");
        assert!(!out.contains("sk-ABCDEFGHIJKLMNOP"), "{out}");
        assert!(!out.contains("hunter2value"), "{out}");
        assert!(burned >= 3, "receipt undercounted: {burned}");
    }

    #[test]
    fn redact_burns_authorization_headers() {
        // Every common Authorization scheme must lose its credential.
        let bearer = r("Authorization: Bearer eyJhbG.ciOiJIUz.I1NiIsInR5cCI6Ik");
        assert!(!bearer.contains("eyJhbG"), "bearer leaked: {bearer}");
        let basic = r("Authorization: Basic dXNlcjpwYXNzd29yZA==");
        assert!(!basic.contains("dXNlcjpwYXNz"), "basic leaked: {basic}");
        let apikey = r("Authorization: ApiKey supersecretvalue123");
        assert!(
            !apikey.contains("supersecretvalue123"),
            "apikey leaked: {apikey}"
        );
        // Bare bearer scheme outside a header.
        let bare = r("bearer abcDEF123.456_tok-en");
        assert!(!bare.contains("abcDEF123"), "bare bearer leaked: {bare}");
        // Quoted JSON/header-object forms.
        let json_basic = r(r#"{"Authorization":"Basic dXNlcjpwYXNzd29yZA=="}"#);
        assert!(
            !json_basic.contains("dXNlcjpwYXNz"),
            "quoted basic leaked: {json_basic}"
        );
        let json_api = r(r#"{"authorization": "ApiKey supersecretvalue123"}"#);
        assert!(
            !json_api.contains("supersecretvalue123"),
            "quoted apikey leaked: {json_api}"
        );
    }

    #[test]
    fn redact_burns_aws_jwt_and_private_keys() {
        let aws = r("creds AKIAIOSFODNN7EXAMPLE here");
        assert!(!aws.contains("AKIAIOSFODNN7"), "aws key leaked: {aws}");
        let jwt = r("token eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.SflKxwRJSMeKKF2QT4");
        assert!(!jwt.contains("SflKxwRJ"), "jwt leaked: {jwt}");
        let pem = r("-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEA\n-----END RSA PRIVATE KEY-----");
        assert!(!pem.contains("MIIEowIBAAKCAQEA"), "private key leaked: {pem}");
        // A torn block with no END marker still burns to the end of text.
        let torn = r("-----BEGIN PRIVATE KEY-----\nMIIEvAIBADANBgkqh");
        assert!(!torn.contains("MIIEvAIBADAN"), "torn key leaked: {torn}");
    }

    #[test]
    fn redact_keeps_plain_prose() {
        let (out, burned) = redact("just a normal sentence about cats");
        assert_eq!(out, "just a normal sentence about cats");
        assert_eq!(burned, 0);
    }

    // --- the carry path enforces the redaction invariant on nested payloads ---

    #[test]
    fn sanitize_strips_block_data_and_metadata_and_reports() {
        let mut user = match msg("user", "hello sk-ABCDEFGHIJKLMNOPqrst") {
            SessionEvent::Message(m) => m,
            _ => unreachable!(),
        };
        user.blocks.push(ir::ContentBlock {
            kind: "data".to_string(),
            text: Some("attachment".to_string()),
            data: Some(serde_json::json!({"leak": "sk-DATALEAK1234567890abcd"})),
        });
        user.metadata.insert(
            "leakmeta".to_string(),
            serde_json::json!("sk-METALEAK1234567890abcd"),
        );
        let mut session = UniversalSession::new("s-0001".to_string());
        session.events.push(SessionEvent::Message(user));
        session.events.push(msg("assistant", "hi"));
        session.events.push(SessionEvent::Reasoning(ir::ReasoningEvent {
            id: None,
            parent_id: None,
            timestamp: None,
            summary: vec!["thinking".to_string()],
            metadata: BTreeMap::new(),
        }));

        let receipt = sanitize(&mut session, false);
        let json = serde_json::to_string(&session.events).unwrap();
        assert!(!json.contains("sk-DATALEAK"), "block.data leaked: {json}");
        assert!(!json.contains("sk-METALEAK"), "metadata leaked: {json}");
        assert!(!json.contains("sk-ABCDEFGHIJKLMNOP"), "text leaked: {json}");
        assert_eq!(receipt.turns, 2);
        assert_eq!(receipt.dropped_reasoning, 1);
        assert_eq!(receipt.redactions, 1);
    }

    // --- with-tools: tool events carried, recursively redacted, size-capped ---

    fn tool_session() -> UniversalSession {
        let mut session = UniversalSession::new("t-0001".to_string());
        session.events.push(msg("user", "run the deploy"));
        session.events.push(SessionEvent::ToolCall(ir::ToolCallEvent {
            id: None,
            parent_id: None,
            call_id: "c1".to_string(),
            name: "bash".to_string(),
            timestamp: None,
            arguments: serde_json::json!({
                "command": "curl -H 'Authorization: Bearer sk-ARGLEAK1234567890abcd'"
            }),
            metadata: BTreeMap::new(),
        }));
        session.events.push(SessionEvent::ToolResult(ir::ToolResultEvent {
            id: None,
            parent_id: None,
            call_id: "c1".to_string(),
            timestamp: None,
            output: serde_json::json!({"stdout": "token: sk-OUTLEAK1234567890abcd ok"}),
            is_error: false,
            metadata: BTreeMap::new(),
        }));
        session.events.push(msg("assistant", "deployed"));
        session
    }

    #[test]
    fn sanitize_with_tools_keeps_and_redacts_tool_events() {
        let mut session = tool_session();
        let receipt = sanitize(&mut session, true);
        assert_eq!(receipt.turns, 2);
        assert_eq!(receipt.tools, 2);
        assert_eq!(receipt.dropped_tools, 0);
        assert!(receipt.redactions >= 2, "receipt: {receipt:?}");
        let json = serde_json::to_string(&session.events).unwrap();
        assert!(json.contains("tool_call"), "tool call lost: {json}");
        assert!(json.contains("tool_result"), "tool result lost: {json}");
        assert!(!json.contains("sk-ARGLEAK"), "argument secret leaked: {json}");
        assert!(!json.contains("sk-OUTLEAK"), "output secret leaked: {json}");
    }

    #[test]
    fn sanitize_without_tools_still_drops_them() {
        let mut session = tool_session();
        let receipt = sanitize(&mut session, false);
        assert_eq!(receipt.tools, 0);
        assert_eq!(receipt.dropped_tools, 2);
        let json = serde_json::to_string(&session.events).unwrap();
        assert!(!json.contains("tool_call"), "tools should be dropped: {json}");
    }

    #[test]
    fn oversized_tool_output_is_capped() {
        let mut output = serde_json::Value::String("x".repeat(TOOL_OUTPUT_CAP * 3));
        truncate_tool_output(&mut output);
        let text = output.as_str().unwrap();
        assert!(text.len() < TOOL_OUTPUT_CAP + 200, "not capped: {}", text.len());
        assert!(text.contains("[truncated by constant"), "no marker");
        // Small outputs pass through untouched.
        let mut small = serde_json::json!({"stdout": "ok"});
        truncate_tool_output(&mut small);
        assert_eq!(small, serde_json::json!({"stdout": "ok"}));
    }

    // --- scaffold detection ---

    #[test]
    fn scaffold_is_recognized() {
        assert!(is_scaffold("<system-reminder>do x</system-reminder>"));
        assert!(is_scaffold("## Memory\nstuff"));
        assert!(is_scaffold("<environment_context> ..."));
        assert!(!is_scaffold("can you help me with this bug"));
    }

    // --- cwd slug + dir comparison ---

    #[test]
    fn cwd_slug_matches_claude_convention() {
        assert_eq!(
            cwd_slug(Path::new("/Users/x/dev/constant")),
            "-Users-x-dev-constant"
        );
    }

    #[test]
    fn same_dir_tolerates_trailing_slash() {
        let here = std::env::temp_dir();
        let with_slash = format!("{}/", here.display());
        assert!(same_dir(&with_slash, &here));
        assert!(!same_dir("/definitely/not/here", &here));
    }

    // --- codec round-trip / drift guard (F3 + S1): carry the conversation
    //     through each native format and back; if a CLI's format drifts under us
    //     this fails loudly instead of silently dropping the thread. ---

    fn roundtrip(target: SessionFormat) {
        let mut session = UniversalSession::new("rt000000-0000-0000-0000-000000000001".to_string());
        session.metadata.cwd = Some(PathBuf::from("/tmp/constant-rt"));
        session.metadata.platform_version = Some("0.0.0".to_string());
        session.metadata.created_at = chrono::DateTime::from_timestamp(1_700_000_000, 0);
        session.metadata.updated_at = chrono::DateTime::from_timestamp(1_700_000_000, 0);
        session.events.push(msg("user", "carry me across please"));
        session.events.push(msg("assistant", "carried, here I am"));

        let out = unique_tmp();
        let written =
            formats::materialize(&session, target, &out).expect("materialize to native format");
        let reloaded =
            formats::load_session(&written, SourceFormat::Auto).expect("reload native format");

        let text: String = reloaded
            .events
            .iter()
            .filter_map(|e| match e {
                SessionEvent::Message(m) => Some(
                    m.blocks
                        .iter()
                        .filter_map(|b| b.text.clone())
                        .collect::<Vec<_>>()
                        .join(" "),
                ),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");

        assert!(
            text.contains("carry me across please"),
            "{target:?} lost user turn: {text}"
        );
        assert!(
            text.contains("carried, here I am"),
            "{target:?} lost assistant turn: {text}"
        );
    }

    #[test]
    fn roundtrip_claude() {
        roundtrip(SessionFormat::Claude);
    }

    // --- B6: a torn FINAL line (child killed mid-flush) must not void the
    //     conversation; corruption mid-file must still fail loudly. ---

    #[test]
    fn loader_tolerates_torn_tail_but_not_mid_file_corruption() {
        let dir = unique_tmp();
        let mut session =
            UniversalSession::new("22222222-2222-2222-2222-222222222222".to_string());
        session.metadata.cwd = Some(PathBuf::from("/tmp/x"));
        session.metadata.platform_version = Some("2.1.0".to_string());
        session.events.push(msg("user", "hello"));
        session.events.push(msg("assistant", "hi"));
        let written =
            formats::materialize(&session, SessionFormat::Claude, &dir).expect("materialize");

        // Torn tail: half a JSON object, no closing — the kill-mid-flush shape.
        let mut text = fs::read_to_string(&written).unwrap();
        text.push_str("{\"type\":\"assistant\",\"mess");
        fs::write(&written, &text).unwrap();
        let reloaded =
            formats::load_session(&written, SourceFormat::Auto).expect("torn tail tolerated");
        assert!(
            reloaded
                .events
                .iter()
                .any(|e| matches!(e, SessionEvent::Message(_))),
            "conversation lost"
        );

        // A bad line FOLLOWED by valid data is real corruption: fail.
        let mut lines: Vec<&str> = text.lines().collect();
        lines.insert(1, "{\"type\":\"user\",\"mess");
        fs::write(&written, lines.join("\n")).unwrap();
        assert!(
            formats::load_session(&written, SourceFormat::Auto).is_err(),
            "mid-file corruption was silently swallowed"
        );
    }

    // --- B1: a FAILED overwrite must leave the existing projection intact
    //     (write goes to a tmp sibling and renames only on success). ---

    #[test]
    #[cfg(unix)]
    fn failed_overwrite_leaves_existing_projection_intact() {
        use std::os::unix::fs::PermissionsExt;
        let dir = unique_tmp();
        let out = dir.join("proj.jsonl");
        let mut session =
            UniversalSession::new("33333333-3333-3333-3333-333333333333".to_string());
        session.metadata.cwd = Some(PathBuf::from("/tmp/x"));
        session.events.push(msg("user", "keep me"));
        session.events.push(msg("assistant", "ok"));
        formats::materialize(&session, SessionFormat::Claude, &out).expect("first write");
        let before = fs::read(&out).unwrap();

        // No tmp debris after a successful write.
        let debris = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| e.path().to_string_lossy().contains("constant-tmp"))
            .count();
        assert_eq!(debris, 0, "tmp sibling left behind after success");

        // Make the directory unwritable so the next write fails mid-flight.
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();
        let result = formats::materialize(&session, SessionFormat::Claude, &out);
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(result.is_err(), "write into read-only dir should fail");
        assert_eq!(
            before,
            fs::read(&out).unwrap(),
            "a failed write damaged the existing projection"
        );
    }

    #[test]
    fn roundtrip_codex() {
        roundtrip(SessionFormat::Codex);
    }

    // --- codex loader: torn tail tolerated, mid-file corruption fails ---

    #[test]
    fn codex_loader_tolerates_torn_tail_but_not_mid_file_corruption() {
        let dir = unique_tmp();
        let mut session =
            UniversalSession::new("01970000-0000-7000-8000-000000000001".to_string());
        session.metadata.cwd = Some(PathBuf::from("/tmp/x"));
        session.metadata.platform_version = Some("0.137.0".to_string());
        session.metadata.created_at = chrono::DateTime::from_timestamp(1_700_000_000, 0);
        session.events.push(msg("user", "hello codex"));
        session.events.push(msg("assistant", "hi"));
        let written =
            formats::materialize(&session, SessionFormat::Codex, &dir).expect("materialize");

        let mut text = fs::read_to_string(&written).unwrap();
        text.push_str("{\"type\":\"response_item\",\"payl");
        fs::write(&written, &text).unwrap();
        let reloaded =
            formats::load_session(&written, SourceFormat::Auto).expect("torn tail tolerated");
        assert!(
            reloaded
                .events
                .iter()
                .any(|e| matches!(e, SessionEvent::Message(_))),
            "conversation lost"
        );

        let mut lines: Vec<&str> = text.lines().collect();
        lines.insert(1, "{\"type\":\"resp");
        fs::write(&written, lines.join("\n")).unwrap();
        assert!(
            formats::load_session(&written, SourceFormat::Auto).is_err(),
            "mid-file corruption swallowed"
        );
    }

    // --- symlink attack: a reuse target that LINKS to the source is refused ---

    #[test]
    #[cfg(unix)]
    fn carry_refuses_symlinked_reuse_target_pointing_at_source() {
        let dir = unique_tmp();
        let src = dir.join("source.json");
        let mut session = UniversalSession::new("sym-0001".to_string());
        session.metadata.cwd = Some(PathBuf::from("/tmp/x"));
        session.events.push(msg("user", "keep me intact"));
        session.events.push(msg("assistant", "ok"));
        formats::write_ir(&session, &src).expect("write IR source");
        let before = fs::read(&src).unwrap();

        let link = dir.join("sneaky-link.jsonl");
        std::os::unix::fs::symlink(&src, &link).unwrap();
        let err = distill_path(&src, Runtime::Claude, Some(("sneaky", &link)), None)
            .expect_err("symlinked reuse target must be refused");
        assert!(
            format!("{err:#}").contains("refusing to overwrite source"),
            "unexpected error: {err:#}"
        );
        assert_eq!(before, fs::read(&src).unwrap(), "source was modified");
    }

    // --- gemini loader: messages, thoughts, tool calls map to the IR ---

    #[test]
    fn gemini_loader_maps_the_full_shape() {
        let dir = unique_tmp();
        let path = dir.join("gem.json");
        fs::write(
            &path,
            r#"{
  "sessionId": "abc12345-0000-0000-0000-000000000001",
  "projectHash": "deadbeef", "startTime": "2026-06-01T10:00:00.000Z",
  "lastUpdated": "2026-06-01T10:05:00.000Z", "kind": "main",
  "messages": [
    {"id":"m1","timestamp":"2026-06-01T10:00:01.000Z","type":"user",
     "content":[{"text":"part one"},{"text":"part two"}]},
    {"id":"m2","timestamp":"2026-06-01T10:00:05.000Z","type":"gemini",
     "content":"the reply",
     "thoughts":[{"subject":"S","description":"D"}],
     "toolCalls":[{"id":"t1","name":"shell","args":{"command":"ls"},"result":[{"ok":true}]}],
     "model":"gemini-3-pro"},
    {"id":"m3","timestamp":"2026-06-01T10:01:00.000Z","type":"info","content":"notice"}
  ]
}"#,
        )
        .unwrap();

        let d = distill_source(&path, true).expect("gemini load");
        assert_eq!(d.receipt.turns, 2, "{:?}", d.receipt);
        assert_eq!(d.receipt.tools, 2, "{:?}", d.receipt);
        assert_eq!(d.receipt.dropped_reasoning, 1, "{:?}", d.receipt);
        assert_eq!(d.session.metadata.model.as_deref(), Some("gemini-3-pro"));
        let user_text = d
            .session
            .events
            .iter()
            .find_map(|e| match e {
                SessionEvent::Message(m) if m.role == "user" => Some(
                    m.blocks
                        .iter()
                        .filter_map(|b| b.text.clone())
                        .collect::<Vec<_>>()
                        .join("|"),
                ),
                _ => None,
            })
            .unwrap();
        assert_eq!(user_text, "part one|part two");
    }

    // --- opencode export parser: tolerates the trailing status line ---

    #[test]
    fn opencode_parse_export_tolerates_status_trailer() {
        let text = r#"{"info":{"id":"ses_x1","directory":"/tmp/p","title":"t","version":"1.14.48",
"time":{"created":1700000000000,"updated":1700000001000}},
"messages":[{"info":{"id":"msg_1","role":"user","time":{"created":1700000000100}},
"parts":[{"type":"text","text":"hi there"}]}]}
Exporting session: ses_x1"#;
        let session = formats::opencode::parse_export(text).expect("trailer tolerated");
        assert_eq!(session.metadata.session_id, "ses_x1");
        assert_eq!(session.events.len(), 1);
    }

    // --- IR forward-compat: unknown fields in future IR files are ignored ---

    #[test]
    fn ir_with_unknown_future_fields_still_loads() {
        let text = r#"{
  "ir_version": "transession/v1",
  "some_future_top_field": {"x": 1},
  "metadata": {"session_id": "f-1", "future_meta": true},
  "events": [
    {"kind": "message", "role": "user", "future_event_field": 7,
     "blocks": [{"kind": "text", "text": "hello", "future_block_field": []}]}
  ]
}"#;
        let session: UniversalSession =
            serde_json::from_str(text).expect("unknown fields must not break old readers");
        assert_eq!(session.events.len(), 1);
    }

    // --- F1: a carry must NEVER modify the source session (data-loss guard) ---

    #[test]
    fn carry_never_modifies_source() {
        let dir = unique_tmp();
        let src = dir.join("source.json");
        let mut session = UniversalSession::new("src-0000".to_string());
        session.metadata.cwd = Some(PathBuf::from("/tmp/x"));
        session.events.push(msg("user", "keep me intact"));
        session.events.push(msg("assistant", "ok"));
        formats::write_ir(&session, &src).expect("write IR source");
        let before = fs::read(&src).expect("read source before");

        // Carry it, directing output to a tempdir file (reuse) so the test never
        // touches the real ~/.claude store.
        let out = dir.join("claude-out.jsonl");
        let _ = distill_path(&src, Runtime::Claude, Some(("dummy-claude-id", &out)), None)
            .expect("carry");

        let after = fs::read(&src).expect("read source after");
        assert_eq!(
            before, after,
            "carry modified the source file — F1 violation"
        );
        assert!(out.exists(), "carry did not write the target");
    }

    #[test]
    fn carry_refuses_to_reuse_the_source_as_target() {
        let dir = unique_tmp();
        let src = dir.join("source.jsonl");
        let mut session = UniversalSession::new("src-0000".to_string());
        session.metadata.cwd = Some(PathBuf::from("/tmp/x"));
        session.events.push(msg("user", "keep me intact"));
        session.events.push(msg("assistant", "ok"));
        formats::write_ir(&session, &src).expect("write IR source");
        let before = fs::read(&src).expect("read source before");

        let err = distill_path(
            &src,
            Runtime::Claude,
            Some(("bad-reuse-target", &src)),
            None,
        )
        .expect_err("source reuse should be refused");

        assert!(
            format!("{err:#}").contains("refusing to overwrite source session"),
            "unexpected error: {err:#}"
        );
        assert_eq!(
            before,
            fs::read(&src).expect("read source after"),
            "refused carry still modified the source"
        );
    }

    // --- the fields Claude's resume parser requires (guards "Failed to resume") ---

    #[test]
    fn claude_output_has_resume_schema() {
        let dir = unique_tmp();
        let mut session = UniversalSession::new("11111111-1111-1111-1111-111111111111".to_string());
        session.metadata.cwd = Some(PathBuf::from("/tmp/x"));
        session.metadata.platform_version = Some("2.1.0".to_string());
        session.metadata.created_at = chrono::DateTime::from_timestamp(1_700_000_000, 0);
        session.events.push(msg("user", "hello"));
        session.events.push(msg("assistant", "hi"));
        let written =
            formats::materialize(&session, SessionFormat::Claude, &dir).expect("materialize");
        let text = fs::read_to_string(&written).expect("read claude output");
        for needle in [
            "\"sessionId\"",
            "\"version\"",
            "\"type\":\"user\"",
            "\"type\":\"assistant\"",
        ] {
            assert!(text.contains(needle), "claude output missing {needle}");
        }
    }

    // --- export master: strips nested data/metadata/extra AND redacts text ---

    #[test]
    fn export_ir_strips_and_redacts() {
        let dir = unique_tmp();
        let src = dir.join("src.json");
        let mut user = match msg("user", "my key sk-ABCDEFGHIJKLMNOPqrst") {
            SessionEvent::Message(m) => m,
            _ => unreachable!(),
        };
        user.blocks.push(ir::ContentBlock {
            kind: "data".to_string(),
            text: None,
            data: Some(serde_json::json!({"leak": "sk-DATALEAK1234567890abcd"})),
        });
        user.metadata.insert(
            "leakmeta".to_string(),
            serde_json::json!("sk-METALEAK1234567890abcd"),
        );
        let mut session = UniversalSession::new("e-0000".to_string());
        session.events.push(SessionEvent::Message(user));
        session.metadata.extra.insert(
            "scaffold".to_string(),
            serde_json::json!("sk-EXTRALEAK1234567890abcd"),
        );
        formats::write_ir(&session, &src).expect("write IR source");

        let (json, _n) = export_ir(&src).expect("export");
        assert!(
            !json.contains("sk-ABCDEFGHIJKLMNOP"),
            "text key leaked: {json}"
        );
        assert!(!json.contains("sk-DATALEAK"), "block.data leaked");
        assert!(!json.contains("sk-METALEAK"), "message metadata leaked");
        assert!(!json.contains("sk-EXTRALEAK"), "metadata.extra leaked");
        assert!(json.contains("[redacted-key]"), "text not redacted");
    }
}
