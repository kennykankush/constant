//! The Constant meta-harness: host an agent CLI inside a PTY, intercept a
//! tmux-style prefix key, and switch the underlying runtime live.
//!
//! ```text
//!   real terminal (raw mode)
//!     you type ──▶ [tokenizer + FSM] ──┬─▶ PTY master ──▶ child TUI (native)
//!     screen   ◀── stdout         ◀────┴── PTY master ◀── child TUI
//!
//!   PREFIX  →  prefix mode  →  c/x quick-switch, or  :  command line
//!   switch  →  kill child · (transcode session) · spawn target · keep hosting
//! ```
//!
//! Input subtlety: modern TUIs (e.g. codex) enable the **Kitty keyboard
//! protocol**, after which Ctrl-<key> arrives NOT as a raw control byte but as a
//! CSI-u escape sequence: Ctrl-B == `\x1b[98;5u` (codepoint 98='b', mods 5=Ctrl).
//! So the interceptor recognizes the prefix in BOTH encodings, swallows it, and
//! forwards every other sequence (terminal replies, key releases) untouched.

use anyhow::{Context, Result, bail};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, size};
use portable_pty::{Child, MasterPty, PtySize, native_pty_system};
use std::collections::HashMap;
use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Sender, channel};
use std::thread;

use crate::runtime::Runtime;

// Input FSM states.
const M_NORMAL: u8 = 0;
const M_PREFIX: u8 = 1;
const M_COMMAND: u8 = 2;

// Largest escape sequence we'll buffer before giving up and flushing it as-is,
// so a malformed/never-terminating stream can't grow the buffer forever (M8).
const MAX_ESC: usize = 256;

/// Parse a prefix-key spec like `C-b`, `ctrl-t`, `^g` into the control byte the
/// terminal sends in legacy mode (Ctrl-<L> == <L> & 0x1f), plus a human label.
pub fn parse_prefix(spec: &str) -> Result<(u8, String)> {
    let s = spec.trim().to_lowercase();
    let letter = s
        .strip_prefix("c-")
        .or_else(|| s.strip_prefix("ctrl-"))
        .or_else(|| s.strip_prefix('^'))
        .unwrap_or(s.as_str());
    let ch = letter
        .chars()
        .next()
        .filter(|c| c.is_ascii_alphabetic() && letter.len() == 1)
        .with_context(|| format!("invalid prefix `{spec}` (try C-b, C-t, C-g, ...)"))?;
    let byte = (ch.to_ascii_lowercase() as u8) & 0x1f;
    if byte == 0 {
        bail!("invalid prefix `{spec}`");
    }
    Ok((byte, format!("Ctrl-{}", ch.to_ascii_uppercase())))
}

/// Parse a Kitty keyboard-protocol CSI-u sequence `ESC [ cp[:alt] ; mods[:event] u`
/// into (codepoint, modifiers, event). event: 1=press, 2=repeat, 3=release.
fn parse_kitty_u(seq: &[u8]) -> Option<(u32, u32, u32)> {
    if seq.len() < 4 || seq[0] != 0x1b || seq[1] != b'[' || *seq.last().unwrap() != b'u' {
        return None;
    }
    let body = std::str::from_utf8(&seq[2..seq.len() - 1]).ok()?;
    let mut fields = body.split(';');
    let cp = fields.next()?.split(':').next()?.parse::<u32>().ok()?;
    let (mods, event) = match fields.next() {
        Some(m) => {
            let mut it = m.split(':');
            let mods = it.next().unwrap_or("1").parse::<u32>().unwrap_or(1);
            let event = it.next().and_then(|e| e.parse().ok()).unwrap_or(1);
            (mods, event)
        }
        None => (1, 1),
    };
    Some((cp, mods, event))
}

/// A normalized input unit produced by the tokenizer.
enum Token {
    Byte(u8),
    Prefix,
    Seq(Vec<u8>),
}

/// Reassembles escape sequences across reads and recognizes the prefix key in
/// both legacy (raw control byte) and Kitty-protocol (CSI-u) encodings.
struct Tokenizer {
    esc: Vec<u8>,
    prefix_byte: u8,
    prefix_cp: u32,
}

impl Tokenizer {
    fn new(prefix_byte: u8) -> Self {
        // For a Ctrl-<letter> prefix, the Kitty codepoint is the lowercase letter.
        Self {
            esc: Vec::new(),
            prefix_byte,
            prefix_cp: (prefix_byte | 0x60) as u32,
        }
    }

    fn feed(&mut self, bytes: &[u8], out: &mut Vec<Token>) {
        for &b in bytes {
            if !self.esc.is_empty() {
                self.esc.push(b);
                if let Some(seq) = self.take_if_complete() {
                    self.classify(seq, out);
                } else if self.esc.len() > MAX_ESC {
                    // Runaway/malformed escape: flush as-is and reset instead of
                    // buffering without bound (M8).
                    out.push(Token::Seq(std::mem::take(&mut self.esc)));
                }
                continue;
            }
            if b == 0x1b {
                self.esc.push(b);
            } else if b == self.prefix_byte {
                out.push(Token::Prefix);
            } else {
                out.push(Token::Byte(b));
            }
        }
    }

    /// Return the buffered escape sequence if it is now syntactically complete.
    fn take_if_complete(&mut self) -> Option<Vec<u8>> {
        let e = &self.esc;
        if e.len() < 2 {
            return None;
        }
        let complete = match e[1] {
            b'[' => e.len() >= 3 && (0x40..=0x7e).contains(e.last().unwrap()), // CSI: final byte
            b']' => {
                let last = *e.last().unwrap();
                last == 0x07 || (e.len() >= 2 && e[e.len() - 2] == 0x1b && last == 0x5c) // OSC: BEL or ST
            }
            b'O' => e.len() >= 3, // SS3
            0x1b => true,         // ESC ESC
            _ => true,            // ESC + single char (Alt-key, etc.)
        };
        if complete {
            Some(std::mem::take(&mut self.esc))
        } else {
            None
        }
    }

    fn classify(&self, seq: Vec<u8>, out: &mut Vec<Token>) {
        if let Some((cp, mods, event)) = parse_kitty_u(&seq) {
            let ctrl = mods.saturating_sub(1) & 4 != 0;
            if cp == self.prefix_cp && ctrl {
                if event != 3 {
                    out.push(Token::Prefix); // press/repeat → prefix
                }
                return; // swallow press AND release of the prefix
            }
        }
        out.push(Token::Seq(seq)); // everything else passes through verbatim
    }
}

/// The command key a prefix-mode token represents, decoded from EITHER a legacy
/// control/byte input OR a Kitty CSI-u press — so `c`/`x`/`d`/`:` work whatever
/// keyboard protocol the child negotiated (M7).
fn command_key(tok: &Token) -> Option<u8> {
    match tok {
        Token::Byte(b) => Some(*b),
        Token::Seq(s) => {
            let (cp, mods, event) = parse_kitty_u(s)?;
            // Ignore key releases (event 3) and any MODIFIED key (mods != 1): a
            // plain c/x/d/: only. Without this, Ctrl-C/Alt-C (cp=99, mods!=1)
            // would decode to bare `c` and trigger a switch — which the legacy
            // byte path never does (Ctrl-C is 0x03, not 'c').
            if event == 3 || mods != 1 {
                return None;
            }
            u8::try_from(cp).ok()
        }
        Token::Prefix => None,
    }
}

enum Ev {
    Stdin(Vec<u8>),
    Pty(Vec<u8>),
    PtyClosed,
    Resize,
}

struct Session {
    runtime: Runtime,
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
}

#[derive(Clone, Copy)]
struct SwitchRequest {
    target: Runtime,
    new: bool,
}

fn spawn_session(
    runtime: Runtime,
    resume: Option<&str>,
    cwd: Option<&Path>,
    cols: u16,
    rows: u16,
    tx: Sender<Ev>,
) -> Result<Session> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .context("openpty")?;

    let mut cmd = match resume {
        Some(id) => runtime.resume_command(id),
        None => runtime.fresh_command(),
    };
    if let Some(dir) = cwd {
        cmd.cwd(dir);
    }

    let child = pair
        .slave
        .spawn_command(cmd)
        .with_context(|| format!("failed to launch `{}` — is it on PATH?", runtime.label()))?;
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().context("clone pty reader")?;
    let writer = pair.master.take_writer().context("take pty writer")?;

    thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => {
                    let _ = tx.send(Ev::PtyClosed);
                    break;
                }
                Ok(n) => {
                    if tx.send(Ev::Pty(buf[..n].to_vec())).is_err() {
                        break;
                    }
                }
            }
        }
    });

    Ok(Session {
        runtime,
        master: pair.master,
        writer,
        child,
    })
}

/// Stop a child gracefully: SIGTERM first so it can flush state and restore its
/// own terminal modes, then SIGKILL as a fallback if it ignores us (S2). Either
/// way the child exits, closing the pty and driving the switch/quit flow.
fn terminate(child: &mut Box<dyn Child + Send + Sync>) {
    #[cfg(unix)]
    if let Some(pid) = child.process_id() {
        unsafe {
            libc::kill(pid as i32, libc::SIGTERM);
        }
        for _ in 0..15 {
            if matches!(child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
    let _ = child.kill();
}

fn request_switch(
    target: Runtime,
    new: bool,
    session: &mut Session,
    switching_to: &mut Option<SwitchRequest>,
) {
    if session.runtime == target || switching_to.is_some() {
        return;
    }
    *switching_to = Some(SwitchRequest { target, new });
    terminate(&mut session.child);
}

#[derive(PartialEq)]
enum Action {
    None,
    Quit,
}

fn execute_command(
    line: &str,
    session: &mut Session,
    switching_to: &mut Option<SwitchRequest>,
) -> Action {
    let parts: Vec<&str> = line.split_whitespace().collect();
    match parts.as_slice() {
        ["switch", rt] | ["s", rt] => {
            if let Ok(target) = Runtime::parse(rt) {
                request_switch(target, false, session, switching_to);
            }
            Action::None
        }
        ["new", rt] | ["n", rt] | ["fork", rt] | ["switch", "--new", rt] | ["s", "--new", rt] => {
            if let Ok(target) = Runtime::parse(rt) {
                request_switch(target, true, session, switching_to);
            }
            Action::None
        }
        ["quit"] | ["q"] | ["detach"] => Action::Quit,
        _ => Action::None,
    }
}

// Terminal chrome is best-effort (M3): a cosmetic write failure must never tear
// down a live session, matching how writes to the child are already swallowed.
fn bottom_overlay(out: &mut impl Write, text: &str) {
    let (_, rows) = size().unwrap_or((80, 24));
    let _ = write!(out, "\x1b7\x1b[{rows};1H\x1b[7m{text}\x1b[0m\x1b[K\x1b8");
    let _ = out.flush();
}

fn clear_bottom(out: &mut impl Write) {
    let (_, rows) = size().unwrap_or((80, 24));
    let _ = write!(out, "\x1b7\x1b[{rows};1H\x1b[2K\x1b8");
    let _ = out.flush();
}

fn banner(out: &mut impl Write, runtime: Runtime, prefix_label: &str) {
    let _ = write!(
        out,
        "\x1b[2m  constant · hosting {} · {prefix_label} then  c=claude  C=new claude  x=codex  X=new codex  :=command  d=detach\x1b[0m\r\n",
        runtime.label(),
    );
    let _ = out.flush();
}

/// Escape sequences that undo every terminal mode a hosted child might have
/// turned on — alt-screen, mouse tracking, focus reporting, bracketed paste, and
/// the Kitty keyboard protocol. Required because we SIGKILL children, so they
/// never run their own cleanup and would otherwise leave the terminal wedged.
const TERM_RESET: &[u8] = b"\x1b[?1049l\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[?1004l\x1b[?2004l\x1b[<u\x1b[?25h\x1b[0m";

fn write_reset(out: &mut impl Write) {
    let _ = out.write_all(TERM_RESET);
    let _ = out.flush();
}

/// RAII restore: runs on every exit path — normal return, `?` error, or panic —
/// so the user's terminal is never left in raw mode or a child's escape modes.
struct TerminalGuard;
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let mut out = std::io::stdout();
        let _ = out.write_all(TERM_RESET);
        let _ = out.write_all(b"\r\n");
        let _ = out.flush();
        let _ = disable_raw_mode();
    }
}

/// Describe a raw input byte for the key probe.
#[allow(clippy::match_overlapping_arm)] // specific bytes handled before the ranges, on purpose
fn describe_byte(b: u8) -> String {
    match b {
        0x1b => "ESC".to_string(),
        0x7f => "DEL/Backspace".to_string(),
        b' ' => "Space".to_string(),
        0..=0x1f => format!("Ctrl-{}", (b | 0x40) as char),
        0x20..=0x7e => format!("'{}'", b as char),
        _ => "non-ascii".to_string(),
    }
}

/// `constant keys` — print the raw byte(s) each keypress produces. Quit with q / Ctrl-C.
pub fn debug_keys() -> Result<()> {
    if !std::io::stdin().is_terminal() {
        bail!("`constant keys` must run in an interactive terminal (a TTY)");
    }
    enable_raw_mode().context("enable raw mode")?;
    let mut out = std::io::stdout();
    let _ = write!(
        out,
        "constant key probe — press any key to see its byte(s). Press q or Ctrl-C to quit.\r\n"
    );
    let _ = out.flush();

    let mut stdin = std::io::stdin();
    let mut buf = [0u8; 64];
    'outer: loop {
        let n = match stdin.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        for &b in &buf[..n] {
            let _ = write!(out, "  byte 0x{b:02x}  ({})\r\n", describe_byte(b));
            if b == b'q' || b == 0x03 {
                break 'outer;
            }
        }
        let _ = out.flush();
    }
    let _ = disable_raw_mode();
    let _ = write!(out, "\r\nprobe done.\r\n");
    let _ = out.flush();
    Ok(())
}

/// Entry point for `constant host [runtime] [--prefix ...]`.
pub fn run(initial: Runtime, prefix: u8, prefix_label: String) -> Result<()> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        bail!("`constant host` must run in an interactive terminal (a TTY)");
    }

    let (cols, rows) = size().unwrap_or((80, 24));
    enable_raw_mode().context("enable raw mode")?;
    let _guard = TerminalGuard; // restores the terminal on any exit path

    let (tx, rx) = channel::<Ev>();

    {
        let tx = tx.clone();
        thread::spawn(move || {
            let mut stdin = std::io::stdin();
            let mut buf = [0u8; 4096];
            loop {
                match stdin.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tx.send(Ev::Stdin(buf[..n].to_vec())).is_err() {
                            break;
                        }
                    }
                }
            }
        });
    }

    {
        let tx = tx.clone();
        thread::spawn(move || {
            if let Ok(mut signals) =
                signal_hook::iterator::Signals::new([signal_hook::consts::SIGWINCH])
            {
                for _ in signals.forever() {
                    if tx.send(Ev::Resize).is_err() {
                        break;
                    }
                }
            }
        });
    }

    let prefix_hint = format!(
        " {prefix_label} ▸  c=claude   C=new claude   x=codex   X=new codex   :=command   d=detach "
    );

    let mut out = std::io::stdout();

    let mut dbg: Option<std::fs::File> = if std::env::var("CONSTANT_DEBUG").is_ok() {
        std::fs::File::create("/tmp/constant-debug.log").ok()
    } else {
        None
    };
    if let Some(f) = dbg.as_mut() {
        let _ = writeln!(
            f,
            "[start] prefix=0x{prefix:02x} ({prefix_label}) cp={}",
            prefix | 0x60
        );
        let _ = f.flush();
    }

    let host_cwd = std::env::current_dir().ok();
    let mut tokenizer = Tokenizer::new(prefix);
    // Constant-owned projections: runtime -> (session id, file). We ONLY ever
    // write/resume these; the user's original sessions are read but NEVER
    // overwritten (F1). The pair stays stable as you ping-pong (no
    // proliferation). The read source for `from` is its projection if we have
    // one, else whatever session the user is currently in (the original seed on
    // the first switch, or after a `/resume` inside the child).
    let mut owned: HashMap<Runtime, (String, PathBuf)> = HashMap::new();
    // Trail: a switch counter, the conversation's stable key (its root source
    // session id, for grouping), and a readable handle (its first user message,
    // for naming `constant·tNN·from-<src>·<root-slug>`).
    let mut trail_n: u32 = 0;
    let mut root_id: Option<String> = None;
    let mut root_slug: Option<String> = None;
    let mut session = spawn_session(initial, None, None, cols, rows, tx.clone())?;
    banner(&mut out, session.runtime, &prefix_label);

    let mut mode = M_NORMAL;
    let mut cmd_buf = String::new();
    let mut switching_to: Option<SwitchRequest> = None;
    let mut pending_out: Vec<u8> = Vec::new();
    let mut quitting = false;

    for ev in rx.iter() {
        match ev {
            Ev::Pty(bytes) => {
                if mode == M_COMMAND {
                    pending_out.extend_from_slice(&bytes);
                } else {
                    let _ = out.write_all(&bytes);
                    let _ = out.flush();
                }
            }

            Ev::PtyClosed => {
                if let Some(request) = switching_to.take() {
                    let target = request.target;
                    let from = session.runtime;
                    let _ = session.child.wait();
                    let (c, r) = size().unwrap_or((cols, rows));
                    write_reset(&mut out); // undo outgoing child's terminal modes
                    let _ = out.write_all(b"\x1b[2J\x1b[H");

                    // Read source for `from`. Prefer our tracked projection — it's
                    // the established ping-pong pair, positively tied to the child
                    // we spawned — so an unrelated newer session in the same cwd is
                    // NOT adopted (another codex/claude process must not hijack the
                    // carry). Use the live active session when it's the child's OWN
                    // session (same id — e.g. codex wrote a fresh rollout file for
                    // the resumed id) so we still capture the latest turns, or as
                    // the seed on the very first switch.
                    // NB: a `/resume` to a *different* conversation inside the child
                    // is deliberately not auto-followed — it's indistinguishable
                    // from an unrelated same-cwd session (documented limitation).
                    let active = crate::alembic::active_session(from, host_cwd.as_deref());
                    let tracked = owned
                        .get(&from)
                        .and_then(|(id, p)| p.exists().then(|| (p.clone(), id.clone())));
                    let src = match (tracked, active) {
                        (Some((tpath, tid)), Some((apath, aid))) => {
                            if aid == tid {
                                Some((apath, aid)) // child's own session, freshest file
                            } else {
                                Some((tpath, tid)) // ignore unrelated newer session
                            }
                        }
                        (Some(t), None) => Some(t),
                        (None, Some(a)) => Some(a), // first switch / seed
                        (None, None) => None,
                    };

                    match src {
                        Some((src_path, src_id)) => {
                            // Which conversation does the LIVE source belong to?
                            // Re-resolved every switch (via the ledger), not just
                            // the first — so if the user `/resume`d a *different*
                            // conversation inside the child, we detect the change
                            // instead of overwriting the old pair's projection with
                            // an unrelated thread and mis-recording the trail (M6).
                            let (cid, last_n, projs) = crate::trail::resume(&src_path, &src_id);
                            if root_id.as_deref() != Some(cid.as_str()) {
                                // New conversation (first switch, or a /resume-away):
                                // adopt it, reseed the projection map from the
                                // ledger, and DROP the previous conversation's
                                // projections so they're never reused as a target.
                                root_id = Some(cid.clone());
                                trail_n = last_n;
                                owned.clear();
                                for (rt, id, path) in projs {
                                    owned.insert(rt, (id, path));
                                }
                                let name = crate::alembic::root_name(&src_path, from)
                                    .unwrap_or_else(|| "conversation".to_string());
                                root_slug = Some(crate::trail::slug(&name));
                            }
                            let conv_id = cid;
                            let slug = root_slug.clone().unwrap_or_default();
                            // Candidate trail number; committed only on a successful
                            // carry, so a failed carry consumes no t-number.
                            let n = trail_n + 1;
                            let title = crate::trail::title(n, from, &slug);

                            let action = if request.new { "new" } else { "continue" };
                            let _ = out.write_all(
                                format!(
                                    "\x1b[2m  trail · {} · {} \u{2192} {} · {action}\x1b[0m\r\n",
                                    title,
                                    from.label(),
                                    target.label()
                                )
                                .as_bytes(),
                            );
                            let _ = out.flush();

                            // Never write to the user's originals: reuse only our
                            // own projection for `target`, else mint a fresh one.
                            let reuse_owned = if request.new {
                                None
                            } else {
                                owned.get(&target).cloned()
                            };
                            let reuse = reuse_owned
                                .as_ref()
                                .map(|(id, p)| (id.as_str(), p.as_path()));
                            match crate::alembic::distill_path(
                                &src_path,
                                target,
                                reuse,
                                Some(&title),
                            ) {
                                Ok((id, written, session_cwd)) => {
                                    if let Some(f) = dbg.as_mut() {
                                        let _ = writeln!(
                                            f,
                                            "[alembic] {} -> {} as {id} (reused={})",
                                            from.label(),
                                            target.label(),
                                            reuse_owned.is_some()
                                        );
                                        let _ = f.flush();
                                    }
                                    trail_n = n; // commit only on success
                                    owned.insert(target, (id.clone(), written.clone()));
                                    crate::trail::record(
                                        n,
                                        &conv_id,
                                        &slug,
                                        host_cwd.as_deref(),
                                        &src_id,
                                        &src_path,
                                        from,
                                        target,
                                        &id,
                                        &written,
                                        &title,
                                        if reuse_owned.is_some() {
                                            "refresh-existing"
                                        } else {
                                            "new-fork"
                                        },
                                    );
                                    session = spawn_session(
                                        target,
                                        Some(&id),
                                        session_cwd.as_deref(),
                                        c,
                                        r,
                                        tx.clone(),
                                    )?;
                                }
                                Err(e) => {
                                    if let Some(f) = dbg.as_mut() {
                                        let _ = writeln!(f, "[alembic] failed: {e:#}");
                                        let _ = f.flush();
                                    }
                                    // Fresh fallback: abandon recovered state so the
                                    // new session starts a clean conversation and
                                    // never reuses a prior projection. trail_n is
                                    // left un-advanced.
                                    owned.clear();
                                    root_id = None;
                                    root_slug = None;
                                    let _ = out.write_all(
                                        format!("\x1b[2m  (couldn't carry — {e}; starting fresh)\x1b[0m\r\n")
                                            .as_bytes(),
                                    );
                                    let _ = out.flush();
                                    session = spawn_session(target, None, None, c, r, tx.clone())?;
                                }
                            }
                        }
                        None => {
                            let _ = out.write_all(
                                b"\x1b[2m  (no conversation here to carry; starting fresh)\x1b[0m\r\n",
                            );
                            let _ = out.flush();
                            session = spawn_session(target, None, None, c, r, tx.clone())?;
                        }
                    }
                    banner(&mut out, session.runtime, &prefix_label);
                } else {
                    break;
                }
            }

            Ev::Resize => {
                let (c, r) = size().unwrap_or((cols, rows));
                let _ = session.master.resize(PtySize {
                    rows: r,
                    cols: c,
                    pixel_width: 0,
                    pixel_height: 0,
                });
            }

            Ev::Stdin(bytes) => {
                if let Some(f) = dbg.as_mut() {
                    let _ = writeln!(f, "[stdin] mode={mode} bytes={bytes:02x?}");
                    let _ = f.flush();
                }

                let mut tokens = Vec::new();
                tokenizer.feed(&bytes, &mut tokens);

                let mut passthrough: Vec<u8> = Vec::new();
                for tok in tokens {
                    match mode {
                        M_NORMAL => match tok {
                            Token::Prefix => {
                                if let Some(f) = dbg.as_mut() {
                                    let _ = writeln!(f, "[prefix] -> PREFIX mode");
                                    let _ = f.flush();
                                }
                                if !passthrough.is_empty() {
                                    let _ = session.writer.write_all(&passthrough);
                                    passthrough.clear();
                                }
                                mode = M_PREFIX;
                                bottom_overlay(&mut out, &prefix_hint);
                            }
                            Token::Byte(b) => passthrough.push(b),
                            Token::Seq(s) => passthrough.extend_from_slice(&s),
                        },

                        M_PREFIX => {
                            clear_bottom(&mut out);
                            mode = M_NORMAL;
                            if matches!(tok, Token::Prefix) {
                                passthrough.push(prefix); // literal prefix to child
                            } else {
                                match command_key(&tok) {
                                    Some(b'c') => request_switch(
                                        Runtime::Claude,
                                        false,
                                        &mut session,
                                        &mut switching_to,
                                    ),
                                    Some(b'C') => request_switch(
                                        Runtime::Claude,
                                        true,
                                        &mut session,
                                        &mut switching_to,
                                    ),
                                    Some(b'x') => request_switch(
                                        Runtime::Codex,
                                        false,
                                        &mut session,
                                        &mut switching_to,
                                    ),
                                    Some(b'X') => request_switch(
                                        Runtime::Codex,
                                        true,
                                        &mut session,
                                        &mut switching_to,
                                    ),
                                    Some(b'd') => quitting = true,
                                    Some(b':') => {
                                        mode = M_COMMAND;
                                        cmd_buf.clear();
                                        bottom_overlay(&mut out, " constant ▸ ");
                                    }
                                    _ => {} // unknown: ignore
                                }
                            }
                        }

                        M_COMMAND => {
                            let mut submit = false;
                            let mut cancel = false;
                            match tok {
                                Token::Byte(b) => match b {
                                    0x0d | 0x0a => submit = true,
                                    0x1b => cancel = true,
                                    0x7f | 0x08 => {
                                        cmd_buf.pop();
                                        bottom_overlay(&mut out, &format!(" constant ▸ {cmd_buf}"));
                                    }
                                    _ => {
                                        if b == b' ' || b.is_ascii_graphic() {
                                            cmd_buf.push(b as char);
                                            bottom_overlay(
                                                &mut out,
                                                &format!(" constant ▸ {cmd_buf}"),
                                            );
                                        }
                                    }
                                },
                                Token::Seq(s) => {
                                    if let Some((cp, _, event)) = parse_kitty_u(&s)
                                        && event != 3
                                    {
                                        if cp == 13 {
                                            submit = true;
                                        } else if cp == 27 {
                                            cancel = true;
                                        }
                                    }
                                }
                                Token::Prefix => {}
                            }
                            if submit || cancel {
                                clear_bottom(&mut out);
                                mode = M_NORMAL;
                                if !pending_out.is_empty() {
                                    let _ = out.write_all(&pending_out);
                                    let _ = out.flush();
                                    pending_out.clear();
                                }
                                if submit
                                    && execute_command(
                                        cmd_buf.trim(),
                                        &mut session,
                                        &mut switching_to,
                                    ) == Action::Quit
                                {
                                    quitting = true;
                                }
                            }
                        }

                        _ => unreachable!(),
                    }
                    if quitting {
                        break;
                    }
                }

                if !passthrough.is_empty() {
                    let _ = session.writer.write_all(&passthrough);
                    let _ = session.writer.flush();
                }
                if quitting {
                    break;
                }
            }
        }

        if quitting {
            break;
        }
    }

    terminate(&mut session.child);
    let _ = session.child.wait();
    Ok(())
    // TerminalGuard restores the terminal on drop.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_prefix_legacy_bytes() {
        assert_eq!(parse_prefix("C-b").unwrap(), (0x02, "Ctrl-B".to_string()));
        assert_eq!(parse_prefix("ctrl-t").unwrap().0, 0x14);
        assert_eq!(parse_prefix("^g").unwrap().0, 0x07);
        assert!(parse_prefix("nonsense").is_err());
        assert!(parse_prefix("").is_err());
    }

    #[test]
    fn parse_kitty_u_decodes_fields() {
        // Ctrl-b press: codepoint 98 ('b'), mods 5 (=1+Ctrl), event 1 (press).
        assert_eq!(parse_kitty_u(b"\x1b[98;5u"), Some((98, 5, 1)));
        // Explicit release event.
        assert_eq!(parse_kitty_u(b"\x1b[98;5:3u"), Some((98, 5, 3)));
        // Not a CSI-u sequence.
        assert_eq!(parse_kitty_u(b"abc"), None);
    }

    #[test]
    fn command_key_from_both_encodings() {
        assert_eq!(command_key(&Token::Byte(b'c')), Some(b'c'));
        // Kitty press of 'c' (codepoint 99, unmodified mods=1).
        assert_eq!(command_key(&Token::Seq(b"\x1b[99;1u".to_vec())), Some(b'c'));
        // Release is ignored.
        assert_eq!(command_key(&Token::Seq(b"\x1b[99;1:3u".to_vec())), None);
        // A MODIFIED key (Ctrl-C: mods=5) must NOT decode as plain `c`.
        assert_eq!(command_key(&Token::Seq(b"\x1b[99;5u".to_vec())), None);
        assert_eq!(command_key(&Token::Prefix), None);
    }

    fn classify_all(bytes: &[u8], prefix_byte: u8) -> Vec<Token> {
        let mut tk = Tokenizer::new(prefix_byte);
        let mut out = Vec::new();
        tk.feed(bytes, &mut out);
        out
    }

    #[test]
    fn tokenizer_detects_prefix_legacy_and_kitty() {
        // Legacy Ctrl-B (0x02).
        let toks = classify_all(&[0x02], 0x02);
        assert!(matches!(toks.as_slice(), [Token::Prefix]));
        // Kitty CSI-u Ctrl-b.
        let toks = classify_all(b"\x1b[98;5u", 0x02);
        assert!(matches!(toks.as_slice(), [Token::Prefix]));
        // A normal byte passes through.
        let toks = classify_all(b"a", 0x02);
        assert!(matches!(toks.as_slice(), [Token::Byte(b'a')]));
    }

    #[test]
    fn tokenizer_caps_runaway_escape() {
        // A never-terminating escape must not grow the buffer without bound (M8):
        // it gets flushed as a Seq once it exceeds MAX_ESC.
        let mut blob = vec![0x1b, b'['];
        blob.extend(std::iter::repeat_n(b'0', MAX_ESC * 2));
        let toks = classify_all(&blob, 0x02);
        assert!(
            toks.iter().any(|t| matches!(t, Token::Seq(_))),
            "runaway escape was not flushed"
        );
    }
}
