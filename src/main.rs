//! Constant — one conversation, any agent runtime.
//!
//! `constant host [runtime]` boots an agent CLI inside a Constant-owned PTY so
//! you can switch the active runtime live (tmux-style prefix key) without losing
//! the conversation.

mod alembic;
mod explorer;
mod host;
mod live;
mod picker;
mod runtime;
mod trail;
mod tui;

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
        Some("recall") => run_recall(&args[1..]),
        Some("audit") => run_audit(&args[1..]),
        Some("ps") | Some("live") => run_ps(&args[1..]),
        Some("rename") => run_rename(&args[1..]),
        Some("pack") => run_pack(&args[1..]),
        Some("unpack") => run_unpack(&args[1..]),
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
    let mut bar = true;
    // None = no flag: long-thread switches ask [v]erbatim · [c]ompact.
    let mut render: Option<bool> = None;
    let mut yolo = false;

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
            // EXTREMELY DANGEROUS, explicit opt-in: spawn every child runtime
            // with its bypass-sandbox / skip-approvals flag, and keep it across
            // switches. Surfaced as a ⚠ yolo marker in the bar.
            "--yolo" => {
                yolo = true;
                i += 1;
            }
            "--render" => {
                render = match flag_value(rest, i, "--render")?.as_str() {
                    "paged" => Some(true),
                    "full" => Some(false),
                    other => bail!("unknown render mode: {other} (full|paged)"),
                };
                i += 2;
            }
            "--no-bar" => {
                bar = false;
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
    maybe_offer_update();
    host::run(
        Runtime::parse(&runtime_str)?,
        None,
        with_tools,
        bar,
        render,
        prefix_byte,
        prefix_label,
        yolo,
    )
}

/// The claude-code-style startup offer: when a newer release is already known
/// (cache only — never a network call here) and we're on a real terminal, ask
/// once before hosting. `y` runs the right updater for how this binary was
/// installed and relaunches the NEW constant with the same arguments; anything
/// else continues with the current one. Quiet no-op otherwise.
fn maybe_offer_update() {
    use std::io::{IsTerminal, Write};
    let current = env!("CARGO_PKG_VERSION");
    let Some(latest) = alembic::cached_release_version() else {
        return;
    };
    if !alembic::version_newer(&latest, current)
        || !std::io::stdin().is_terminal()
        || !std::io::stdout().is_terminal()
    {
        return;
    }
    print!("constant v{latest} is available (you have v{current}). update now? [y/N] ");
    let _ = std::io::stdout().flush();
    let mut answer = String::new();
    if std::io::stdin().read_line(&mut answer).is_err() {
        return;
    }
    if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        return;
    }

    let exe = std::env::current_exe().ok();
    let exe_str = exe
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let status = match install_channel(&exe_str) {
        InstallChannel::Brew => {
            println!("running: brew upgrade kennykankush/constant/constant");
            std::process::Command::new("brew")
                .args(["upgrade", "kennykankush/constant/constant"])
                .status()
        }
        InstallChannel::Cargo => {
            println!(
                "this constant was installed with cargo — update with:
  cargo install --git https://github.com/kennykankush/constant --locked"
            );
            return;
        }
        InstallChannel::Standalone => {
            println!("running the installer (downloads the latest release binary)…");
            std::process::Command::new("sh")
                .args([
                    "-c",
                    "curl -fsSL https://raw.githubusercontent.com/kennykankush/constant/main/scripts/install.sh | sh",
                ])
                .status()
        }
    };
    match status {
        Ok(st) if st.success() => {
            // Relaunch as the new binary with the same arguments (the path is
            // stable across both brew and installer upgrades).
            if let Some(exe) = exe {
                println!("updated — restarting constant");
                use std::os::unix::process::CommandExt;
                let err = std::process::Command::new(exe)
                    .args(std::env::args_os().skip(1))
                    .exec();
                eprintln!("couldn't restart automatically ({err}); continuing with v{current}");
            }
        }
        Ok(_) | Err(_) => {
            eprintln!("update didn't complete; continuing with v{current}");
        }
    }
}

#[derive(PartialEq, Debug)]
enum InstallChannel {
    Brew,
    Cargo,
    Standalone,
}

/// How this binary got onto the machine, judged from its own path.
fn install_channel(exe: &str) -> InstallChannel {
    if exe.contains("/Cellar/") || exe.contains("/homebrew/") || exe.contains("/linuxbrew/") {
        InstallChannel::Brew
    } else if exe.contains("/.cargo/") {
        InstallChannel::Cargo
    } else {
        InstallChannel::Standalone
    }
}

/// Resolve a `--session` argument the way a person types it. In order:
/// a path or native session id (exact, scriptable — unchanged), then a
/// conversation HANDLE from the trail (globally unambiguous by construction;
/// falls back to the conversation's latest record volume when no projection
/// lives), then a NAME. Names can be duplicated — claude and codex both
/// allow several sessions to share a title — so several matches open a
/// picker in a terminal, or list the candidates and bail in a script.
fn resolve_session_arg(q: &str) -> Result<PathBuf> {
    // 1. Path or native id — the exact, scriptable spelling. An AMBIGUOUS id
    // (one id in two runtimes' stores) is a refusal, not a miss: it must
    // propagate, never fall through to fuzzier matching.
    match alembic::resolve_session(Path::new(q)) {
        Ok(p) => return Ok(p),
        Err(e) if e.to_string().contains("more than one") => return Err(e),
        Err(_) => {}
    }

    // 2. Trail handle: cobalt-37 names exactly one conversation, ever.
    let convs = trail::conversations(None);
    if let Some(c) = convs.iter().find(|c| c.handle.eq_ignore_ascii_case(q)) {
        if let Some(p) = c.projections.iter().max_by_key(|p| p.last_n)
            && let Ok(rt) = Runtime::parse(&p.runtime)
            && let Some((path, _)) = alembic::session_by_id(rt, &p.id)
        {
            return Ok(path);
        }
        // Every projection gone: the record volume is itself a valid carry
        // source (carry reads IR), so a packed/expired conversation still
        // carries by handle.
        if let Some(snap) = trail::latest_snapshot(&c.conversation) {
            return Ok(snap);
        }
        bail!(
            "{} has no live projection and no record volume on this machine",
            term_safe(q)
        );
    }

    // 3. A name. Scan this folder first, widen to everywhere only when the
    // folder has no match (the near thing is almost always the meant thing).
    let here = std::env::current_dir().ok();
    let mut candidates = name_candidates(q, here.as_deref());
    if candidates.is_empty() {
        candidates = name_candidates(q, None);
    }
    match candidates.len() {
        0 => bail!(
            "could not resolve {} as a path, session id, handle, or name \
             (see `constant sessions`)",
            term_safe(q)
        ),
        1 => Ok(candidates.remove(0).path),
        _ => {
            // Several files, ONE conversation: projections of the same
            // conversation share its name by design (the codex copy, the
            // claude copy). The user means the conversation — its newest
            // projection wins. Real ambiguity is DIFFERENT conversations
            // (or unknown sessions) wearing one name.
            let handles: std::collections::HashSet<&str> =
                candidates.iter().filter_map(|c| c.handle()).collect();
            if handles.len() == 1 && candidates.iter().all(|c| c.handle().is_some()) {
                return Ok(candidates.remove(0).path); // newest-first order
            }
            use std::io::IsTerminal;
            if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
                let heading = format!("several sessions are named \u{201c}{}\u{201d}", term_safe(q));
                match picker::pick_among(&heading, candidates)? {
                    Some(choice) => Ok(choice.path),
                    None => bail!("cancelled"),
                }
            } else {
                let a = trail::ansi();
                eprintln!("several sessions match \u{201c}{}\u{201d}:", term_safe(q));
                for c in &candidates {
                    let age = trail::ago(c.mtime_secs);
                    eprintln!(
                        "  {age:>8}  {:<9} {}  {}{}{}",
                        c.runtime.label(),
                        term_safe(&c.display()),
                        a.dim,
                        term_safe(&c.id),
                        a.reset
                    );
                }
                bail!("pick one by id: constant carry --session <id> ...");
            }
        }
    }
}

/// Sessions whose display name matches `q` (case-insensitive; exact-name
/// matches outrank contains-matches), newest first, names fully resolved
/// (this is a one-shot lookup — the per-file title reads are worth it).
fn name_candidates(q: &str, cwd: Option<&Path>) -> Vec<picker::PickEntry> {
    let ql = q.to_lowercase();
    let naming = trail::naming_index();
    let mut exact = Vec::new();
    let mut partial = Vec::new();
    for rt in [
        Runtime::Codex,
        Runtime::Claude,
        Runtime::OpenCode,
        Runtime::Gemini,
    ] {
        for s in alembic::list_sessions(rt, cwd, false) {
            let mtime_secs = s
                .mtime
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let entry = picker::PickEntry {
                runtime: rt,
                trail: naming.get(&s.id).cloned(),
                lineage: None,
                runtime_title: s.title.filter(|t| !t.is_empty()),
                id: s.id,
                path: s.path,
                cwd: s.cwd,
                mtime_secs,
            };
            // Only REAL names participate — when display() falls back to the
            // raw id, "name matching" would just be a looser, more dangerous
            // version of the id resolution step 1 already does exactly.
            let name = entry.display();
            if name == entry.id {
                continue;
            }
            let name = name.to_lowercase();
            if name == ql {
                exact.push(entry);
            } else if name.contains(&ql) {
                partial.push(entry);
            }
        }
    }
    let mut out = if exact.is_empty() { partial } else { exact };
    out.sort_by_key(|e| std::cmp::Reverse(e.mtime_secs));
    out
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
    let mut paged = false;
    let mut tail_budget = alembic::render::TAIL_BUDGET_CHARS;

    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--with-tools" => {
                with_tools = true;
                i += 1;
            }
            "--render" => {
                paged = match flag_value(rest, i, "--render")?.as_str() {
                    "paged" => true,
                    "full" => false,
                    other => bail!("unknown render mode: {other} (full|paged)"),
                };
                i += 2;
            }
            "--tail" => {
                tail_budget = flag_value(rest, i, "--tail")?
                    .parse()
                    .context("--tail takes a character budget, e.g. --tail 8000")?;
                i += 2;
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
            // Resolve a path, session id, handle, or name to the real file.
            let resolved = resolve_session_arg(&p.display().to_string())?;
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
    let harvested = alembic::harvested_title(&distilled.session);
    let naming = trail::naming_for(&conv_id, &slug, harvested.as_deref());
    let n = last_n + 1;
    let title = trail::title(n, from_rt, &naming.name, &naming.handle);

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
    let mut receipt_json = serde_json::json!({
        "turns": receipt.turns,
        "tools": receipt.tools,
        "dropped_tools": receipt.dropped_tools,
        "dropped_reasoning": receipt.dropped_reasoning,
        "dropped_scaffold": receipt.dropped_scaffold,
        "redactions": receipt.redactions,
        "indexed": receipt.indexed,
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
    if paged {
        // The desk layout: the record above holds the FULL thread (the index's
        // addresses point into it); the projection gets head + index + tail.
        let anchor = alembic::render::git_anchor(
            distilled.session.metadata.cwd.as_deref().map(Path::new),
        );
        let stats = alembic::render::render_paged(
            &mut distilled.session,
            &naming.handle,
            &naming.name,
            n,
            from_rt.label(),
            to.label(),
            anchor.as_deref(),
            tail_budget,
        );
        distilled.receipt.indexed = stats.indexed;
        receipt_json["indexed"] = serde_json::json!(stats.indexed);
    }
    let (id, written, cwd) =
        alembic::distill_write(&mut distilled, &src_path, to, reuse, Some(&title))?;
    // Record the trail under the CONVERSATION's own cwd (carried from the source
    // session), not the directory `carry` happened to be invoked from — so
    // `constant trail` in that project shows its own threads. Fall back to the
    // invocation dir only if the session has no recorded cwd. A failed ledger
    // append is surfaced: pair-reuse depends on it.
    if let Err(e) = trail::record(&trail::CarryRow {
        n,
        conv_id: &conv_id,
        slug: &slug,
        cwd: cwd.as_deref().or(here.as_deref()),
        source_id: &src_id,
        source_path: &src_path,
        from: from_rt,
        to,
        id: &id,
        path: &written,
        title: &title,
        mode,
        snapshot: snapshot.as_deref(),
        handle: &naming.handle,
        name: &naming.name,
        named: naming.named,
    }) {
        eprintln!("warning: trail ledger write failed: {e}");
    }

    let resume = native_resume_cmd(to, &id);

    if json {
        let mut out = serde_json::json!({
            "id": &id,
            "from": from_rt.label(),
            "to": to.label(),
            "cwd": cwd.as_ref().map(|p| p.display().to_string()),
            "path": written.display().to_string(),
            "resume": &resume,
            "trail": &title,
            "handle": &naming.handle,
            "name": &naming.name,
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
        let a = trail::ansi();
        let color = if a.tty { trail::runtime_paint(to.label()) } else { "" };
        println!(
            "carried \u{2192} {color}{}{} session {id}  {}({title}){}",
            to.label(),
            a.reset,
            a.dim,
            a.reset
        );
        println!("{}", distilled.receipt.summary());
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
    let mut full = false;
    let mut plain = false;
    for arg in rest {
        match arg.as_str() {
            "--all" => all = true,
            "--events" => events = true,
            "--full" => full = true,
            "--plain" => plain = true,
            other => bail!("unknown flag: {other}"),
        }
    }
    let cwd = if all {
        None
    } else {
        std::env::current_dir().ok()
    };
    if events {
        return trail::print_events(cwd.as_deref());
    }
    if full {
        return trail::print_full(cwd.as_deref());
    }
    // In a terminal the trail is a place, not a printout: the explorer zooms
    // conversations → chapters → filed turns. Piped output (and --plain)
    // keeps the card view for scripts and quick glances.
    if !plain && tui::interactive() {
        return match explorer::explore(cwd)? {
            // Handles are globally unambiguous, and the explorer's everywhere
            // scope can pick a conversation from any folder — resume unscoped.
            Some(handle) => run_resume_cmd(&[handle, "--all".to_string()]),
            None => Ok(()),
        };
    }
    trail::print(cwd.as_deref())
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
    let mut bar = true;
    let mut render: Option<bool> = None;
    let mut yolo = false;
    let mut prefix_str = std::env::var("CONSTANT_PREFIX").unwrap_or_else(|_| "C-b".to_string());

    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--with-tools" => {
                with_tools = true;
                i += 1;
            }
            // EXTREMELY DANGEROUS, explicit opt-in — see run_host's --yolo.
            "--yolo" => {
                yolo = true;
                i += 1;
            }
            "--render" => {
                render = match flag_value(rest, i, "--render")?.as_str() {
                    "paged" => Some(true),
                    "full" => Some(false),
                    other => bail!("unknown render mode: {other} (full|paged)"),
                };
                i += 2;
            }
            "--no-bar" => {
                bar = false;
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
    // No query, interactive terminal → the picker: every session in scope,
    // type-to-search, Enter wakes it hosted. (Non-TTY and explicit queries
    // keep the scriptable paths below.)
    if query.is_none()
        && !list
        && std::io::stdin().is_terminal()
        && std::io::stdout().is_terminal()
    {
        let Some(choice) = picker::pick(cwd.clone())? else {
            return Ok(());
        };
        let (prefix_byte, prefix_label) = host::parse_prefix(&prefix_str)?;
        maybe_offer_update();
        return host::run(
            choice.runtime,
            Some(&choice.id),
            with_tools,
            bar,
            render,
            prefix_byte,
            prefix_label,
            yolo,
        );
    }

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
            // Exact handle match is unambiguous by construction — short-circuit.
            if let Some(exact) = convs.iter().find(|c| c.handle == ql) {
                vec![exact]
            } else {
                convs
                    .iter()
                    .filter(|c| {
                        c.slug.to_lowercase().contains(&ql)
                            || c.name.to_lowercase().contains(&ql)
                            || c.handle.starts_with(ql.as_str())
                            || c.conversation.starts_with(q.as_str())
                    })
                    .collect()
            }
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
                println!("  {}  {}", term_safe(&c.handle), term_safe(&c.name));
            }
            bail!("narrow the query (handles are always unambiguous)");
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
        println!(
            "conversation: {}  {}",
            term_safe(&conv.handle),
            term_safe(&conv.name)
        );
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
    maybe_offer_update();
    host::run(rt, Some(&id), with_tools, bar, render, prefix_byte, prefix_label, yolo)
}

fn print_resume_list(convs: &[trail::ConversationView]) {
    let a = trail::ansi();
    let (dim, bold, reset) = (a.dim, a.bold, a.reset);
    println!("{dim}resumable conversations (newest first){reset}");
    let handle_w = convs
        .iter()
        .map(|c| c.handle.chars().count())
        .max()
        .unwrap_or(8)
        .max(8);
    for c in convs {
        let lives_in = if c.projections.is_empty() {
            format!("{dim}record only{reset}")
        } else {
            c.projections
                .iter()
                .map(|p| {
                    let color = if a.tty { trail::runtime_paint(&p.runtime) } else { "" };
                    format!("{color}{}{reset}", term_safe(&p.runtime))
                })
                .collect::<Vec<_>>()
                .join(&format!("{dim}\u{b7}{reset}"))
        };
        println!(
            "  {bold}{:<42}{reset} {dim}{:<handle_w$}{reset} {}  {dim}{}{reset}",
            trail::clip(&term_safe(&c.name), 40),
            term_safe(&c.handle),
            lives_in,
            trail::ago(c.last_ts)
        );
        println!("  {dim}{:<handle_w$} \u{21b3} constant resume {}{reset}", "", term_safe(&c.handle));
    }
}

/// `constant ps` — every agent CLI process alive on this machine right now:
/// runtime, uptime, the conversation it holds (when the ledger knows it), and
/// how to get back in. Read-only.
/// `constant rename [--of HANDLE] NEW NAME…` — explicitly name a conversation.
/// An explicit rename locks the title (auto-naming stops). Native pickers are
/// re-stamped for claude/codex projections; opencode picks it up at the next
/// carry.
fn run_rename(rest: &[String]) -> Result<()> {
    let mut of: Option<String> = None;
    let mut words: Vec<String> = Vec::new();
    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--of" => {
                of = Some(flag_value(rest, i, "--of")?);
                i += 2;
            }
            w => {
                words.push(w.to_string());
                i += 1;
            }
        }
    }
    let new_name = words.join(" ");
    if new_name.trim().is_empty() {
        bail!("rename needs the new name: constant rename [--of HANDLE] my new name");
    }

    let here = std::env::current_dir().ok();
    // Target: explicit handle, else the newest conversation here.
    let all = trail::conversations(None);
    let conv = match &of {
        Some(h) => all
            .iter()
            .find(|c| c.handle == *h)
            .with_context(|| format!("no conversation with handle {h}"))?,
        None => {
            let scoped = trail::conversations(here.as_deref());
            let Some(first) = scoped.first() else {
                bail!("no conversation here to rename (try --of HANDLE; see `constant trail --all`)");
            };
            all.iter()
                .find(|c| c.conversation == first.conversation)
                .with_context(|| format!("conversation {} missing from the ledger", first.handle))?
        }
    };

    trail::record_rename(&conv.conversation, &conv.handle, &new_name, here.as_deref())?;

    // Re-stamp the conversation's current projections in native pickers.
    let stamp = format!("{new_name} \u{b7} {}", conv.handle);
    for p in &conv.projections {
        if let Ok(rt) = Runtime::parse(&p.runtime)
            && let Some((path, _)) = alembic::session_by_id(rt, &p.id)
        {
            let _ = alembic::restamp_title(rt, &p.id, &path, &stamp);
        }
    }

    println!(
        "renamed: {}  \u{201c}{}\u{201d}",
        term_safe(&conv.handle),
        term_safe(&new_name)
    );
    println!("(the name is now locked to your words; auto-naming stops)");
    Ok(())
}

/// `constant pack HANDLE [--out FILE]` — bundle a conversation (its ledger
/// rows + every record volume) into one portable file. The conversation
/// crosses machines: `constant unpack` on the other side, then
/// `constant resume <handle>` reprints it from the record.
fn run_pack(rest: &[String]) -> Result<()> {
    let mut query: Option<String> = None;
    let mut out_path: Option<PathBuf> = None;
    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--out" => {
                out_path = Some(PathBuf::from(flag_value(rest, i, "--out")?));
                i += 2;
            }
            s if !s.starts_with("--") => {
                if query.is_some() {
                    bail!("pack takes one conversation handle");
                }
                query = Some(s.to_string());
                i += 1;
            }
            other => bail!("unknown flag: {other}"),
        }
    }
    let q = query.context("pack needs a conversation handle (see `constant trail --all`)")?;

    let convs = trail::conversations(None);
    let conv = convs
        .iter()
        .find(|c| c.handle == q)
        .or_else(|| convs.iter().find(|c| c.conversation == q))
        .with_context(|| format!("no conversation with handle {q} (see `constant trail --all`)"))?;

    let rows = trail::raw_rows(&conv.conversation);
    if rows.is_empty() {
        bail!("nothing recorded for {q} yet — packs carry the ledger + record volumes");
    }

    // Volumes: every record file the rows reference and that still exists.
    let mut volumes = serde_json::Map::new();
    for line in &rows {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let Some(snap) = v.get("snapshot").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let path = Path::new(snap);
        let Some(fname) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if volumes.contains_key(fname) || !path.exists() {
            continue;
        }
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read record volume {}", path.display()))?;
        let vol: serde_json::Value = serde_json::from_str(&text)
            .with_context(|| format!("record volume {} is not valid JSON", path.display()))?;
        volumes.insert(fname.to_string(), vol);
    }

    let doc = serde_json::json!({
        "constant_pack": 1,
        "packed_at": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        "conversation": conv.conversation,
        "handle": conv.handle,
        "name": conv.name,
        "rows": rows,
        "volumes": volumes,
    });

    let out_path =
        out_path.unwrap_or_else(|| PathBuf::from(format!("{}.constant-pack.json", conv.handle)));
    if out_path.exists() {
        bail!(
            "refusing to overwrite {} — pick another --out",
            term_safe(&out_path.display().to_string())
        );
    }
    std::fs::write(&out_path, serde_json::to_string_pretty(&doc)?)
        .with_context(|| format!("failed to write {}", out_path.display()))?;

    let n_vols = doc["volumes"].as_object().map(|m| m.len()).unwrap_or(0);
    println!(
        "packed {} \u{b7} {}  ({} ledger rows, {} record volumes)",
        term_safe(&conv.handle),
        term_safe(&conv.name),
        doc["rows"].as_array().map(|a| a.len()).unwrap_or(0),
        n_vols,
    );
    println!("\u{2192} {}", term_safe(&out_path.display().to_string()));
    println!("on the other machine: constant unpack {}", term_safe(&out_path.display().to_string()));
    println!("                      constant resume {}", term_safe(&conv.handle));
    Ok(())
}

/// `constant unpack FILE` — import a packed conversation: volumes land in the
/// local vault (never overwriting existing ones — volumes are immutable),
/// ledger rows append idempotently with snapshot paths rewritten, and the
/// handle re-mints if the local registry already gave it to someone else.
fn run_unpack(rest: &[String]) -> Result<()> {
    let file = rest
        .iter()
        .find(|a| !a.starts_with("--"))
        .context("unpack needs a pack file: constant unpack <file>")?;
    let text = std::fs::read_to_string(file)
        .with_context(|| format!("failed to read {file}"))?;
    let doc: serde_json::Value =
        serde_json::from_str(&text).with_context(|| format!("{file} is not a pack file"))?;
    if doc.get("constant_pack").and_then(serde_json::Value::as_u64) != Some(1) {
        bail!("{file} is not a constant pack (or a newer pack version this build can't read)");
    }
    let conv_id = doc
        .get("conversation")
        .and_then(serde_json::Value::as_str)
        .context("pack missing conversation id")?;

    // Volumes first (record before ledger — same law as carries).
    let vault = trail::vault_dir(conv_id).context("HOME is not set")?;
    let mut volume_paths: std::collections::HashMap<String, PathBuf> =
        std::collections::HashMap::new();
    let mut vols_written = 0usize;
    let mut vols_kept = 0usize;
    if let Some(vols) = doc.get("volumes").and_then(serde_json::Value::as_object) {
        for (fname, vol) in vols {
            if fname.contains('/') || fname.contains("..") {
                bail!("pack volume has a hostile filename: {}", term_safe(fname));
            }
            // A volume must BE a neutral IR session — validated by parsing.
            let session: alembic::ir::UniversalSession =
                serde_json::from_value(vol.clone())
                    .with_context(|| format!("pack volume {fname} is not valid IR"))?;
            let target = vault.join(fname);
            if target.exists() {
                vols_kept += 1; // volumes are immutable: the local copy stands
            } else {
                alembic::write_snapshot(&session, &target)?;
                vols_written += 1;
            }
            volume_paths.insert(fname.clone(), target);
        }
    }

    let rows: Vec<String> = doc
        .get("rows")
        .and_then(serde_json::Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let summary = trail::import_rows(conv_id, &rows, &volume_paths)?;

    println!(
        "unpacked {} \u{b7} {}",
        term_safe(&summary.handle),
        term_safe(doc.get("name").and_then(serde_json::Value::as_str).unwrap_or("conversation")),
    );
    println!(
        "  ledger: {} rows added, {} already present \u{b7} volumes: {} written, {} kept",
        summary.rows_added, summary.rows_skipped, vols_written, vols_kept
    );
    if summary.rehandled {
        println!(
            "  note: the packed handle was taken here \u{2192} re-minted as {}",
            term_safe(&summary.handle)
        );
    }
    println!("wake it: constant resume {}", term_safe(&summary.handle));
    Ok(())
}

fn run_ps(rest: &[String]) -> Result<()> {
    let mut json = false;
    let mut deep = false;
    for arg in rest {
        match arg.as_str() {
            "--json" => json = true,
            "--deep" => deep = true,
            other => bail!("unknown flag: {other}"),
        }
    }

    let agents = live::census();

    // --deep joins each live process to the trail: which chapter of the thread
    // it holds, and whether two live agents are unknowingly on the SAME
    // conversation (a silent double-booking). Indexes built once, only on --deep.
    let naming = deep.then(trail::naming_index).unwrap_or_default();
    let lineage = deep.then(trail::lineage_index).unwrap_or_default();
    let mut live_by_handle: std::collections::HashMap<String, Vec<i32>> =
        std::collections::HashMap::new();
    if deep {
        for a in &agents {
            if let Some((_, handle, _)) = a.session_id.as_deref().and_then(|id| naming.get(id)) {
                live_by_handle.entry(handle.clone()).or_default().push(a.pid);
            }
        }
    }
    let chapter_of = |id: Option<&str>| -> Option<String> { id.and_then(|id| lineage.get(id)).cloned() };
    let shared_with = |id: Option<&str>, pid: i32| -> Vec<i32> {
        id.and_then(|id| naming.get(id))
            .and_then(|(_, h, _)| live_by_handle.get(h))
            .map(|pids| pids.iter().copied().filter(|p| *p != pid).collect())
            .unwrap_or_default()
    };

    if json {
        let arr: Vec<_> = agents
            .iter()
            .map(|a| {
                let conversation = a.session_id.as_deref().and_then(trail::label_for_session);
                let mut obj = serde_json::json!({
                    "runtime": a.runtime.label(),
                    "pid": a.pid,
                    "up": a.up,
                    "session": a.session_id,
                    "conversation": conversation,
                    "cwd": a.cwd,
                    "resume": a.session_id.as_deref().map(|id| native_resume_cmd(a.runtime, id)),
                });
                if deep {
                    obj["chapter"] = serde_json::json!(chapter_of(a.session_id.as_deref()));
                    obj["shared_with"] =
                        serde_json::json!(shared_with(a.session_id.as_deref(), a.pid));
                }
                obj
            })
            .collect();
        println!("{}", serde_json::Value::Array(arr));
        return Ok(());
    }

    if agents.is_empty() {
        println!("no live agent sessions");
        return Ok(());
    }

    let a_style = trail::ansi();
    let (dim, bold, reset) = (a_style.dim, a_style.bold, a_style.reset);
    println!("{dim}{} live agent session{}{reset}", agents.len(), if agents.len() == 1 { "" } else { "s" });
    println!();
    let home = std::env::var("HOME").unwrap_or_default();
    for a in &agents {
        let color = if a_style.tty {
            trail::runtime_paint(a.runtime.label())
        } else {
            ""
        };
        // Pad BEFORE styling: escape codes inside a width spec break columns.
        let raw_label = a
            .session_id
            .as_deref()
            .and_then(trail::naming_parts_for_session)
            .map(|(n, h, _)| term_safe(&trail::clip(&format!("{n} \u{b7} {h}"), 44)));
        let conversation = match &raw_label {
            Some(l) => format!("{bold}{:<44}{reset}", l),
            None => format!("{dim}{:<44}{reset}", "\u{2014}"),
        };
        let session = a
            .session_id
            .as_deref()
            .map(|id| {
                let head: String = id.chars().take(8).collect();
                format!("{head}\u{2026}")
            })
            .unwrap_or_else(|| "fresh".to_string());
        let cwd = a
            .cwd
            .as_deref()
            .map(|c| {
                if !home.is_empty() && c.starts_with(&home) {
                    format!("~{}", &c[home.len()..])
                } else {
                    c.to_string()
                }
            })
            .unwrap_or_default();
        let mut extra = String::new();
        if deep {
            if let Some(ch) = chapter_of(a.session_id.as_deref()) {
                extra.push_str(&format!("  {dim}{}{reset}", term_safe(&ch)));
            }
            let shared = shared_with(a.session_id.as_deref(), a.pid);
            if !shared.is_empty() {
                let pids = shared.iter().map(i32::to_string).collect::<Vec<_>>().join(",");
                extra.push_str(&format!("  {bold}\u{26a0} shared w/ pid {pids}{reset}"));
            }
        }
        println!(
            "  {color}{:<9}{reset} {dim}{:>12}{reset}  {dim}{:<10}{reset} {conversation} {dim}{}{reset}{extra}",
            a.runtime.label(),
            term_safe(&a.up),
            term_safe(&session),
            term_safe(&cwd),
        );
    }
    println!();
    if deep {
        let booked = live_by_handle.values().filter(|p| p.len() > 1).count();
        if booked > 0 {
            println!(
                "{dim}\u{26a0} {booked} conversation{} held by more than one live agent \u{2014} \
                 they ping-pong the same projections silently{reset}",
                if booked == 1 { "" } else { "s" }
            );
            println!();
        }
    }
    println!(
        "resume any of them: constant resume <conversation>, or the native command via `constant ps --json`"
    );
    Ok(())
}

/// `constant recall HANDLE [chNN] [TURN | A-B]` — read filed turns back from
/// the record, verbatim. The pager's other half: rendered projections file
/// older turns under addresses like `ch04·12`; this command resolves them.
/// Read-only — the record is never written, only opened.
fn run_recall(rest: &[String]) -> Result<()> {
    const MAX_TURNS: usize = 40;
    const MAX_CHARS: usize = 20_000;

    let mut query: Option<String> = None;
    let mut chapter: Option<u32> = None;
    let mut range: Option<(usize, usize)> = None;
    for arg in rest {
        let a = arg.as_str();
        if let Some(n) = a.strip_prefix("ch").and_then(|x| x.parse::<u32>().ok()) {
            chapter = Some(n);
        } else if let Some((lo, hi)) = parse_turn_range(a) {
            range = Some((lo, hi));
        } else if a.starts_with("--") {
            bail!("unknown flag: {a}");
        } else if query.is_none() {
            query = Some(a.to_string());
        } else {
            bail!("recall takes one conversation, e.g. `constant recall cobalt-37 ch04 12-18`");
        }
    }
    let q = query.context("recall needs a conversation handle (see `constant trail --all`)")?;

    // Resolve the conversation: handle exact first (never ambiguous), then
    // name/slug contains, then conversation-id prefix.
    let convs = trail::conversations(None);
    let ql = q.to_lowercase();
    let conv = convs
        .iter()
        .find(|c| c.handle.to_lowercase() == ql)
        .or_else(|| {
            let hits: Vec<_> = convs
                .iter()
                .filter(|c| {
                    c.name.to_lowercase().contains(&ql)
                        || c.slug.to_lowercase().contains(&ql)
                        || c.conversation.starts_with(&q)
                })
                .collect();
            match hits.len() {
                1 => Some(hits[0]),
                0 => None,
                _ => {
                    eprintln!("\"{q}\" matches several conversations — use the handle:");
                    for c in &hits {
                        eprintln!("  {}  {}", c.handle, c.name);
                    }
                    None
                }
            }
        })
        .with_context(|| format!("no conversation matches \"{q}\""))?;

    // Resolve the volume: a named chapter's, else the newest on disk.
    let volume = match chapter {
        Some(n) => {
            let row = trail::chapters(&conv.conversation)
                .into_iter()
                .find(|c| c.n == n)
                .with_context(|| format!("{} has no chapter {n}", conv.handle))?;
            let path = trail::snapshot_path(&conv.conversation, n, Runtime::parse(&row.from)?)
                .context("record vault unavailable")?;
            if !path.exists() {
                bail!(
                    "chapter {n}'s record volume is missing on this machine \
                     (see `constant snapshots`)"
                );
            }
            path
        }
        None => trail::latest_snapshot(&conv.conversation).with_context(|| {
            format!("{} has no record volumes on this machine", conv.handle)
        })?,
    };

    let text = std::fs::read_to_string(&volume)
        .with_context(|| format!("could not read {}", volume.display()))?;
    let session: alembic::ir::UniversalSession =
        serde_json::from_str(&text).context("record volume is not a valid IR volume")?;
    let turns = alembic::render::message_turns(&session);
    if turns.is_empty() {
        bail!("that volume holds no conversational turns");
    }

    let (lo, hi) = range.unwrap_or((1, turns.len()));
    let lo = lo.max(1);
    let hi = hi.min(turns.len());
    if lo > hi {
        bail!("turn range {lo}-{hi} is out of bounds (1-{})", turns.len());
    }

    let ch = chapter
        .or_else(|| {
            trail::chapters(&conv.conversation)
                .last()
                .map(|c| c.n)
        })
        .unwrap_or(0);
    let a = trail::ansi();
    println!(
        "{}{}{} {}({}) \u{b7} ch{ch:02} \u{b7} turns {lo}-{hi} of {}{}",
        a.bold,
        trail::clip(&term_safe(&conv.name), 56),
        a.reset,
        a.dim,
        term_safe(&conv.handle),
        turns.len(),
        a.reset
    );

    let mut chars = 0usize;
    let mut last = lo;
    for (shown, (n, role, text)) in turns.iter().skip(lo - 1).take(hi - lo + 1).enumerate() {
        if shown >= MAX_TURNS || chars >= MAX_CHARS {
            println!(
                "\u{2026} output capped \u{b7} next: constant recall {} ch{ch:02} {}-{hi}",
                conv.handle, last
            );
            break;
        }
        let safe = term_safe(text);
        println!("\n[ch{ch:02}\u{b7}{n}] {role}:\n{safe}");
        chars += safe.chars().count();
        last = n + 1;
    }
    Ok(())
}

/// `"14"` → (14,14) · `"12-18"` → (12,18) · anything else → None.
fn parse_turn_range(s: &str) -> Option<(usize, usize)> {
    if let Ok(n) = s.parse::<usize>() {
        return Some((n, n));
    }
    let (a, b) = s.split_once('-')?;
    Some((a.parse().ok()?, b.parse().ok()?))
}

/// `constant audit [HANDLE] [--all] [--json] [--tail CHARS]` — turn on the
/// renderer's instruments. For each recorded chapter it reads the record volume
/// and reports, READ-ONLY, what a paged render would keep on the desk (the
/// verbatim tail) vs file in the cabinet (the recallable index) — the
/// measurement substrate the continuation-fidelity work needs, on real
/// conversations, today. Zero model calls, zero writes.
fn run_audit(rest: &[String]) -> Result<()> {
    let mut query: Option<String> = None;
    let mut all = false;
    let mut json = false;
    let mut tail_budget = alembic::render::TAIL_BUDGET_CHARS;

    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--all" => {
                all = true;
                i += 1;
            }
            "--json" => {
                json = true;
                i += 1;
            }
            "--tail" => {
                tail_budget = flag_value(rest, i, "--tail")?
                    .parse()
                    .context("--tail wants a character count, e.g. --tail 24000")?;
                i += 2;
            }
            a if a.starts_with("--") => bail!("unknown flag: {a}"),
            a if query.is_none() => {
                query = Some(a.to_string());
                i += 1;
            }
            _ => bail!("audit takes one conversation, e.g. `constant audit iris-19`"),
        }
    }

    // Pick the conversations: one when a handle/name is given, else cwd-scoped.
    let scope = if all || query.is_some() {
        None
    } else {
        std::env::current_dir().ok()
    };
    let mut convs = trail::conversations(scope.as_deref());
    if let Some(q) = &query {
        let ql = q.to_lowercase();
        let chosen = convs
            .iter()
            .find(|c| c.handle.to_lowercase() == ql)
            .cloned()
            .or_else(|| {
                let hits: Vec<_> = convs
                    .iter()
                    .filter(|c| {
                        c.name.to_lowercase().contains(&ql)
                            || c.slug.to_lowercase().contains(&ql)
                            || c.conversation.starts_with(q.as_str())
                    })
                    .cloned()
                    .collect();
                (hits.len() == 1).then(|| hits[0].clone())
            })
            .with_context(|| {
                format!("no conversation matches \"{q}\" (see `constant trail --all`)")
            })?;
        convs = vec![chosen];
    }

    enum Vol {
        Ok {
            turns: usize,
            verbatim: usize,
            filed: usize,
            tail_chars: usize,
        },
        Missing,
        Corrupt,
    }
    struct ChapterAudit {
        n: u32,
        from: String,
        to: String,
        vol: Vol,
    }
    struct ConvAudit<'a> {
        conv: &'a trail::ConversationView,
        chapters: Vec<ChapterAudit>,
    }

    let mut audited: Vec<ConvAudit> = Vec::new();
    for conv in &convs {
        let mut chapters = Vec::new();
        for row in trail::chapters(&conv.conversation) {
            // Open the volume the LEDGER recorded — never reconstruct the path
            // (that would miss restored/imported/legacy volumes AND, via the
            // write-side `snapshot_path` helper, create vault dirs from a
            // read-only command). A hop with no record is skipped; a recorded
            // volume that is gone or corrupt is SURFACED, never hidden — the
            // record integrity is exactly what an audit must declare (no silent
            // loss).
            let Some(snapshot) = row.snapshot.as_deref() else {
                continue;
            };
            let vol = match std::fs::read_to_string(snapshot) {
                Err(_) => Vol::Missing,
                Ok(text) => {
                    match serde_json::from_str::<alembic::ir::UniversalSession>(&text) {
                        Err(_) => Vol::Corrupt,
                        Ok(session) => {
                            let turns = alembic::render::message_turns(&session);
                            let stats = alembic::render::render_stats(&session, tail_budget);
                            let tail_start = turns.len().saturating_sub(stats.verbatim);
                            let tail_chars: usize = turns[tail_start..]
                                .iter()
                                .map(|(_, _, t)| t.chars().count())
                                .sum();
                            Vol::Ok {
                                turns: turns.len(),
                                verbatim: stats.verbatim,
                                filed: stats.indexed,
                                tail_chars,
                            }
                        }
                    }
                }
            };
            chapters.push(ChapterAudit {
                n: row.n,
                from: row.from.clone(),
                to: row.to.clone(),
                vol,
            });
        }
        if !chapters.is_empty() {
            audited.push(ConvAudit { conv, chapters });
        }
    }

    if json {
        let arr: Vec<_> = audited
            .iter()
            .map(|ca| {
                serde_json::json!({
                    "handle": ca.conv.handle,
                    "name": ca.conv.name,
                    "conversation": ca.conv.conversation,
                    "tail_budget": tail_budget,
                    "chapters": ca.chapters.iter().map(|c| {
                        let mut obj = serde_json::json!({
                            "n": c.n,
                            "from": c.from,
                            "to": c.to,
                        });
                        match &c.vol {
                            Vol::Ok { turns, verbatim, filed, tail_chars } => {
                                obj["status"] = serde_json::json!("ok");
                                obj["turns"] = serde_json::json!(turns);
                                obj["verbatim"] = serde_json::json!(verbatim);
                                obj["filed"] = serde_json::json!(filed);
                                obj["tail_chars"] = serde_json::json!(tail_chars);
                            }
                            Vol::Missing => obj["status"] = serde_json::json!("record volume missing"),
                            Vol::Corrupt => obj["status"] = serde_json::json!("record volume unreadable"),
                        }
                        obj
                    }).collect::<Vec<_>>(),
                })
            })
            .collect();
        println!("{}", serde_json::Value::Array(arr));
        return Ok(());
    }

    if audited.is_empty() {
        let scope_note = query
            .as_deref()
            .map(|q| format!(" for \"{q}\""))
            .unwrap_or_default();
        println!(
            "no recorded chapters to audit{scope_note} \
             (records are written at every carry; try `constant audit --all`)"
        );
        return Ok(());
    }

    let a = trail::ansi();
    for ca in &audited {
        println!(
            "{}{}{} {}({}){} \u{b7} tail budget {}k chars",
            a.bold,
            trail::clip(&term_safe(&ca.conv.name), 56),
            a.reset,
            a.dim,
            term_safe(&ca.conv.handle),
            a.reset,
            tail_budget / 1000,
        );
        for c in &ca.chapters {
            let (dim, bold, reset) = (a.dim, a.bold, a.reset);
            let (from, to) = (term_safe(&c.from), term_safe(&c.to));
            match &c.vol {
                Vol::Ok { turns, verbatim, filed, tail_chars } => println!(
                    "  {dim}ch{n:02}{reset}  {from}\u{2192}{to}   {turns} turns \u{b7} \
                     keep {verbatim} verbatim \u{b7} file {filed} \u{b7} ~{tail}k tail",
                    n = c.n,
                    tail = tail_chars.div_ceil(1000),
                ),
                Vol::Missing => println!(
                    "  {dim}ch{n:02}{reset}  {from}\u{2192}{to}   {bold}\u{26a0} record volume missing{reset}",
                    n = c.n,
                ),
                Vol::Corrupt => println!(
                    "  {dim}ch{n:02}{reset}  {from}\u{2192}{to}   {bold}\u{26a0} record volume unreadable{reset}",
                    n = c.n,
                ),
            }
        }
        println!();
    }
    println!(
        "{}filed turns are never lost \u{2014} read any back with \
         `constant recall <handle> chNN <turn>`{}",
        a.dim, a.reset
    );
    Ok(())
}

fn run_snapshots(rest: &[String]) -> Result<()> {
    let mut all = false;
    let mut full = false;
    for arg in rest {
        match arg.as_str() {
            "--all" => all = true,
            "--full" => full = true,
            other => bail!("unknown flag: {other}"),
        }
    }
    let cwd = if all {
        None
    } else {
        std::env::current_dir().ok()
    };
    trail::print_snapshots(cwd.as_deref(), full)
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
    let harvested = alembic::harvested_title(&distilled.session);
    let naming = trail::naming_for(&conv_id, &slug, harvested.as_deref());
    let n = last_n + 1;
    let title = trail::title(n, src_rt, &naming.name, &naming.handle);

    let (id, written, cwd) =
        alembic::distill_write(&mut distilled, &resolved, to, None, Some(&title))?;
    let here = std::env::current_dir().ok();
    if let Err(e) = trail::record(&trail::CarryRow {
        n,
        conv_id: &conv_id,
        slug: &slug,
        cwd: cwd.as_deref().or(here.as_deref()),
        source_id: &src_id,
        source_path: &resolved,
        from: src_rt,
        to,
        id: &id,
        path: &written,
        title: &title,
        mode: "restore",
        snapshot: Some(&resolved),
        handle: &naming.handle,
        name: &naming.name,
        named: naming.named,
    }) {
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
        Runtime::Gemini => format!("gemini --resume {id}"),
        Runtime::OpenCode => format!("opencode -s {id}"),
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
    let mut json = false;
    let mut session: Option<PathBuf> = None;

    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--all" => {
                all = true;
                i += 1;
            }
            "--json" => {
                json = true;
                i += 1;
            }
            "--session" => {
                session = Some(PathBuf::from(flag_value(rest, i, "--session")?));
                i += 2;
            }
            other => bail!("unknown flag: {other}"),
        }
    }

    // Resolve a single-conversation filter when --session is given.
    let conv_id = match &session {
        Some(session) => {
            let resolved = alembic::resolve_session(session)?;
            let (_, id) = alembic::identify(&resolved)
                .with_context(|| format!("could not identify session {}", resolved.display()))?;
            Some(trail::resume(&resolved, &id).0)
        }
        None => None,
    };

    // --json: emit the fork-DAG the trail already reconstructs, so external
    // tools read lineage without screen-scraping the debug view.
    if json {
        let cwd = if all || conv_id.is_some() {
            None
        } else {
            std::env::current_dir().ok()
        };
        let mut views = trail::route_views(cwd.as_deref());
        if let Some(cid) = &conv_id {
            views.retain(|v| &v.conversation == cid);
        }
        let arr: Vec<_> = views
            .iter()
            .map(|v| {
                let nodes: Vec<_> = v
                    .nodes
                    .iter()
                    .map(|n| {
                        let resume = Runtime::parse(&n.runtime)
                            .ok()
                            .map(|rt| native_resume_cmd(rt, &n.id));
                        serde_json::json!({
                            "alias": n.alias,
                            "runtime": n.runtime,
                            "id": n.id,
                            "path": n.path,
                            "title": n.title,
                            "parent": n.parent_alias,
                            "mode": n.mode,
                            "last_from": n.last_from,
                            "last_n": n.last_n,
                            "refreshes": n.refreshes,
                            "active": n.active,
                            "resume": resume,
                        })
                    })
                    .collect();
                serde_json::json!({
                    "conversation": v.conversation,
                    "slug": v.slug,
                    "cwd": v.cwd,
                    "root": v.root_alias,
                    "root_runtime": v.root_runtime,
                    "entries": v.entries,
                    "last_ts": v.last_ts,
                    "nodes": nodes,
                })
            })
            .collect();
        println!("{}", serde_json::Value::Array(arr));
        return Ok(());
    }

    if let Some(cid) = conv_id {
        return trail::print_routes(None, Some(&cid));
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
        "  codex    : {} (cli {}, sessions {}, db {})",
        d.codex_version.as_deref().unwrap_or("not found"),
        mark(d.codex_version.is_some()),
        mark(d.codex_store),
        mark(d.codex_db),
    );
    println!(
        "  claude   : {} (cli {}, projects {})",
        d.claude_version.as_deref().unwrap_or("not found"),
        mark(d.claude_version.is_some()),
        mark(d.claude_store),
    );
    println!(
        "  opencode : {} (cli {}, db {})",
        d.opencode_version.as_deref().unwrap_or("not found"),
        mark(d.opencode_version.is_some()),
        mark(d.opencode_db),
    );
    println!(
        "  gemini   : {} (cli {}, store {}) — carry source only for now",
        d.gemini_version.as_deref().unwrap_or("not found"),
        mark(d.gemini_version.is_some()),
        mark(d.gemini_store),
    );

    println!();
    trail::print_status(cwd.as_deref())?;

    let a = trail::ansi();
    let (dim, bold, reset) = (a.dim, a.bold, a.reset);
    println!("\nlatest sessions:");
    for rt in [
        Runtime::Codex,
        Runtime::Claude,
        Runtime::OpenCode,
        Runtime::Gemini,
    ] {
        // Keep `status` cheap and privacy-minimal: no transcript reads here.
        // Use `constant sessions --titles` when the prompt-derived title is wanted.
        let sessions = alembic::list_sessions(rt, cwd.as_deref(), false);
        let color = if a.tty { trail::runtime_paint(rt.label()) } else { "" };
        if let Some(s) = sessions.first() {
            let age = s
                .mtime
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| trail::ago(d.as_secs()))
                .unwrap_or_default();
            let known = trail::naming_parts_for_session(&s.id)
                .map(|(n, h, _)| {
                    format!(
                        "  {bold}{}{reset} {dim}\u{b7} {}{reset}",
                        term_safe(&trail::clip(&n, 44)),
                        term_safe(&h)
                    )
                })
                .unwrap_or_default();
            println!(
                "  {color}{:<8}{reset} {dim}{}{reset}  {dim}{age}{reset}{known}",
                s.runtime,
                term_safe(&s.id)
            );
        } else {
            println!("  {color}{:<8}{reset} {dim}none{reset}", rt.label());
        }
    }
    Ok(())
}

fn run_sessions(rest: &[String]) -> Result<()> {
    let mut from: Option<String> = None;
    let mut all = false;
    let mut json = false;
    let mut titles = false;
    let mut plain = false;
    let mut query: Option<String> = None;
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
            "--plain" => {
                plain = true;
                i += 1;
            }
            q if !q.starts_with('-') => {
                if query.is_some() {
                    bail!("sessions takes one search query, e.g. `constant sessions market`");
                }
                query = Some(q.to_lowercase());
                i += 1;
            }
            other => bail!("unknown flag: {other}"),
        }
    }

    // Bare `constant sessions` in a terminal opens the same picker as
    // `constant resume`: the sessions ARE its rows, Enter wakes one hosted.
    // Any query/flag (or a pipe) keeps the scriptable printout below.
    if !plain && !json && !titles && from.is_none() && query.is_none() && tui::interactive() {
        let fwd: Vec<String> = if all { vec!["--all".to_string()] } else { Vec::new() };
        return run_resume_cmd(&fwd);
    }

    let cwd = if all {
        None
    } else {
        std::env::current_dir().ok()
    };
    let runtimes = match from {
        Some(s) => vec![Runtime::parse(&s)?],
        None => vec![
            Runtime::Codex,
            Runtime::Claude,
            Runtime::OpenCode,
            Runtime::Gemini,
        ],
    };
    let mut sessions = Vec::new();
    for rt in runtimes {
        sessions.extend(alembic::list_sessions(rt, cwd.as_deref(), titles));
    }
    sessions.sort_by_key(|b| std::cmp::Reverse(b.mtime));
    if let Some(q) = &query {
        sessions.retain(|s| {
            s.id.to_lowercase().contains(q)
                || s.title
                    .as_deref()
                    .map(|t| t.to_lowercase().contains(q))
                    .unwrap_or(false)
                || trail::label_for_session(&s.id)
                    .map(|l| l.to_lowercase().contains(q))
                    .unwrap_or(false)
        });
        if sessions.is_empty() {
            println!("no sessions match \u{201c}{}\u{201d}", term_safe(q));
            println!("(codex titles come from its registry; claude/gemini need --titles to search transcripts)");
            return Ok(());
        }
    }

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
        let a = trail::ansi();
        let (dim, bold, reset) = (a.dim, a.bold, a.reset);
        // One ledger read for every row's naming, not one per row.
        let naming = trail::naming_index();
        for s in &sessions {
            // `·` marks a session known to be empty (only determinable with --titles).
            let mark = match s.has_conversation {
                Some(false) => "\u{b7}",
                _ => " ",
            };
            let color = if a.tty { trail::runtime_paint(s.runtime) } else { "" };
            let age = s
                .mtime
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| trail::ago(d.as_secs()))
                .unwrap_or_default();
            let label = naming
                .get(&s.id)
                .map(|(n, h, _)| {
                    format!(
                        "  {bold}{}{reset} {dim}\u{b7} {}{reset}",
                        term_safe(&trail::clip(n, 44)),
                        term_safe(h)
                    )
                })
                .unwrap_or_default();
            let title = s
                .title
                .as_deref()
                .filter(|t| !t.is_empty())
                .map(|t| format!("  {}", term_safe(&trail::clip(t, 56))))
                .unwrap_or_default();
            println!(
                "{mark} {color}{:<8}{reset} {} {dim}{age:>8}{reset}{label}{title}",
                s.runtime,
                term_safe(&s.id)
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
    // The one diagnostic that checks for a newer Constant (a single GitHub
    // API GET; quietly skipped offline). Drift fixes ship as releases — this
    // is how an existing install learns one exists.
    let current = env!("CARGO_PKG_VERSION");
    let latest = alembic::latest_release_version();
    let update = latest
        .as_deref()
        .filter(|l| alembic::version_newer(l, current))
        .map(str::to_string);
    if json {
        println!(
            "{}",
            serde_json::json!({
                "constant": {
                    "version": current,
                    "latest": latest,
                    "update_available": update.is_some(),
                },
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
                "opencode": {
                    "version": r.opencode_version,
                    "supported": alembic::SUPPORTED_OPENCODE,
                    "db": r.opencode_db,
                },
                "gemini": {
                    "version": r.gemini_version,
                    "supported": alembic::SUPPORTED_GEMINI,
                    "store": r.gemini_store,
                    "target": false,
                },
            })
        );
    } else {
        let a = trail::ansi();
        let (dim, bold, reset) = (a.dim, a.bold, a.reset);
        let amber = if a.tty { "\x1b[38;5;214m" } else { "" };
        let mark = |b: bool| if b { "ok" } else { "MISSING" };
        println!("{bold}constant doctor{reset}");
        match (&update, &latest) {
            (Some(u), _) => println!(
                "  {bold}constant{reset} : {current} {amber}\u{2014} v{u} AVAILABLE{reset} {dim}\u{b7} brew upgrade kennykankush/constant/constant{reset}"
            ),
            (None, Some(_)) => println!("  {bold}constant{reset} : {current} {dim}(latest){reset}"),
            (None, None) => println!("  {bold}constant{reset} : {current} {dim}(release check skipped \u{2014} offline?){reset}"),
        }
        let rt_line = |rt: &str| -> String {
            if a.tty {
                format!("{}{rt:<8}{reset}", trail::runtime_paint(rt))
            } else {
                format!("{rt:<8}")
            }
        };
        println!(
            "  {} : {} {dim}(cli {}, sessions {}, db {}) \u{2014} validated against {}.x{reset}",
            rt_line("codex"),
            r.codex_version.as_deref().unwrap_or("not found"),
            mark(r.codex_version.is_some()),
            mark(r.codex_store),
            mark(r.codex_db),
            alembic::SUPPORTED_CODEX,
        );
        println!(
            "  {} : {} {dim}(cli {}, projects {}) \u{2014} validated against {}.x{reset}",
            rt_line("claude"),
            r.claude_version.as_deref().unwrap_or("not found"),
            mark(r.claude_version.is_some()),
            mark(r.claude_store),
            alembic::SUPPORTED_CLAUDE,
        );
        println!(
            "  {} : {} {dim}(cli {}, db {}) \u{2014} validated against {}.x{reset}",
            rt_line("opencode"),
            r.opencode_version.as_deref().unwrap_or("not found"),
            mark(r.opencode_version.is_some()),
            mark(r.opencode_db),
            alembic::SUPPORTED_OPENCODE,
        );
        println!(
            "  {} : {} {dim}(cli {}, store {}) \u{2014} validated against {}.x; carry source only{reset}",
            rt_line("gemini"),
            r.gemini_version.as_deref().unwrap_or("not found"),
            mark(r.gemini_version.is_some()),
            mark(r.gemini_store),
            alembic::SUPPORTED_GEMINI,
        );
    }
    Ok(())
}

fn print_help() {
    let a = trail::ansi();
    let (dim, bold, reset) = (a.dim, a.bold, a.reset);
    let paint = |rt: &str| -> String {
        if a.tty {
            format!("{}{rt}{reset}", trail::runtime_paint(rt))
        } else {
            rt.to_string()
        }
    };
    let (codex, claude, opencode, gemini) = (
        paint("codex"),
        paint("claude"),
        paint("opencode"),
        paint("gemini"),
    );
    println!(
        "\
{bold}constant{reset} \u{2014} one conversation, any agent runtime.

{dim}LIVE{reset}
  {bold}constant host{reset} [codex|claude|opencode] {dim}[--prefix C-t] [--with-tools] [--no-bar] [--render full|paged]{reset}
        {dim}Host an agent CLI in a Constant PTY (default: codex, prefix: Ctrl-B).
        A status bar lives on the bottom row; --no-bar disables it.
        No --render flag: switching a LONG thread asks [v]erbatim \u{b7} [c]ompact
        (compact files older turns in the record \u{2014} recallable, never lost).{reset}

  {bold}constant resume{reset} [QUERY] {dim}[--in RT] [--list] [--all] [--prefix C-t] [--with-tools] [--no-bar] [--render paged]{reset}
        {dim}No QUERY (in a terminal): an interactive picker over every session
        in scope \u{2014} type to search, \u{2191}\u{2193} browse, Tab toggles folder/everywhere,
        Ctrl-T narrows to constant conversations, Enter wakes it hosted.
        With QUERY: matches a handle, name, or id from the trail. If every
        projection is gone, reprints from the record.{reset}

{dim}CARRY{reset}
  {bold}constant carry{reset} --to codex|claude|opencode {dim}[--from RT | --session PATH_OR_ID]
        [--json] [--dry-run] [--debug] [--new] [--with-tools] [--render paged] [--tail CHARS]{reset}
        {dim}Headless: carry a conversation into the target's native session and
        print the receipt + resume command. Sources: codex, claude, opencode,
        and gemini (source only). --new forks instead of refreshing;
        --with-tools carries tool calls/results (redacted, capped).
        (`distill` is an alias.){reset}

{dim}THE RECORD{reset}  {dim}\u{2014} every carry snapshots the FULL thread; filed is never lost{reset}
  {bold}constant recall{reset} HANDLE {dim}[chNN] [TURN | A-B]{reset}
        {dim}Read filed turns back, verbatim, by address (ch04\u{b7}12). Read-only.
        --render paged lays projections out as head card + index + recent
        turns verbatim; the index's addresses resolve here.{reset}

  {bold}constant snapshots{reset} {dim}[--all] [--full]{reset}
        {dim}List the record volumes per conversation (--full shows paths).{reset}

  {bold}constant audit{reset} {dim}[HANDLE] [--all] [--json] [--tail CHARS]{reset}
        {dim}Per chapter, what a paged render keeps verbatim vs files (recallable)
        \u{2014} the renderer's instruments, read-only, on real conversations.{reset}

  {bold}constant restore{reset} SNAPSHOT {dim}[--to codex|claude] [--json]{reset}
        {dim}Reprint a fresh native session from any volume. Never overwrites.{reset}

{dim}NAME & MOVE{reset}
  {bold}constant rename{reset} {dim}[--of HANDLE]{reset} NEW NAME...
        {dim}Name a conversation (locks the title; native pickers re-stamped).
        Inside a hosted session:  prefix then  :rename NEW NAME{reset}

  {bold}constant pack{reset} HANDLE {dim}[--out FILE]{reset}   {bold}constant unpack{reset} FILE
        {dim}Bundle a conversation (ledger + record volumes) into one portable
        file; unpack it on another machine, then resume by handle.{reset}

{dim}LOOK AROUND{reset}
  {bold}constant trail{reset} {dim}[--all] [--plain] [--full] [--events]{reset}
        {dim}In a terminal: the explorer \u{2014} type to search, Enter zooms into a
        conversation \u{2192} its chapters \u{2192} the filed turns, verbatim; Esc backs
        out, r resumes hosted. --plain prints the cards instead.{reset}
  {bold}constant sessions{reset} {dim}[QUERY] [--from RT] [--all] [--titles] [--plain] [--json]{reset}
        {dim}Carryable sessions on disk, newest first, linked to their handles.
        Bare in a terminal: the resume picker. QUERY filters the printout
        by name/id (codex names come from its own registry).{reset}
  {bold}constant ps{reset} {dim}[--deep] [--json]{reset}
        {dim}Every live agent process right now (alias: `live`). Read-only.
        --deep adds each agent's chapter and flags double-booked conversations.{reset}
  {bold}constant status{reset} {dim}[--all]{reset}    {bold}constant doctor{reset} {dim}[--json]{reset}    {bold}constant route{reset} {dim}[--all] [--json]{reset}
        {dim}Orientation \u{b7} runtime/codec preflight + update check \u{b7} fork-graph
        (--json emits the lineage DAG for external tools).{reset}
  {bold}constant export{reset} {dim}(--from RT | --session PATH) [--out FILE]{reset}
        {dim}The distilled, redacted neutral IR of a thread (stdout or FILE).{reset}

{dim}PREFIX KEY{reset}
  {dim}Default Ctrl-B. Inside tmux (which owns Ctrl-B), pick another:{reset}
      constant host codex --prefix C-t
      CONSTANT_PREFIX=C-g constant host codex

{dim}INSIDE A HOSTED SESSION{reset} {dim}(press the prefix, release, then){reset}
  {bold}c{reset} / {bold}C{reset}        {dim}continue in{reset} {claude} {dim}/ new continuation{reset}
  {bold}x{reset} / {bold}X{reset}        {dim}continue in{reset} {codex} {dim}/ new continuation{reset}
  {bold}o{reset} / {bold}O{reset}        {dim}continue in{reset} {opencode} {dim}/ new continuation{reset}
               {dim}(a long thread asks first: [v]erbatim \u{b7} [c]ompact \u{b7} esc){reset}
  {bold}H{reset}            {dim}handover \u{2014} sends a sign-out request to the agent; watch it
               write, then switch (the sign-out rides the carried tail){reset}
  {bold}t{reset}            {dim}the trail graph: \u{2191}\u{2193} pick a chapter, Enter lands in that
               projection; c/x/o switch, r rename, t/q close{reset}
  {bold}:{reset}            {dim}command line \u{2014} switch/new/rename/quit, Tab completes, \u{2191}\u{2193} history{reset}
  {bold}d{reset}            {dim}quit (the hosted CLI exits with it){reset}
  {bold}prefix again{reset} {dim}send a literal prefix key to the child{reset}

  {dim}({gemini} is a carry source \u{2014} its conversations carry IN; hosting it lands later){reset}
"
    );
}

#[cfg(test)]
mod tests {
    #[test]
    fn install_channel_judges_paths() {
        use super::{install_channel, InstallChannel};
        assert_eq!(
            install_channel("/opt/homebrew/Cellar/constant/0.3.1/bin/constant"),
            InstallChannel::Brew
        );
        assert_eq!(
            install_channel("/home/linuxbrew/.linuxbrew/bin/constant"),
            InstallChannel::Brew
        );
        assert_eq!(
            install_channel("/Users/x/.cargo/bin/constant"),
            InstallChannel::Cargo
        );
        assert_eq!(
            install_channel("/Users/x/.local/bin/constant"),
            InstallChannel::Standalone
        );
    }
}
