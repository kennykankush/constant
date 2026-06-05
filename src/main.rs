//! Constant — one conversation, any agent runtime.
//!
//! `constant host [runtime]` boots an agent CLI inside a Constant-owned PTY so
//! you can switch the active runtime live (tmux-style prefix key) without losing
//! the conversation.

mod alembic;
mod host;
mod runtime;
mod trail;

use anyhow::{bail, Context, Result};
use runtime::Runtime;
use std::path::{Path, PathBuf};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();

    match args.first().map(String::as_str) {
        Some("host") => run_host(&args[1..]),
        // `carry` is the headless verb; `distill` kept as an alias (alembic's
        // internal step is still called distillation).
        Some("carry") | Some("distill") => run_carry(&args[1..]),
        Some("sessions") | Some("ls") => run_sessions(&args[1..]),
        Some("export") => run_export(&args[1..]),
        Some("doctor") => run_doctor(&args[1..]),
        Some("trail") => run_trail(&args[1..]),
        Some("keys") => host::debug_keys(),
        Some("-V") | Some("--version") => {
            println!("constant {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("-h") | Some("--help") | None => {
            print_help();
            Ok(())
        }
        Some(other) => {
            eprintln!("constant: unknown command: {other}\n");
            print_help();
            std::process::exit(1);
        }
    }
}

/// Read the value following a flag at `rest[i]`, erroring if it's missing or is
/// itself another flag (so `--session --json` / `--from` with no value fail loudly
/// instead of silently swallowing the next token or falling through).
fn flag_value(rest: &[String], i: usize, flag: &str) -> Result<String> {
    match rest.get(i + 1) {
        Some(v) if !v.starts_with("--") => Ok(v.clone()),
        _ => bail!("{flag} needs a value"),
    }
}

/// Neutralize control characters before printing transcript/metadata-derived text
/// to the terminal — a crafted session could embed ANSI/OSC bytes that manipulate
/// terminal state or spoof output. (JSON output is escaped by the serializer.)
fn term_safe(s: &str) -> String {
    s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect()
}

/// True if `out` resolves to the same file as `src`, so `export --out` can never
/// clobber the session it's reading. For existing files this compares device+inode
/// (catching hard links AND symlinks, which path canonicalization alone misses);
/// for a not-yet-existing `out` it compares resolved parent-dir + filename.
fn same_file(src: &Path, out: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let (Ok(a), Ok(b)) = (std::fs::metadata(src), std::fs::metadata(out)) {
            return a.dev() == b.dev() && a.ino() == b.ino();
        }
    }
    let Ok(src_c) = src.canonicalize() else {
        return false;
    };
    if let Ok(out_c) = out.canonicalize() {
        return src_c == out_c;
    }
    // `out` doesn't exist yet: resolve its parent dir + filename.
    match (out.parent(), out.file_name()) {
        (Some(parent), Some(name)) => {
            let parent = if parent.as_os_str().is_empty() {
                Path::new(".")
            } else {
                parent
            };
            parent
                .canonicalize()
                .map(|p| p.join(name) == src_c)
                .unwrap_or(false)
        }
        _ => false,
    }
}

fn run_host(rest: &[String]) -> Result<()> {
    let mut runtime_str = "codex".to_string();
    let mut prefix_str = std::env::var("CONSTANT_PREFIX").unwrap_or_else(|_| "C-b".to_string());

    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--prefix" | "-p" => {
                prefix_str = rest
                    .get(i + 1)
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("--prefix needs a value, e.g. --prefix C-t"))?;
                i += 2;
            }
            s if !s.starts_with('-') => {
                runtime_str = s.to_string();
                i += 1;
            }
            other => bail!("unknown flag: {other}"),
        }
    }

    let (prefix_byte, prefix_label) = host::parse_prefix(&prefix_str)?;
    host::run(Runtime::parse(&runtime_str)?, prefix_byte, prefix_label)
}

fn run_carry(rest: &[String]) -> Result<()> {
    let mut from: Option<String> = None;
    let mut to: Option<String> = None;
    let mut session: Option<PathBuf> = None;
    let mut json = false;
    let mut dry_run = false;

    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--from" => {
                from = Some(flag_value(rest, i, "--from")?);
                i += 2;
            }
            "--to" => {
                to = Some(flag_value(rest, i, "--to")?);
                i += 2;
            }
            "--session" => {
                session = Some(PathBuf::from(flag_value(rest, i, "--session")?));
                i += 2;
            }
            "--json" => {
                json = true;
                i += 1;
            }
            "--dry-run" => {
                dry_run = true;
                i += 1;
            }
            other => bail!("unknown flag: {other}"),
        }
    }

    if session.is_some() && from.is_some() {
        bail!("--from cannot be combined with --session (--session selects the file directly)");
    }

    let to = Runtime::parse(&to.context("carry requires --to codex|claude")?)?;

    let here = std::env::current_dir().ok();

    // Resolve the source: its path, native id, and which runtime it belongs to.
    let (src_path, src_id, from_rt) = match &session {
        Some(p) => {
            // Resolve a path OR session id to the real file, then identify it.
            let resolved = alembic::resolve_session(p)?;
            let (rt, id) = alembic::identify(&resolved)
                .with_context(|| format!("could not identify session {}", resolved.display()))?;
            (resolved, id, rt)
        }
        None => {
            let rt = Runtime::parse(
                from.as_deref()
                    .context("carry requires --from or --session")?,
            )?;
            let (p, id) = alembic::active_session(rt, here.as_deref())
                .context("no conversation found here to carry")?;
            (p, id, rt)
        }
    };

    if dry_run {
        let p = alembic::preview(&src_path)?;
        if json {
            println!(
                "{}",
                serde_json::json!({
                    "dry_run": true,
                    "from": from_rt.label(),
                    "to": to.label(),
                    "messages": p.message_count,
                    "root": p.root_name,
                    "source": src_path.display().to_string(),
                })
            );
        } else {
            println!(
                "dry-run: would carry {} message(s) {} → {} (root: {})",
                p.message_count,
                from_rt.label(),
                to.label(),
                term_safe(p.root_name.as_deref().unwrap_or("?"))
            );
        }
        return Ok(());
    }

    // Trail: which conversation this source belongs to, the next hop number, and
    // any existing projection for the target — so repeated headless carries reuse
    // the stable pair (overwrite Constant's own projection) instead of minting a
    // new file every time, exactly like the interactive harness.
    let (conv_id, last_n, projs) = trail::resume(&src_path, &src_id);
    let slug = trail::slug(
        &alembic::root_name(&src_path, from_rt).unwrap_or_else(|| "conversation".to_string()),
    );
    let n = last_n + 1;
    let title = trail::title(n, from_rt, &slug);

    // Reuse the conversation's existing projection for the target (stable pair) —
    // but NEVER one that is the source file we're reading, which would rewrite the
    // source in place (e.g. a same-runtime carry). Mint a fresh one in that case.
    let reuse_owned = projs
        .into_iter()
        .find(|(rt, _, _)| *rt == to)
        .map(|(_, id, p)| (id, p))
        .filter(|(_, p)| !same_file(&src_path, p));
    let reuse = reuse_owned
        .as_ref()
        .map(|(id, p)| (id.as_str(), p.as_path()));
    let (id, written, cwd) = alembic::distill_path(&src_path, to, reuse, Some(&title))?;
    // Record the trail under the CONVERSATION's own cwd (carried from the source
    // session), not the directory `carry` happened to be invoked from — so
    // `constant trail` in that project shows its own threads. Fall back to the
    // invocation dir only if the session has no recorded cwd.
    trail::record(
        n,
        &conv_id,
        &slug,
        cwd.as_deref().or(here.as_deref()),
        from_rt,
        to,
        &id,
        &written,
        &title,
    );

    let resume = match to {
        Runtime::Claude => format!("claude -r {id}"),
        Runtime::Codex => format!("codex resume {id}"),
    };

    if json {
        let out = serde_json::json!({
            "id": id,
            "from": from_rt.label(),
            "to": to.label(),
            "cwd": cwd.as_ref().map(|p| p.display().to_string()),
            "resume": resume,
            "trail": title,
        });
        println!("{out}");
    } else {
        println!("carried → {} session {id}  ({title})", to.label());
        if let Some(cwd) = cwd {
            println!("cwd: {}", term_safe(&cwd.display().to_string()));
        }
        println!("resume with: {resume}");
    }
    Ok(())
}

fn run_export(rest: &[String]) -> Result<()> {
    let mut from: Option<String> = None;
    let mut session: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--from" => {
                from = Some(flag_value(rest, i, "--from")?);
                i += 2;
            }
            "--session" => {
                session = Some(PathBuf::from(flag_value(rest, i, "--session")?));
                i += 2;
            }
            "--out" => {
                out = Some(PathBuf::from(flag_value(rest, i, "--out")?));
                i += 2;
            }
            other => bail!("unknown flag: {other}"),
        }
    }

    if session.is_some() && from.is_some() {
        bail!("--from cannot be combined with --session (--session selects the file directly)");
    }

    let here = std::env::current_dir().ok();
    let src_path = match (&session, &from) {
        // Resolve --session (path OR session id) to the real file, so export_ir
        // and the same-file guard both operate on the actual path.
        (Some(p), _) => alembic::resolve_session(p)?,
        (None, Some(f)) => {
            let rt = Runtime::parse(f)?;
            alembic::active_session(rt, here.as_deref())
                .context("no conversation found here to export")?
                .0
        }
        (None, None) => bail!("export requires --from or --session"),
    };

    let (ir, messages) = alembic::export_ir(&src_path)?;
    match out {
        Some(path) => {
            if same_file(&src_path, &path) {
                bail!(
                    "refusing to overwrite the source session at {}",
                    term_safe(&path.display().to_string())
                );
            }
            std::fs::write(&path, ir)
                .with_context(|| format!("failed to write {}", path.display()))?;
            println!(
                "exported {messages} message(s) → {}",
                term_safe(&path.display().to_string())
            );
        }
        // Raw IR to stdout — pipe/redirect-friendly.
        None => println!("{ir}"),
    }
    Ok(())
}

fn run_trail(rest: &[String]) -> Result<()> {
    let mut all = false;
    for arg in rest {
        match arg.as_str() {
            "--all" => all = true,
            other => bail!("unknown flag: {other}"),
        }
    }
    let cwd = if all { None } else { std::env::current_dir().ok() };
    trail::print(cwd.as_deref())
}

fn run_sessions(rest: &[String]) -> Result<()> {
    let mut from: Option<String> = None;
    let mut all = false;
    let mut json = false;
    let mut titles = false;
    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--from" => {
                from = Some(flag_value(rest, i, "--from")?);
                i += 2;
            }
            "--all" => {
                all = true;
                i += 1;
            }
            "--json" => {
                json = true;
                i += 1;
            }
            "--titles" => {
                titles = true;
                i += 1;
            }
            other => bail!("unknown flag: {other}"),
        }
    }

    let cwd = if all { None } else { std::env::current_dir().ok() };
    let runtimes = match from {
        Some(s) => vec![Runtime::parse(&s)?],
        None => vec![Runtime::Codex, Runtime::Claude],
    };
    let mut sessions = Vec::new();
    for rt in runtimes {
        sessions.extend(alembic::list_sessions(rt, cwd.as_deref(), titles));
    }
    sessions.sort_by(|a, b| b.mtime.cmp(&a.mtime));

    if json {
        let arr: Vec<_> = sessions
            .iter()
            .map(|s| {
                serde_json::json!({
                    "runtime": s.runtime,
                    "id": s.id,
                    "path": s.path.display().to_string(),
                    "cwd": s.cwd,
                    "mtime": s.mtime.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
                    "has_conversation": s.has_conversation,
                    "title": s.title,
                })
            })
            .collect();
        println!("{}", serde_json::Value::Array(arr));
    } else if sessions.is_empty() {
        let scope = if all { "" } else { " in this directory (try --all)" };
        println!("no sessions found{scope}");
    } else {
        for s in &sessions {
            // `·` marks a session known to be empty (only determinable with --titles).
            let mark = match s.has_conversation {
                Some(false) => "·",
                _ => " ",
            };
            println!(
                "{mark} {:6}  {}  {}",
                s.runtime,
                term_safe(&s.id),
                term_safe(s.title.as_deref().unwrap_or(""))
            );
        }
    }
    Ok(())
}

fn run_doctor(rest: &[String]) -> Result<()> {
    let mut json = false;
    for arg in rest {
        match arg.as_str() {
            "--json" => json = true,
            other => bail!("unknown flag: {other}"),
        }
    }
    let r = alembic::doctor();
    if json {
        println!(
            "{}",
            serde_json::json!({
                "codex": {
                    "version": r.codex_version,
                    "supported": alembic::SUPPORTED_CODEX,
                    "store": r.codex_store,
                    "db": r.codex_db,
                },
                "claude": {
                    "version": r.claude_version,
                    "supported": alembic::SUPPORTED_CLAUDE,
                    "store": r.claude_store,
                },
            })
        );
    } else {
        let mark = |b: bool| if b { "ok" } else { "MISSING" };
        println!("constant doctor");
        println!(
            "  codex  : {} (cli {}, sessions {}, db {}) — validated against {}.x",
            r.codex_version.as_deref().unwrap_or("not found"),
            mark(r.codex_version.is_some()),
            mark(r.codex_store),
            mark(r.codex_db),
            alembic::SUPPORTED_CODEX,
        );
        println!(
            "  claude : {} (cli {}, projects {}) — validated against {}.x",
            r.claude_version.as_deref().unwrap_or("not found"),
            mark(r.claude_version.is_some()),
            mark(r.claude_store),
            alembic::SUPPORTED_CLAUDE,
        );
    }
    Ok(())
}

fn print_help() {
    println!(
        r#"Constant — one conversation, any agent runtime.

USAGE:
  constant host [codex|claude] [--prefix C-t]
        Host an agent CLI in a Constant PTY (default runtime: codex, prefix: Ctrl-B)

  constant carry --to codex|claude [--from codex|claude | --session PATH] [--json] [--dry-run]
        Headless: carry a conversation into the target runtime's native session and
        print the resume command (no terminal). --json for machine output;
        --dry-run previews without writing. (`distill` is an alias.)

  constant sessions [--from codex|claude] [--all] [--titles] [--json]
        List carryable sessions (this directory, or --all). --titles adds a preview
        (reads transcripts; slower on large stores). Discovery for carry.

  constant export (--from codex|claude | --session PATH) [--out FILE]
        Export a conversation as the neutral IR master (distilled + redacted JSON):
        a portable, runtime-agnostic copy. Writes to --out FILE, else stdout.

  constant doctor [--json]
        Preflight: which runtimes/versions are installed and whether supported.

  constant trail [--all]
        Show the switch lineage (per directory, or --all) from ~/.constant/trail.jsonl

PREFIX KEY:
  Default is Ctrl-B. If you run inside tmux (which also uses Ctrl-B), pick another:
      constant host codex --prefix C-t
      CONSTANT_PREFIX=C-g constant host codex

INSIDE A HOSTED SESSION (press the prefix, then):
  c              switch to claude
  x              switch to codex
  :              open the command line (e.g. `switch claude`, `quit`)
  d              detach / quit the harness
  <prefix> again send a literal prefix key to the child
"#
    );
}
