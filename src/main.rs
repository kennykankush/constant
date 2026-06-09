//! Constant — one conversation, any agent runtime.
//!
//! `constant host [runtime]` boots an agent CLI inside a Constant-owned PTY so
//! you can switch the active runtime live (tmux-style prefix key) without losing
//! the conversation.

mod alembic;
mod host;
mod runtime;
mod trail;

use anyhow::{Context, Result, bail};
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
        Some("resume") => run_resume_cmd(&args[1..]),
        Some("export") => run_export(&args[1..]),
        Some("doctor") => run_doctor(&args[1..]),
        Some("status") => run_status(&args[1..]),
        Some("trail") => run_trail(&args[1..]),
        Some("snapshots") | Some("records") => run_snapshots(&args[1..]),
        Some("restore") => run_restore(&args[1..]),
        Some("route") | Some("routes") => run_route(&args[1..]),
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
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

fn run_host(rest: &[String]) -> Result<()> {
    let mut runtime_str = "codex".to_string();
    let mut prefix_str = std::env::var("CONSTANT_PREFIX").unwrap_or_else(|_| "C-b".to_string());
    let mut with_tools = false;

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
            "--with-tools" => {
                with_tools = true;
                i += 1;
            }
            s if !s.starts_with('-') => {
                runtime_str = s.to_string();
                i += 1;
            }
            other => bail!("unknown flag: {other}"),
        }
    }

    let (prefix_byte, prefix_label) = host::parse_prefix(&prefix_str)?;
    host::run(
        Runtime::parse(&runtime_str)?,
        None,
        with_tools,
        prefix_byte,
        prefix_label,
    )
}

fn run_carry(rest: &[String]) -> Result<()> {
    let mut from: Option<String> = None;
    let mut to: Option<String> = None;
    let mut session: Option<PathBuf> = None;
    let mut json = false;
    let mut dry_run = false;
    let mut debug = false;
    let mut new = false;
    let mut with_tools = false;

    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--with-tools" => {
                with_tools = true;
                i += 1;
            }
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
            "--debug" => {
                debug = true;
                i += 1;
            }
            "--new" => {
                new = true;
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
            let (p, id) = alembic::active_session(rt, here.as_deref(), None)
                .context("no conversation found here to carry")?;
            (p, id, rt)
        }
    };

    // Load + distill ONCE — the trail name, the dry-run preview, and the carry
    // itself all share the same parsed source.
    let mut distilled = alembic::distill_source(&src_path, with_tools)?;
    let root = distilled.root_name();

    // Trail: which conversation this source belongs to, the next hop number, and
    // any existing projection for the target — so repeated headless carries reuse
    // the stable pair (overwrite Constant's own projection) instead of minting a
    // new file every time, exactly like the interactive harness.
    let (conv_id, last_n, projs) = trail::resume(&src_path, &src_id);
    let slug = trail::slug(&root.clone().unwrap_or_else(|| "conversation".to_string()));
    let n = last_n + 1;
    let title = trail::title(n, from_rt, &slug);

    // Reuse the conversation's existing projection for the target (stable pair)
    // unless the caller explicitly asks for a fresh continuation. Never reuse
    // the source file itself, which would rewrite the source in place.
    let reuse_owned = if new {
        None
    } else {
        projs
            .iter()
            .find(|(rt, _, _)| *rt == to)
            .map(|(_, id, p)| (id.clone(), p.clone()))
            .filter(|(_, p)| !alembic::same_file(&src_path, p))
    };
    let reuse = reuse_owned
        .as_ref()
        .map(|(id, p)| (id.as_str(), p.as_path()));
    let mode = if reuse_owned.is_some() {
        "refresh-existing"
    } else {
        "new-fork"
    };

    let receipt = distilled.receipt;
    let receipt_json = serde_json::json!({
        "turns": receipt.turns,
        "tools": receipt.tools,
        "dropped_tools": receipt.dropped_tools,
        "dropped_reasoning": receipt.dropped_reasoning,
        "dropped_scaffold": receipt.dropped_scaffold,
        "redactions": receipt.redactions,
    });

    if dry_run {
        if debug && !json {
            trail::print_carry_debug(&src_path, &src_id, &conv_id, &slug, from_rt, to, n, reuse);
        }
        if json {
            let mut out = serde_json::json!({
                "dry_run": true,
                "from": from_rt.label(),
                "to": to.label(),
                "messages": receipt.turns,
                "root": root,
                "source": src_path.display().to_string(),
                "receipt": receipt_json,
            });
            if debug {
                out["debug"] = serde_json::json!({
                    "conversation": conv_id,
                    "source_id": src_id,
                    "source_path": src_path.display().to_string(),
                    "target_runtime": to.label(),
                    "mode": mode,
                    "new": new,
                    "reuse_id": reuse_owned.as_ref().map(|(id, _)| id.as_str()),
                    "next_event": format!("t{n:02}"),
                });
            }
            println!("{out}");
        } else {
            println!(
                "dry-run: would carry {} message(s) {} → {} (root: {})",
                receipt.turns,
                from_rt.label(),
                to.label(),
                term_safe(root.as_deref().unwrap_or("?"))
            );
        }
        return Ok(());
    }

    if debug && !json {
        trail::print_carry_debug(&src_path, &src_id, &conv_id, &slug, from_rt, to, n, reuse);
    }
    // The record comes first: write this hop's snapshot volume (distilled IR)
    // before materializing the native copy. A failed record never blocks the
    // carry, but it is announced — a silent gap is the one unforgivable thing.
    let snapshot = trail::snapshot_path(&conv_id, n, from_rt).and_then(|p| {
        match alembic::write_snapshot(&distilled.session, &p) {
            Ok(()) => Some(p),
            Err(e) => {
                eprintln!("warning: record not written: {e}");
                None
            }
        }
    });
    let (id, written, cwd) =
        alembic::distill_write(&mut distilled, &src_path, to, reuse, Some(&title))?;
    // Record the trail under the CONVERSATION's own cwd (carried from the source
    // session), not the directory `carry` happened to be invoked from — so
    // `constant trail` in that project shows its own threads. Fall back to the
    // invocation dir only if the session has no recorded cwd. A failed ledger
    // append is surfaced: pair-reuse depends on it.
    if let Err(e) = trail::record(
        n,
        &conv_id,
        &slug,
        cwd.as_deref().or(here.as_deref()),
        &src_id,
        &src_path,
        from_rt,
        to,
        &id,
        &written,
        &title,
        mode,
        snapshot.as_deref(),
    ) {
        eprintln!("warning: trail ledger write failed: {e}");
    }

    let resume = match to {
        Runtime::Claude => format!("claude -r {id}"),
        Runtime::Codex => format!("codex resume {id}"),
    };

    if json {
        let mut out = serde_json::json!({
            "id": &id,
            "from": from_rt.label(),
            "to": to.label(),
            "cwd": cwd.as_ref().map(|p| p.display().to_string()),
            "resume": &resume,
            "trail": &title,
            "receipt": receipt_json,
            "snapshot": snapshot.as_ref().map(|p| p.display().to_string()),
        });
        if debug {
            out["debug"] = serde_json::json!({
                "conversation": conv_id,
                "source_id": src_id,
                "source_path": src_path.display().to_string(),
                "target_runtime": to.label(),
                "mode": mode,
                "new": new,
                "target_id": &id,
                "target_path": written.display().to_string(),
                "next_event": format!("t{n:02}"),
            });
        }
        println!("{out}");
    } else {
        println!("carried → {} session {id}  ({title})", to.label());
        println!("{}", receipt.summary());
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
            alembic::active_session(rt, here.as_deref(), None)
                .context("no conversation found here to export")?
                .0
        }
        (None, None) => bail!("export requires --from or --session"),
    };

    let (ir, messages) = alembic::export_ir(&src_path)?;
    match out {
        Some(path) => {
            if alembic::same_file(&src_path, &path) {
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
    let mut events = false;
    for arg in rest {
        match arg.as_str() {
            "--all" => all = true,
            "--events" => events = true,
            other => bail!("unknown flag: {other}"),
        }
    }
    let cwd = if all {
        None
    } else {
        std::env::current_dir().ok()
    };
    if events {
        trail::print_events(cwd.as_deref())
    } else {
        trail::print(cwd.as_deref())
    }
}

/// `constant resume [QUERY]` — re-host a conversation from the trail: pick its
/// latest projection and open it live in the harness (identity declared from
/// birth, no filesystem detection). If every projection is gone, reprint one
/// from the latest record volume first. Without a TTY, prints the native
/// resume command instead so it stays scriptable.
fn run_resume_cmd(rest: &[String]) -> Result<()> {
    use std::io::IsTerminal;

    let mut query: Option<String> = None;
    let mut runtime_in: Option<Runtime> = None;
    let mut all = false;
    let mut list = false;
    let mut with_tools = false;
    let mut prefix_str = std::env::var("CONSTANT_PREFIX").unwrap_or_else(|_| "C-b".to_string());

    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--with-tools" => {
                with_tools = true;
                i += 1;
            }
            "--in" => {
                runtime_in = Some(Runtime::parse(&flag_value(rest, i, "--in")?)?);
                i += 2;
            }
            "--all" => {
                all = true;
                i += 1;
            }
            "--list" => {
                list = true;
                i += 1;
            }
            "--prefix" | "-p" => {
                prefix_str = flag_value(rest, i, "--prefix")?;
                i += 2;
            }
            s if !s.starts_with('-') => {
                if query.is_some() {
                    bail!("resume takes one query (slug or conversation id)");
                }
                query = Some(s.to_string());
                i += 1;
            }
            other => bail!("unknown flag: {other}"),
        }
    }

    let cwd = if all {
        None
    } else {
        std::env::current_dir().ok()
    };
    let convs = trail::conversations(cwd.as_deref());
    if convs.is_empty() {
        let scope = if all {
            String::new()
        } else {
            " here (try --all)".to_string()
        };
        bail!("no conversations in the trail{scope} — start one with `constant host`");
    }

    if list {
        print_resume_list(&convs);
        return Ok(());
    }

    // Pick the conversation: newest when no query; else match slug substring
    // or conversation-id prefix.
    let matches: Vec<&trail::ConversationView> = match &query {
        None => vec![&convs[0]],
        Some(q) => {
            let ql = q.to_lowercase();
            convs
                .iter()
                .filter(|c| c.slug.to_lowercase().contains(&ql) || c.conversation.starts_with(q.as_str()))
                .collect()
        }
    };
    let conv = match matches.len() {
        0 => {
            print_resume_list(&convs);
            bail!(
                "no conversation matches `{}`",
                term_safe(query.as_deref().unwrap_or(""))
            );
        }
        1 => matches[0],
        _ => {
            println!("`{}` is ambiguous:", term_safe(query.as_deref().unwrap_or("")));
            for c in &matches {
                println!("  {}", term_safe(&c.slug));
            }
            bail!("narrow the query");
        }
    };

    // Pick the projection: the requested runtime, else the latest hop's target.
    let projection = match runtime_in {
        Some(rt) => conv.projections.iter().find(|p| p.runtime == rt.label()),
        None => conv.projections.iter().max_by_key(|p| p.last_n),
    };

    let (rt, id, note) = match projection {
        Some(p) => (Runtime::parse(&p.runtime)?, p.id.clone(), None),
        None => {
            // Lost-record doctrine: every live projection is gone — reprint a
            // fresh one from the conversation's latest record volume.
            let snap = trail::latest_snapshot(&conv.conversation).with_context(|| {
                format!(
                    "`{}` has no live projection and no record volume to restore from",
                    conv.slug
                )
            })?;
            let restored = restore_session(&snap, runtime_in)?;
            let note = format!(
                "projection missing — restored from the record ({})",
                restored.receipt.summary()
            );
            (restored.to, restored.id, Some(note))
        }
    };

    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        if let Some(n) = &note {
            println!("{}", term_safe(n));
        }
        println!("conversation: {}", term_safe(&conv.slug));
        println!(
            "not a terminal — resume manually with: {}",
            native_resume_cmd(rt, &id)
        );
        return Ok(());
    }

    let (prefix_byte, prefix_label) = host::parse_prefix(&prefix_str)?;
    if let Some(n) = &note {
        println!("{n}");
    }
    host::run(rt, Some(&id), with_tools, prefix_byte, prefix_label)
}

fn print_resume_list(convs: &[trail::ConversationView]) {
    println!("resumable conversations (newest first):");
    for c in convs {
        let lives_in = if c.projections.is_empty() {
            "record only".to_string()
        } else {
            c.projections
                .iter()
                .map(|p| p.runtime.as_str())
                .collect::<Vec<_>>()
                .join("·")
        };
        println!("  {:<44}  {}", term_safe(&c.slug), lives_in);
        println!("      constant resume {}", term_safe(&c.slug));
    }
}

fn run_snapshots(rest: &[String]) -> Result<()> {
    let mut all = false;
    for arg in rest {
        match arg.as_str() {
            "--all" => all = true,
            other => bail!("unknown flag: {other}"),
        }
    }
    let cwd = if all {
        None
    } else {
        std::env::current_dir().ok()
    };
    trail::print_snapshots(cwd.as_deref())
}

/// A restore's outcome: the freshly minted projection and where it came from.
struct Restored {
    to: Runtime,
    id: String,
    title: String,
    receipt: alembic::CarryReceipt,
    cwd: Option<PathBuf>,
    source: PathBuf,
}

/// Reprint a fresh native session from a record volume (or any session file).
/// Always MINTS a new projection: certified copies circulate, the record never
/// gets written on. The restore is logged to the trail so lineage stays joined.
fn restore_session(source: &Path, to_override: Option<Runtime>) -> Result<Restored> {
    let resolved = alembic::resolve_session(source)?;
    let (src_rt, src_id) = alembic::identify(&resolved)
        .with_context(|| format!("could not identify {}", resolved.display()))?;
    // Default target: the runtime the recorded conversation came from.
    let to = to_override.unwrap_or(src_rt);

    // Fidelity to the record: a restore reprints what the volume holds — if
    // tools were carried into it, they come back out (already redacted/capped).
    let mut distilled = alembic::distill_source(&resolved, true)?;
    let root = distilled.root_name();
    let (conv_id, last_n, _) = trail::resume(&resolved, &src_id);
    let slug = trail::slug(&root.unwrap_or_else(|| "conversation".to_string()));
    let n = last_n + 1;
    let title = trail::title(n, src_rt, &slug);

    let (id, written, cwd) =
        alembic::distill_write(&mut distilled, &resolved, to, None, Some(&title))?;
    let here = std::env::current_dir().ok();
    if let Err(e) = trail::record(
        n,
        &conv_id,
        &slug,
        cwd.as_deref().or(here.as_deref()),
        &src_id,
        &resolved,
        src_rt,
        to,
        &id,
        &written,
        &title,
        "restore",
        Some(&resolved),
    ) {
        eprintln!("warning: trail ledger write failed: {e}");
    }

    Ok(Restored {
        to,
        id,
        title,
        receipt: distilled.receipt,
        cwd,
        source: resolved,
    })
}

fn native_resume_cmd(rt: Runtime, id: &str) -> String {
    match rt {
        Runtime::Claude => format!("claude -r {id}"),
        Runtime::Codex => format!("codex resume {id}"),
    }
}

/// `constant restore <snapshot> [--to codex|claude]`.
fn run_restore(rest: &[String]) -> Result<()> {
    let mut source: Option<PathBuf> = None;
    let mut to: Option<String> = None;
    let mut json = false;

    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--to" => {
                to = Some(flag_value(rest, i, "--to")?);
                i += 2;
            }
            "--json" => {
                json = true;
                i += 1;
            }
            s if !s.starts_with("--") => {
                source = Some(PathBuf::from(s));
                i += 1;
            }
            other => bail!("unknown flag: {other}"),
        }
    }

    let source = source.context(
        "restore needs a snapshot path or session id (see `constant snapshots`)",
    )?;
    let to = to.map(|s| Runtime::parse(&s)).transpose()?;
    let Restored {
        to,
        id,
        title,
        receipt,
        cwd,
        source: resolved,
    } = restore_session(&source, to)?;

    let resume = native_resume_cmd(to, &id);

    if json {
        println!(
            "{}",
            serde_json::json!({
                "restored": true,
                "id": &id,
                "to": to.label(),
                "from_record": resolved.display().to_string(),
                "cwd": cwd.as_ref().map(|p| p.display().to_string()),
                "resume": &resume,
                "trail": &title,
                "receipt": {
                    "turns": receipt.turns,
                    "tools": receipt.tools,
                    "dropped_tools": receipt.dropped_tools,
                    "dropped_reasoning": receipt.dropped_reasoning,
                    "dropped_scaffold": receipt.dropped_scaffold,
                    "redactions": receipt.redactions,
                },
            })
        );
    } else {
        println!("restored → {} session {id}  ({title})", to.label());
        println!("{}", receipt.summary());
        println!(
            "from record: {}",
            term_safe(&resolved.display().to_string())
        );
        if let Some(cwd) = cwd {
            println!("cwd: {}", term_safe(&cwd.display().to_string()));
        }
        println!("resume with: {resume}");
    }
    Ok(())
}

fn run_route(rest: &[String]) -> Result<()> {
    let mut all = false;
    let mut session: Option<PathBuf> = None;

    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--all" => {
                all = true;
                i += 1;
            }
            "--session" => {
                session = Some(PathBuf::from(flag_value(rest, i, "--session")?));
                i += 2;
            }
            other => bail!("unknown flag: {other}"),
        }
    }

    if let Some(session) = session {
        let resolved = alembic::resolve_session(&session)?;
        let (_, id) = alembic::identify(&resolved)
            .with_context(|| format!("could not identify session {}", resolved.display()))?;
        let (conv_id, _, _) = trail::resume(&resolved, &id);
        return trail::print_routes(None, Some(&conv_id));
    }

    let cwd = if all {
        None
    } else {
        std::env::current_dir().ok()
    };
    trail::print_routes(cwd.as_deref(), None)
}

fn run_status(rest: &[String]) -> Result<()> {
    let mut all = false;
    for arg in rest {
        match arg.as_str() {
            "--all" => all = true,
            other => bail!("unknown flag: {other}"),
        }
    }

    let cwd = if all {
        None
    } else {
        std::env::current_dir().ok()
    };

    println!("constant status");
    match &cwd {
        Some(cwd) => println!("project: {}", term_safe(&cwd.display().to_string())),
        None => println!("scope: all projects"),
    }

    let d = alembic::doctor();
    let mark = |b: bool| if b { "ok" } else { "missing" };
    println!("\nruntimes:");
    println!(
        "  codex  : {} (cli {}, sessions {}, db {})",
        d.codex_version.as_deref().unwrap_or("not found"),
        mark(d.codex_version.is_some()),
        mark(d.codex_store),
        mark(d.codex_db),
    );
    println!(
        "  claude : {} (cli {}, projects {})",
        d.claude_version.as_deref().unwrap_or("not found"),
        mark(d.claude_version.is_some()),
        mark(d.claude_store),
    );

    println!();
    trail::print_status(cwd.as_deref())?;

    println!("\nlatest sessions:");
    for rt in [Runtime::Codex, Runtime::Claude] {
        // Keep `status` cheap and privacy-minimal: no transcript reads here.
        // Use `constant sessions --titles` when the prompt-derived title is wanted.
        let sessions = alembic::list_sessions(rt, cwd.as_deref(), false);
        if let Some(s) = sessions.first() {
            println!("  {:<6} {}", s.runtime, term_safe(&s.id));
        } else {
            println!("  {:<6} none", rt.label());
        }
    }
    Ok(())
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

    let cwd = if all {
        None
    } else {
        std::env::current_dir().ok()
    };
    let runtimes = match from {
        Some(s) => vec![Runtime::parse(&s)?],
        None => vec![Runtime::Codex, Runtime::Claude],
    };
    let mut sessions = Vec::new();
    for rt in runtimes {
        sessions.extend(alembic::list_sessions(rt, cwd.as_deref(), titles));
    }
    sessions.sort_by_key(|b| std::cmp::Reverse(b.mtime));

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
        let scope = if all {
            ""
        } else {
            " in this directory (try --all)"
        };
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
  constant host [codex|claude] [--prefix C-t] [--with-tools]
        Host an agent CLI in a Constant PTY (default runtime: codex, prefix: Ctrl-B)

  constant carry --to codex|claude [--from codex|claude | --session PATH] [--json] [--dry-run] [--debug] [--new] [--with-tools]
        Headless: carry a conversation into the target runtime's native session and
        print the resume command (no terminal). --json for machine output;
        --dry-run previews without writing; --debug shows the route decision;
        --new creates a fresh target continuation instead of refreshing one.
        (`distill` is an alias.)

        --with-tools (experimental, also on host/resume): carry tool calls and
        results too — redacted and size-capped. Default carries conversation only.

  constant resume [QUERY] [--in codex|claude] [--list] [--all] [--prefix C-t] [--with-tools]
        Re-host a conversation from the trail: wakes its latest projection live
        (prefix switching ready). No QUERY = the newest conversation here;
        QUERY matches the slug or conversation id. If every projection is gone,
        reprints one from the latest record volume first.

  constant sessions [--from codex|claude] [--all] [--titles] [--json]
        List carryable sessions (this directory, or --all). --titles adds a preview
        (reads transcripts; slower on large stores). Discovery for carry.

  constant export (--from codex|claude | --session PATH) [--out FILE]
        Export a conversation as the neutral IR master (distilled + redacted JSON):
        a portable, runtime-agnostic copy. Writes to --out FILE, else stdout.

  constant doctor [--json]
        Preflight: which runtimes/versions are installed and whether supported.

  constant status [--all]
        Show current project, runtime readiness, latest sessions, and Constant trail.

  constant trail [--all] [--events]
        Show current projections by conversation. --events shows the raw switch ledger.

  constant snapshots [--all]
        List the record volumes (per-hop IR snapshots, written at every carry).

  constant restore SNAPSHOT [--to codex|claude] [--json]
        Reprint a fresh native session from a record volume (never overwrites
        anything). Default target: the runtime the record came from.

  constant route [--all] [--session PATH_OR_ID]
        Show the reconstructed fork graph with aliases like codex[1] and claude[1.1].

PREFIX KEY:
  Default is Ctrl-B. If you run inside tmux (which also uses Ctrl-B), pick another:
      constant host codex --prefix C-t
      CONSTANT_PREFIX=C-g constant host codex

INSIDE A HOSTED SESSION (press the prefix, then):
  c              continue in claude
  C              create a new claude continuation
  x              continue in codex
  X              create a new codex continuation
  :              open the command line (e.g. `switch claude`, `new claude`, `quit`)
  d              quit Constant (the hosted CLI exits with it)
  <prefix> again send a literal prefix key to the child
"#
    );
}
