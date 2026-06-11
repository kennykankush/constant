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
/// The control room: Constant's own full-screen view (the trail graph),
/// toggled from prefix mode. Child output is dropped while open — the child
/// repaints on exit via a resize wiggle.
const M_VIEW: u8 = 3;

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
    /// Inside a bracketed paste (ESC[200~ … ESC[201~): pasted bytes that happen
    /// to contain the prefix byte must NOT trigger prefix mode — a paste could
    /// otherwise fire a real runtime switch mid-paste.
    in_paste: bool,
}

impl Tokenizer {
    fn new(prefix_byte: u8) -> Self {
        // For a Ctrl-<letter> prefix, the Kitty codepoint is the lowercase letter.
        Self {
            esc: Vec::new(),
            prefix_byte,
            prefix_cp: (prefix_byte | 0x60) as u32,
            in_paste: false,
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
            } else if b == self.prefix_byte && !self.in_paste {
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

    fn classify(&mut self, seq: Vec<u8>, out: &mut Vec<Token>) {
        // Track bracketed-paste state; the markers themselves pass through.
        if seq.as_slice() == b"\x1b[200~" {
            self.in_paste = true;
            out.push(Token::Seq(seq));
            return;
        }
        if seq.as_slice() == b"\x1b[201~" {
            self.in_paste = false;
            out.push(Token::Seq(seq));
            return;
        }
        if !self.in_paste && let Some((cp, mods, event)) = parse_kitty_u(&seq) {
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
    /// Background installed-version preflight: (runtime, version, validated),
    /// plus a newer Constant release when one exists (checked ONLY when some
    /// runtime is unvalidated — drift detected — never in the happy path).
    Versions(Vec<(Runtime, String, bool)>, Option<String>),
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

enum SpawnMode<'a> {
    /// A fresh launch. For runtimes that support it, the session id is MINTED
    /// and DECLARED here so the harness knows the child's identity instead of
    /// inferring it from the filesystem later.
    Fresh { session_id: Option<&'a str> },
    /// Resume an existing session by id.
    Resume(&'a str),
}

fn spawn_session(
    runtime: Runtime,
    mode: SpawnMode,
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

    let mut cmd = match mode {
        SpawnMode::Resume(id) => runtime.resume_command(id),
        SpawnMode::Fresh { session_id } => runtime.fresh_command(session_id),
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
        // Up to 1s of grace: a child flushing a large session file on a slow
        // disk must not be SIGKILLed mid-write (we read that file next). The
        // loop exits as soon as the child is gone, so quick exits stay quick.
        for _ in 0..100 {
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
    if session.runtime == target
        || switching_to.is_some()
        || !crate::alembic::supports_target(target)
    {
        return;
    }
    // Termination is deferred to the carry gate at the end of the input
    // tick: a CONTINUE switch with nothing to carry is cancelled there
    // instead of tearing the child down for an empty target.
    *switching_to = Some(SwitchRequest { target, new });
}

/// One dim parenthetical status line.
fn dim(out: &mut impl Write, text: &str) {
    let _ = write!(out, "\x1b[2m  ({text})\x1b[0m\r\n");
    let _ = out.flush();
}

/// Spawn a runtime fresh, minting + declaring a session id when the CLI
/// supports it (claude `--session-id`). Returns the session and the declared id.
fn spawn_fresh(
    runtime: Runtime,
    cwd: Option<&Path>,
    cols: u16,
    rows: u16,
    tx: Sender<Ev>,
) -> Result<(Session, Option<String>)> {
    let minted = match runtime {
        Runtime::Claude => crate::alembic::claude_supports_session_id()
            .then(|| uuid::Uuid::new_v4().to_string()),
        // Codex/gemini have no fresh-id flag; opencode fresh sessions are
        // detected via its db (the fence applies there too).
        _ => None,
    };
    let session = spawn_session(
        runtime,
        SpawnMode::Fresh {
            session_id: minted.as_deref(),
        },
        cwd,
        cols,
        rows,
        tx,
    )?;
    Ok((session, minted))
}

/// Launch the post-switch child with a fallback ladder instead of dying:
/// target resumed (the carry) → target fresh → back to the previous runtime
/// (resumed if possible, else fresh). A failed launch must never tear down the
/// whole harness while a recoverable step remains. Returns the live session
/// plus its declared session id when known.
#[allow(clippy::too_many_arguments)]
fn spawn_settled(
    target: Runtime,
    resume_id: Option<&str>,
    from: Runtime,
    from_resume: Option<&str>,
    cwd: Option<&Path>,
    cols: u16,
    rows: u16,
    tx: &Sender<Ev>,
    out: &mut impl Write,
) -> Result<(Session, Option<String>)> {
    if let Some(id) = resume_id {
        match spawn_session(target, SpawnMode::Resume(id), cwd, cols, rows, tx.clone()) {
            Ok(s) => return Ok((s, Some(id.to_string()))),
            Err(e) => dim(
                out,
                &format!(
                    "couldn't launch {} resumed — {e}; trying fresh",
                    target.label()
                ),
            ),
        }
    }
    match spawn_fresh(target, None, cols, rows, tx.clone()) {
        Ok(r) => return Ok(r),
        Err(e) => dim(
            out,
            &format!(
                "couldn't launch {} — {e}; returning to {}",
                target.label(),
                from.label()
            ),
        ),
    }
    if let Some(id) = from_resume
        && let Ok(s) = spawn_session(from, SpawnMode::Resume(id), None, cols, rows, tx.clone())
    {
        return Ok((s, Some(id.to_string())));
    }
    spawn_fresh(from, None, cols, rows, tx.clone())
}

#[derive(PartialEq)]
enum Action {
    None,
    Quit,
    Rename(String),
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
        ["rename", rest @ ..] if !rest.is_empty() => Action::Rename(rest.join(" ")),
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
    let color = runtime_color(runtime.label());
    let _ = write!(
        out,
        "\x1b[2m  constant \u{b7} hosting \x1b[0m{color}{}\x1b[0m\x1b[2m \u{b7} {prefix_label} then  c=claude  x=codex  o=opencode  (shift=new)  t=trail  :=command  d=quit\x1b[0m\r\n",
        runtime.label(),
    );
    let _ = out.flush();
}

/// Escape sequences that undo every terminal mode a hosted child might have
/// turned on — alt-screen, mouse tracking, focus reporting, bracketed paste, and
/// the Kitty keyboard protocol. Required because we SIGKILL children, so they
/// never run their own cleanup and would otherwise leave the terminal wedged.
const TERM_RESET: &[u8] = b"\x1b[?1049l\x1b[r\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[?1004l\x1b[?2004l\x1b[<u\x1b[?25h\x1b[0m";

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

// ---- control room --------------------------------------------------------

/// "2h ago" / "yesterday" / "jun 09" — the graph's time column.
fn relative_time(ts: u64, now: u64) -> String {
    if ts == 0 || ts > now {
        return String::new();
    }
    let d = now - ts;
    match d {
        0..=59 => "just now".to_string(),
        60..=3599 => format!("{}m ago", d / 60),
        3600..=86_399 => format!("{}h ago", d / 3600),
        86_400..=172_799 => "yesterday".to_string(),
        _ => format!("{}d ago", d / 86_400),
    }
}

/// A 256-color accent per runtime (the graph's rail colors).
fn runtime_color(runtime: &str) -> &'static str {
    match runtime {
        "claude" => "\x1b[38;5;208m",  // orange
        "codex" => "\x1b[38;5;39m",    // blue
        "opencode" => "\x1b[38;5;77m", // green
        "gemini" => "\x1b[38;5;177m",  // violet
        _ => "\x1b[0m",
    }
}

const DIM: &str = "\x1b[2m";
const RESET: &str = "\x1b[0m";

/// Render the trail graph — newest chapter on top, GitLab-style rails, one
/// dot per chapter colored by the narrator that produced it. Pure: testable.
fn render_graph(
    naming: Option<&crate::trail::Naming>,
    chapters: &[crate::trail::ChapterRow],
    hosting: &str,
    rows: u16,
    now: u64,
) -> String {
    let mut out = String::new();
    out.push_str("\x1b[2J\x1b[H");

    match naming {
        Some(nm) => {
            out.push_str(&format!(
                "\r\n  {DIM}constant \u{b7}{RESET} \x1b[1m{}\x1b[0m {DIM}\u{b7}{RESET} {}\r\n\r\n",
                crate::term_safe(&nm.name),
                crate::term_safe(&nm.handle),
            ));
        }
        None => {
            out.push_str(&format!(
                "\r\n  {DIM}constant \u{b7} no thread yet — chapters appear after your first switch{RESET}\r\n\r\n"
            ));
        }
    }

    // Head: where you are right now (the chapter being written).
    let head_color = runtime_color(hosting);
    out.push_str(&format!(
        "  {head_color}\u{25c9}{RESET}  \x1b[1mch{:02}\x1b[0m  {head_color}{}{RESET}  {DIM}\u{2190} you are here{RESET}\r\n",
        chapters.len() + 1,
        hosting,
    ));

    // Budget: header(4) + head(1) + footer(2).
    let budget = rows.saturating_sub(8) as usize;
    let shown = chapters.len().min(budget.max(1));
    let hidden = chapters.len() - shown;

    for ch in chapters.iter().rev().take(shown) {
        let color = runtime_color(&ch.from);
        let rail = format!("  {DIM}\u{2502}{RESET}\r\n");
        out.push_str(&rail);
        let glyph = match ch.mode.as_str() {
            "new-fork" => format!("{DIM}\u{251c}\u{2500}{RESET}\u{25cf}"),
            "restore" => format!("{DIM}\u{2570}\u{2500}{RESET}\u{25cb}"),
            _ => format!("{color}\u{25cf}{RESET}"),
        };
        let record = if ch.recorded {
            format!("  {DIM}rec \u{2713}{RESET}")
        } else {
            String::new()
        };
        out.push_str(&format!(
            "  {glyph}  ch{:02}  {color}{:<8}{RESET} {DIM}\u{2192}{RESET} {:<8}  {DIM}{}{RESET}{record}\r\n",
            ch.n,
            crate::term_safe(&ch.from),
            crate::term_safe(&ch.to),
            relative_time(ch.ts, now),
        ));
    }
    if hidden > 0 {
        out.push_str(&format!("  {DIM}\u{2502}  \u{2026} {hidden} earlier chapters{RESET}\r\n"));
    }

    out.push_str(&format!(
        "\r\n  {DIM}c/x/o switch (shift=new) \u{b7} r rename \u{b7} t/q close{RESET}\r\n"
    ));
    out
}

/// Force the child to repaint after the control room owned the screen: a
/// resize wiggle (one row down, back up) delivers SIGWINCH twice — every
/// hosted TUI redraws on it. We never composite, so this is how the child's
/// frame comes back.
/// The validated release line for a runtime (mirrors `constant doctor`).
fn supported_line(rt: Runtime) -> &'static str {
    match rt {
        Runtime::Codex => crate::alembic::SUPPORTED_CODEX,
        Runtime::Claude => crate::alembic::SUPPORTED_CLAUDE,
        Runtime::Gemini => crate::alembic::SUPPORTED_GEMINI,
        Runtime::OpenCode => crate::alembic::SUPPORTED_OPENCODE,
    }
}

/// Cap on output held while the control room is open (a chatty child must not
/// grow memory without bound; past the cap the freshest output wins nothing —
/// the view is a pause, not a recorder).
const VIEW_BUF_CAP: usize = 1 << 20;

/// Track the child's alternate-screen state from its output stream. Escape
/// sequences can split across reads, so a small tail carries between calls.
fn track_alt(active: &mut bool, tail: &mut Vec<u8>, chunk: &[u8]) {
    const ON: &[u8] = b"\x1b[?1049h";
    const OFF: &[u8] = b"\x1b[?1049l";
    let mut buf = Vec::with_capacity(tail.len() + chunk.len());
    buf.extend_from_slice(tail);
    buf.extend_from_slice(chunk);
    let mut i = 0;
    while i + ON.len() <= buf.len() {
        if &buf[i..i + ON.len()] == ON {
            *active = true;
        } else if &buf[i..i + OFF.len()] == OFF {
            *active = false;
        }
        i += 1;
    }
    let keep = buf.len().min(ON.len() - 1);
    *tail = buf[buf.len() - keep..].to_vec();
}

/// Feed one typed byte into the /resume sniffer. Printables accumulate (a
/// small rolling tail), backspace edits, Esc/control clears, and Enter checks:
/// a line starting `/res` means the user ran the child's own resume picker.
/// Navigation keys (arrow sequences) deliberately don't clear the tail — the
/// picker is driven with arrows between typing and Enter.
fn sniff_resume(tail: &mut Vec<u8>, resumed_away: &mut bool, b: u8) {
    match b {
        0x0d | 0x0a => {
            if tail.starts_with(b"/res") {
                *resumed_away = true;
            }
            tail.clear();
        }
        0x7f | 0x08 => {
            tail.pop();
        }
        0x20..=0x7e => {
            if tail.len() < 64 {
                tail.push(b);
            }
        }
        _ => tail.clear(),
    }
}

/// Leave the alt-screen control room: the terminal restores the child's
/// primary screen, then anything the child said while the view was open is
/// replayed so no output is lost. No-op when the view wasn't on the alt screen.
fn leave_view(out: &mut impl Write, view_alt: &mut bool, view_buf: &mut Vec<u8>) {
    if !*view_alt {
        return;
    }
    let _ = out.write_all(b"\x1b[?1049l");
    if !view_buf.is_empty() {
        let _ = out.write_all(view_buf);
        view_buf.clear();
    }
    let _ = out.flush();
    *view_alt = false;
}

fn force_repaint(session: &Session, cols: u16, rows: u16) {
    let _ = session.master.resize(PtySize {
        rows: rows.saturating_sub(1).max(1),
        cols,
        pixel_width: 0,
        pixel_height: 0,
    });
    let _ = session.master.resize(PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    });
}

/// Tab-completion for the `:` command line. Returns the completed buffer.
fn complete_command(buf: &str, current_name: Option<&str>) -> Option<String> {
    const VERBS: [&str; 5] = ["switch", "new", "rename", "quit", "fork"];
    const RUNTIMES: [&str; 4] = ["claude", "codex", "opencode", "gemini"];
    let ends_space = buf.ends_with(' ');
    let words: Vec<&str> = buf.split_whitespace().collect();
    match (words.len(), ends_space) {
        // completing the verb
        (1, false) => {
            let matches: Vec<&str> = VERBS
                .iter()
                .filter(|v| v.starts_with(words[0]) && **v != words[0])
                .copied()
                .collect();
            (matches.len() == 1).then(|| format!("{} ", matches[0]))
        }
        // `rename ` → prefill the current name for editing
        (1, true) if words[0] == "rename" => {
            current_name.map(|n| format!("rename {n}"))
        }
        // completing the runtime after switch/new/fork
        (1, true) => None,
        (2, false) if matches!(words[0], "switch" | "new" | "fork" | "s" | "n") => {
            let matches: Vec<&str> = RUNTIMES
                .iter()
                .filter(|r| r.starts_with(words[1]) && **r != words[1])
                .copied()
                .collect();
            (matches.len() == 1).then(|| format!("{} {}", words[0], matches[0]))
        }
        _ => None,
    }
}

// ---- status bar ---------------------------------------------------------
//
// Constant stays a pass-through proxy, NOT a compositor. The bar works by
// telling the child PTY the terminal is ONE ROW SHORTER — a full-screen TUI
// then never addresses the real last row — and protecting that row from
// inline scrolling with a DECSTBM scroll region. The bar itself is repainted
// only when the child has been idle for a beat, so we never inject escape
// sequences (and clobber a saved-cursor slot) while the child is mid-paint.

/// Below this the bar costs more than it gives.
fn bar_fits(rows: u16) -> bool {
    rows >= 4
}

/// Rows the CHILD believes the terminal has.
fn child_rows(rows: u16, bar: bool) -> u16 {
    if bar && bar_fits(rows) {
        rows - 1
    } else {
        rows
    }
}

/// Compose the bar line, truncated and padded to exactly `cols` columns.
fn bar_text(
    runtime: Runtime,
    with_tools: bool,
    warn: bool,
    trail_n: u32,
    slug: Option<&str>,
    prefix_label: &str,
    cols: u16,
) -> String {
    let tools = if with_tools { "+tools" } else { "" };
    let thread = match slug {
        Some(s) if trail_n > 0 => format!("ch{trail_n:02}\u{b7}{s}"),
        Some(s) => s.to_string(),
        None => "no thread yet".to_string(),
    };
    let warn_mark = if warn { "\u{26a0}" } else { "" };
    let full = format!(
        " constant \u{b7} {}{tools}{warn_mark} \u{b7} {thread} \u{b7} {prefix_label} c/x=switch d=quit ",
        runtime.label()
    );
    let width = cols as usize;
    let mut text: String = full.chars().take(width).collect();
    let pad = width.saturating_sub(text.chars().count());
    text.extend(std::iter::repeat_n(' ', pad));
    text
}

/// Paint the bar on the real last row: re-establish the scroll region (the
/// child may have reset it), draw inverse-video, put the cursor back.
fn draw_bar(out: &mut impl Write, rows: u16, text: &str) {
    let top = rows - 1;
    let _ = write!(out, "\x1b7\x1b[1;{top}r\x1b[{rows};1H\x1b[7m{text}\x1b[0m\x1b8");
    let _ = out.flush();
}

/// Redraw the bar from current state (no-op when disabled or terminal too small).
#[allow(clippy::too_many_arguments)]
fn refresh_bar(
    out: &mut impl Write,
    enabled: bool,
    runtime: Runtime,
    with_tools: bool,
    warn: bool,
    trail_n: u32,
    slug: Option<&str>,
    prefix_label: &str,
    fallback: (u16, u16),
) {
    if !enabled {
        return;
    }
    let (c, r) = size().unwrap_or(fallback);
    if !bar_fits(r) {
        return;
    }
    draw_bar(
        out,
        r,
        &bar_text(runtime, with_tools, warn, trail_n, slug, prefix_label, c),
    );
}

/// Entry point for `constant host [runtime] [--prefix ...]` and
/// `constant resume` (which passes the projection id to wake up — the child's
/// identity is then declared from birth, no detection needed).
#[allow(clippy::too_many_arguments)]
pub fn run(
    initial: Runtime,
    resume: Option<&str>,
    with_tools: bool,
    bar: bool,
    paged: bool,
    prefix: u8,
    prefix_label: String,
) -> Result<()> {
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

    // Colored runtime letters survive inside the inverse-video overlay.
    let (cc, cx, co) = (
        runtime_color("claude"),
        runtime_color("codex"),
        runtime_color("opencode"),
    );
    let prefix_hint = format!(
        " {prefix_label} ▸  c={cc}claude\x1b[39m   x={cx}codex\x1b[39m   o={co}opencode\x1b[39m   (shift=new)   t=trail   :=command   d=quit "
    );

    let mut out = std::io::stdout();

    let mut dbg: Option<std::fs::File> = if std::env::var("CONSTANT_DEBUG").is_ok() {
        let path = std::env::temp_dir().join(format!("constant-debug-{}.log", std::process::id()));
        std::fs::File::create(path).ok()
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
    // The conversation's naming (stable handle + glance title), refreshed at
    // every switch and on :rename.
    let mut naming: Option<crate::trail::Naming> = None;
    // Declared identity: the session id we POSITIVELY know the child owns —
    // the id we resumed into it, or the one we minted at a fresh claude launch.
    // Source resolution prefers this over filesystem inference.
    let mut child_session: Option<String>;
    // Spawn-time fence for detection: only sessions touched at/after this
    // instant can be adopted as a carry seed.
    let mut child_spawned_at = std::time::SystemTime::now();
    // Settlement watchdog: a child that exits within 2s of a spawn almost
    // certainly rejected its session (or isn't installed) — recover once
    // instead of silently tearing the harness down.
    let mut watchdog = std::time::Instant::now();
    let mut respawned_once = false;
    let mut bar_dirty = bar;
    // Switch theater: the carry summary held in the bar for a few seconds
    // after each switch, so the receipt is felt, not just flashed.
    let mut bar_notice: Option<(String, std::time::Instant)> = None;
    // Child alternate-screen tracking + the control room's own alt-screen use:
    // an inline-painting child can't repaint the primary screen after the view
    // closes, so the view goes on the ALT screen and the terminal restores it.
    let mut child_alt_active = false;
    let mut child_alt_tail: Vec<u8> = Vec::new();
    let mut view_alt = false;
    let mut view_buf: Vec<u8> = Vec::new();
    // Installed-version preflight results (filled by a background sweep).
    let mut versions: HashMap<Runtime, (String, bool)> = HashMap::new();
    // `:` line history (up/down recall).
    let mut cmd_history: Vec<String> = Vec::new();
    let mut hist_ix: Option<usize> = None;
    // The command line was opened from the control room: repaint the child
    // when it closes (the graph is still on screen behind it).
    let mut from_view = false;
    let mut session = match resume {
        Some(id) => {
            child_session = Some(id.to_string());
            spawn_session(
                initial,
                SpawnMode::Resume(id),
                None,
                cols,
                child_rows(rows, bar),
                tx.clone(),
            )?
        }
        None => {
            let (s, declared) =
                spawn_fresh(initial, None, cols, child_rows(rows, bar), tx.clone())?;
            child_session = declared;
            s
        }
    };
    banner(&mut out, session.runtime, &prefix_label);
    // The claude-code-style "update is ready" line at startup: read from the
    // LAST check's cache — instant, offline-safe, never blocks the launch.
    // (The background sweep below refreshes the cache, at most ~once a day.)
    if let Some(latest) = crate::alembic::cached_release_version()
        && crate::alembic::version_newer(&latest, env!("CARGO_PKG_VERSION"))
    {
        dim(
            &mut out,
            &format!(
                "constant v{latest} is available — brew upgrade kennykankush/constant/constant"
            ),
        );
    }

    let mut mode = M_NORMAL;
    let mut cmd_buf = String::new();
    let mut switching_to: Option<SwitchRequest> = None;
    // Set once the pending switch has actually terminated the child.
    let mut term_sent = false;
    // What the user is typing into the child, watched for a `/res…` command:
    // on codex 0.139 a /resume-away leaves NO trace on disk until the user
    // talks in the resumed conversation, so the flag powers an honest warning.
    let mut typed_tail: Vec<u8> = Vec::new();
    let mut resumed_away = false;
    let mut pending_out: Vec<u8> = Vec::new();
    // Installed-version preflight, off the hot path (each `--version` is a
    // subprocess). An unvalidated runtime gets a warning in the bar the moment
    // the sweep lands — doctor's quiet knowledge, surfaced where it matters.
    {
        let tx = tx.clone();
        thread::spawn(move || {
            let v: Vec<(Runtime, String, bool)> = [
                Runtime::Codex,
                Runtime::Claude,
                Runtime::OpenCode,
                Runtime::Gemini,
            ]
            .into_iter()
            .filter_map(|rt| crate::alembic::version_status(rt).map(|(ver, ok)| (rt, ver, ok)))
            .collect();
            // Refresh the release cache (live at most ~once a day, else the
            // cached answer) so the startup line and drift notices stay true.
            let update = crate::alembic::latest_release_refreshed()
                .filter(|l| crate::alembic::version_newer(l, env!("CARGO_PKG_VERSION")));
            let _ = tx.send(Ev::Versions(v, update));
        });
    }

    let mut quitting = false;

    loop {
        let ev = match rx.recv_timeout(std::time::Duration::from_millis(120)) {
            Ok(ev) => ev,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                // Theater notice expiry: hand the bar back to the standard text.
                if let Some((_, since)) = &bar_notice
                    && since.elapsed() > std::time::Duration::from_secs(6)
                {
                    bar_notice = None;
                    bar_dirty = bar;
                }
                // The child has been quiet for a beat: safe to repaint the bar
                // (never inject while it might be mid-paint).
                if bar_dirty && mode == M_NORMAL {
                    if let Some((notice, _)) = &bar_notice {
                        let (c, r) = size().unwrap_or((cols, rows));
                        if bar && bar_fits(r) {
                            let width = c as usize;
                            let mut text: String = notice.chars().take(width).collect();
                            let pad = width.saturating_sub(text.chars().count());
                            text.extend(std::iter::repeat_n(' ', pad));
                            draw_bar(&mut out, r, &text);
                        }
                    } else {
                        refresh_bar(
                            &mut out,
                            bar,
                            session.runtime,
                            with_tools,
                    versions.get(&session.runtime).map(|v| !v.1).unwrap_or(false),
                            trail_n,
                            naming.as_ref().map(|nm| nm.name.as_str()).or(root_slug.as_deref()),
                            &prefix_label,
                            (cols, rows),
                        );
                    }
                    bar_dirty = false;
                }
                continue;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        };
        match ev {
            Ev::Pty(bytes) => {
                track_alt(&mut child_alt_active, &mut child_alt_tail, &bytes);
                bar_dirty = bar;
                if mode == M_VIEW {
                    if view_alt {
                        // Inline child: hold its output and replay it after the
                        // alt-screen restore, so nothing is lost while viewing.
                        if view_buf.len() < VIEW_BUF_CAP {
                            view_buf.extend_from_slice(&bytes);
                        }
                    }
                    // A full-screen (alt-screen) child instead repaints itself
                    // on exit via the resize wiggle; buffering would duplicate.
                    continue;
                }
                if mode == M_COMMAND {
                    // Bounded: a chatty child while the user sits in the command
                    // line must not grow memory forever. Past the cap, flush
                    // through and redraw the prompt (a flicker beats a leak).
                    const MAX_PENDING: usize = 1 << 20;
                    if pending_out.len() + bytes.len() > MAX_PENDING {
                        let _ = out.write_all(&pending_out);
                        pending_out.clear();
                        let _ = out.write_all(&bytes);
                        let _ = out.flush();
                        bottom_overlay(&mut out, &format!(" constant ▸ {cmd_buf}"));
                    } else {
                        pending_out.extend_from_slice(&bytes);
                    }
                } else {
                    let _ = out.write_all(&bytes);
                    let _ = out.flush();
                }
            }

            Ev::Versions(list, update) => {
                for (rt, ver, ok) in list {
                    versions.insert(rt, (ver, ok));
                }
                if let Some((ver, false)) = versions
                    .get(&session.runtime)
                    .map(|(v, ok)| (v.clone(), *ok))
                {
                    let fix = match &update {
                        Some(u) => {
                            format!(" \u{b7} constant v{u} is out \u{2014} brew upgrade")
                        }
                        None => " \u{b7} carries may misbehave".to_string(),
                    };
                    bar_notice = Some((
                        format!(
                            " \u{26a0} {} {ver} unvalidated (codec validated against {}.x){fix} ",
                            session.runtime.label(),
                            supported_line(session.runtime),
                        ),
                        std::time::Instant::now(),
                    ));
                    bar_dirty = bar;
                } else if let Some(u) = &update {
                    bar_notice = Some((
                        format!(
                            " \u{2191} constant v{u} is available \u{b7} brew upgrade kennykankush/constant/constant "
                        ),
                        std::time::Instant::now(),
                    ));
                    bar_dirty = bar;
                }
            }
            Ev::PtyClosed => {
                if let Some(request) = switching_to.take() {
                    let target = request.target;
                    let from = session.runtime;
                    let _ = session.child.wait();
                    let (c, r) = size().unwrap_or((cols, rows));
                    let r = child_rows(r, bar);
                    write_reset(&mut out); // undo outgoing child's terminal modes
                    let _ = out.write_all(b"\x1b[2J\x1b[H");

                    // Read source for `from`, strongest evidence first:
                    //  1. DECLARED identity — the session id we positively know
                    //     the child owns (resumed id, or minted at launch).
                    //     Resolved by id with newest-file-wins, so codex's
                    //     rewrite-on-resume is followed automatically.
                    //  2. The tracked projection of the established pair.
                    //  3. cwd- AND spawn-time-fenced detection (the seed on a
                    //     first switch). The fence stops an older or concurrent
                    //     same-cwd session from being adopted; a `/resume` away
                    //     inside the child is picked up here when the declared
                    //     session never got a conversation of its own.
                    let declared = child_session
                        .as_deref()
                        .and_then(|id| crate::alembic::session_by_id(from, id));
                    let src = declared.or_else(|| {
                        let active = crate::alembic::active_session(
                            from,
                            host_cwd.as_deref(),
                            Some(child_spawned_at),
                        );
                        let tracked = owned
                            .get(&from)
                            .and_then(|(id, p)| p.exists().then(|| (p.clone(), id.clone())));
                        match (tracked, active) {
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
                        }
                    });

                    // Load + distill the source ONCE; naming and the carry share it.
                    let distilled = src.as_ref().map(|(src_path, _)| {
                        crate::alembic::distill_source(src_path, with_tools)
                    });

                    let spawned = match (src, distilled) {
                        (Some((src_path, src_id)), Some(Ok(mut distilled))) => {
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
                                let name = distilled
                                    .root_name()
                                    .unwrap_or_else(|| "conversation".to_string());
                                root_slug = Some(crate::trail::slug(&name));
                            }
                            let conv_id = cid;
                            let slug = root_slug.clone().unwrap_or_default();
                            // Naming: stable handle (registry-backed) + glance
                            // title (rename > harvested > birth slug), refreshed
                            // every switch so harvests and renames land.
                            let harvested =
                                crate::alembic::harvested_title(&distilled.session);
                            let nm = crate::trail::naming_for(
                                &conv_id,
                                &slug,
                                harvested.as_deref(),
                            );
                            naming = Some(nm.clone());
                            // Candidate chapter number; committed only on a
                            // successful carry, so a failed carry consumes none.
                            let n = trail_n + 1;
                            let title = crate::trail::title(n, from, &nm.name, &nm.handle);

                            let action = if request.new { "new" } else { "continue" };
                            let (cf, ct) = (
                                runtime_color(from.label()),
                                runtime_color(target.label()),
                            );
                            let _ = out.write_all(
                                format!(
                                    "\x1b[2m  {} · {} · ch{n:02} \x1b[0m{cf}{}\x1b[0m\x1b[2m \u{2192} \x1b[0m{ct}{}\x1b[0m\x1b[2m · {action} · {}\x1b[0m\r\n",
                                    nm.handle,
                                    nm.name,
                                    from.label(),
                                    target.label(),
                                    distilled.receipt.summary(),
                                )
                                .as_bytes(),
                            );
                            let _ = out.flush();

                            // The record comes first: write this hop's snapshot
                            // volume (distilled IR) BEFORE materializing any native
                            // copy. If the record can't be written the switch still
                            // proceeds — but the gap is announced, never silent.
                            let snapshot = crate::trail::snapshot_path(&conv_id, n, from)
                                .and_then(|p| {
                                    match crate::alembic::write_snapshot(&distilled.session, &p) {
                                        Ok(()) => Some(p),
                                        Err(e) => {
                                            dim(&mut out, &format!("record not written — {e}"));
                                            None
                                        }
                                    }
                                });

                            if paged {
                                // The desk layout: the record above holds the FULL
                                // thread; the projection wakes the target on head
                                // card + index + verbatim tail. Index addresses
                                // resolve via `constant recall` into that record.
                                let anchor = crate::alembic::render::git_anchor(
                                    host_cwd.as_deref(),
                                );
                                let stats = crate::alembic::render::render_paged(
                                    &mut distilled.session,
                                    &nm.handle,
                                    &nm.name,
                                    n,
                                    from.label(),
                                    target.label(),
                                    anchor.as_deref(),
                                    crate::alembic::render::TAIL_BUDGET_CHARS,
                                );
                                distilled.receipt.indexed = stats.indexed;
                            }

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
                            match crate::alembic::distill_write(
                                &mut distilled,
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
                                    if let Err(e) = crate::trail::record(&crate::trail::CarryRow {
                                        n,
                                        conv_id: &conv_id,
                                        slug: &slug,
                                        cwd: host_cwd.as_deref(),
                                        source_id: &src_id,
                                        source_path: &src_path,
                                        from,
                                        to: target,
                                        id: &id,
                                        path: &written,
                                        title: &title,
                                        mode: if reuse_owned.is_some() {
                                            "refresh-existing"
                                        } else {
                                            "new-fork"
                                        },
                                        snapshot: snapshot.as_deref(),
                                        handle: &nm.handle,
                                        name: &nm.name,
                                        named: nm.named,
                                    }) {
                                        // The carry is fine; the LEDGER diverged —
                                        // say so, or the next re-host silently forks.
                                        dim(&mut out, &format!("trail ledger write failed: {e}"));
                                    }
                                    // A carry of nothing is technically a success
                                    // and humanly a failure — make it look like one.
                                    let sym = if distilled.receipt.turns <= 1 {
                                        "\u{26a0}"
                                    } else {
                                        "\u{2713}"
                                    };
                                    bar_notice = Some((
                                        format!(
                                            " {sym} ch{n:02} \u{2192} {} \u{b7} {} \u{b7} {} ",
                                            target.label(),
                                            nm.name,
                                            distilled.receipt.summary(),
                                        ),
                                        std::time::Instant::now(),
                                    ));
                                    child_spawned_at = std::time::SystemTime::now();
                                    spawn_settled(
                                        target,
                                        Some(&id),
                                        from,
                                        Some(&src_id),
                                        session_cwd.as_deref(),
                                        c,
                                        r,
                                        &tx,
                                        &mut out,
                                    )?
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
                                    dim(&mut out, &format!("couldn't carry — {e}; starting fresh"));
                                    bar_notice = Some((
                                        format!(
                                            " \u{26a0} couldn't carry \u{b7} {} started fresh ",
                                            target.label()
                                        ),
                                        std::time::Instant::now(),
                                    ));
                                    child_spawned_at = std::time::SystemTime::now();
                                    spawn_settled(
                                        target,
                                        None,
                                        from,
                                        Some(&src_id),
                                        None,
                                        c,
                                        r,
                                        &tx,
                                        &mut out,
                                    )?
                                }
                            }
                        }
                        (Some((_, src_id)), Some(Err(e))) => {
                            // The source exists but won't load (corrupt beyond the
                            // tolerated torn tail). Don't kill recovered state we
                            // might still need; start the target fresh.
                            dim(&mut out, &format!("couldn't read the conversation — {e}; starting fresh"));
                            bar_notice = Some((
                                format!(
                                    " \u{26a0} couldn't read the conversation \u{b7} {} started fresh ",
                                    target.label()
                                ),
                                std::time::Instant::now(),
                            ));
                            child_spawned_at = std::time::SystemTime::now();
                            spawn_settled(target, None, from, Some(&src_id), None, c, r, &tx, &mut out)?
                        }
                        _ => {
                            dim(&mut out, "no conversation here to carry; starting fresh");
                            bar_notice = Some((
                                format!(
                                    " \u{26a0} nothing to carry \u{b7} {} started fresh ",
                                    target.label()
                                ),
                                std::time::Instant::now(),
                            ));
                            child_spawned_at = std::time::SystemTime::now();
                            spawn_settled(target, None, from, None, None, c, r, &tx, &mut out)?
                        }
                    };
                    session = spawned.0;
                    child_session = spawned.1;
                    watchdog = std::time::Instant::now();
                    respawned_once = false;
                    term_sent = false;
                    resumed_away = false;
                    typed_tail.clear();
                    banner(&mut out, session.runtime, &prefix_label);
                    // Establish the protected bar row BEFORE the child starts
                    // writing (an inline-scrolling child must never reach it).
                    refresh_bar(
                        &mut out,
                        bar,
                        session.runtime,
                        with_tools,
                    versions.get(&session.runtime).map(|v| !v.1).unwrap_or(false),
                        trail_n,
                        naming.as_ref().map(|nm| nm.name.as_str()).or(root_slug.as_deref()),
                        &prefix_label,
                        (cols, rows),
                    );
                    bar_dirty = bar_notice.is_some() && bar;
                } else if watchdog.elapsed() < std::time::Duration::from_secs(2)
                    && !respawned_once
                {
                    // Settlement check: the child died almost immediately after a
                    // spawn — a rejected resume or a missing/broken binary, not a
                    // user quitting. Recover once with a fresh launch (the carry,
                    // if any, is safe in the trail) instead of exiting silently.
                    respawned_once = true;
                    let rt = session.runtime;
                    let _ = session.child.wait();
                    write_reset(&mut out);
                    dim(
                        &mut out,
                        &format!(
                            "{} exited immediately — restarting it fresh (any carried session stays in `constant trail`)",
                            rt.label()
                        ),
                    );
                    let (c, r) = size().unwrap_or((cols, rows));
                    child_spawned_at = std::time::SystemTime::now();
                    match spawn_fresh(rt, None, c, child_rows(r, bar), tx.clone()) {
                        Ok((s, declared)) => {
                            session = s;
                            child_session = declared;
                            watchdog = std::time::Instant::now();
                            banner(&mut out, session.runtime, &prefix_label);
                            refresh_bar(
                                &mut out,
                                bar,
                                session.runtime,
                                with_tools,
                    versions.get(&session.runtime).map(|v| !v.1).unwrap_or(false),
                                trail_n,
                                naming.as_ref().map(|nm| nm.name.as_str()).or(root_slug.as_deref()),
                                &prefix_label,
                                (cols, rows),
                            );
                            bar_dirty = false;
                        }
                        Err(_) => break,
                    }
                } else {
                    break;
                }
            }

            Ev::Resize => {
                let (c, r) = size().unwrap_or((cols, rows));
                let _ = session.master.resize(PtySize {
                    rows: child_rows(r, bar),
                    cols: c,
                    pixel_width: 0,
                    pixel_height: 0,
                });
                refresh_bar(
                    &mut out,
                    bar,
                    session.runtime,
                    with_tools,
                    versions.get(&session.runtime).map(|v| !v.1).unwrap_or(false),
                    trail_n,
                    naming.as_ref().map(|nm| nm.name.as_str()).or(root_slug.as_deref()),
                    &prefix_label,
                    (cols, rows),
                );
                bar_dirty = false;
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
                            Token::Byte(b) => {
                                sniff_resume(&mut typed_tail, &mut resumed_away, b);
                                passthrough.push(b);
                            }
                            Token::Seq(s) => {
                                if let Some((cp, mods, ev)) = parse_kitty_u(&s)
                                    && ev != 3
                                    && mods <= 2
                                    && let Ok(b) = u8::try_from(cp)
                                {
                                    sniff_resume(&mut typed_tail, &mut resumed_away, b);
                                }
                                passthrough.extend_from_slice(&s);
                            }
                        },

                        M_PREFIX => {
                            clear_bottom(&mut out);
                            mode = M_NORMAL;
                            bar_dirty = bar; // the hint borrowed the bar's row
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
                                    Some(b'o') => request_switch(
                                        Runtime::OpenCode,
                                        false,
                                        &mut session,
                                        &mut switching_to,
                                    ),
                                    Some(b'O') => request_switch(
                                        Runtime::OpenCode,
                                        true,
                                        &mut session,
                                        &mut switching_to,
                                    ),
                                    Some(b'g') | Some(b'G') => dim(
                                        &mut out,
                                        "gemini isn't a switch target yet — it works as a carry source (writer pending one live-format check)",
                                    ),
                                    Some(b't') => {
                                        mode = M_VIEW;
                                        view_alt = !child_alt_active;
                                        if view_alt {
                                            view_buf.clear();
                                            let _ = out.write_all(b"\x1b[?1049h");
                                        }
                                        let (_, r) = size().unwrap_or((cols, rows));
                                        let chapters = root_id
                                            .as_deref()
                                            .map(crate::trail::chapters)
                                            .unwrap_or_default();
                                        let now = std::time::SystemTime::now()
                                            .duration_since(std::time::UNIX_EPOCH)
                                            .map(|d| d.as_secs())
                                            .unwrap_or(0);
                                        let view = render_graph(
                                            naming.as_ref(),
                                            &chapters,
                                            session.runtime.label(),
                                            r,
                                            now,
                                        );
                                        let _ = out.write_all(view.as_bytes());
                                        let _ = out.flush();
                                    }
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
                                        hist_ix = None;
                                        bottom_overlay(&mut out, &format!(" constant ▸ {cmd_buf}"));
                                    }
                                    // Tab: complete verbs/runtimes; `rename `
                                    // prefills the current name for editing.
                                    0x09 => {
                                        if let Some(completed) = complete_command(
                                            &cmd_buf,
                                            naming.as_ref().map(|nm| nm.name.as_str()),
                                        ) {
                                            cmd_buf = completed;
                                            bottom_overlay(
                                                &mut out,
                                                &format!(" constant ▸ {cmd_buf}"),
                                            );
                                        }
                                    }
                                    _ => {
                                        if b == b' ' || b.is_ascii_graphic() {
                                            cmd_buf.push(b as char);
                                            hist_ix = None;
                                            bottom_overlay(
                                                &mut out,
                                                &format!(" constant ▸ {cmd_buf}"),
                                            );
                                        }
                                    }
                                },
                                Token::Seq(s) => {
                                    // Arrow recall: ↑/↓ walk the command history
                                    // (legacy ESC[A/B, SS3 ESCOA/OB, and kitty all
                                    // end in A/B).
                                    let last = s.last().copied();
                                    if last == Some(b'A') && !cmd_history.is_empty() {
                                        let ix = match hist_ix {
                                            None => cmd_history.len() - 1,
                                            Some(0) => 0,
                                            Some(i) => i - 1,
                                        };
                                        hist_ix = Some(ix);
                                        cmd_buf = cmd_history[ix].clone();
                                        bottom_overlay(
                                            &mut out,
                                            &format!(" constant ▸ {cmd_buf}"),
                                        );
                                    } else if last == Some(b'B') {
                                        match hist_ix {
                                            Some(i) if i + 1 < cmd_history.len() => {
                                                hist_ix = Some(i + 1);
                                                cmd_buf = cmd_history[i + 1].clone();
                                            }
                                            Some(_) => {
                                                hist_ix = None;
                                                cmd_buf.clear();
                                            }
                                            None => {}
                                        }
                                        bottom_overlay(
                                            &mut out,
                                            &format!(" constant ▸ {cmd_buf}"),
                                        );
                                    } else if let Some((cp, _, event)) = parse_kitty_u(&s)
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
                                bar_dirty = bar;
                                if from_view {
                                    from_view = false;
                                    if view_alt {
                                        leave_view(&mut out, &mut view_alt, &mut view_buf);
                                    } else {
                                        let _ = out.write_all(b"\x1b[2J\x1b[H");
                                        let _ = out.flush();
                                        let (c, r) = size().unwrap_or((cols, rows));
                                        force_repaint(&session, c, child_rows(r, bar));
                                    }
                                }
                                if !pending_out.is_empty() {
                                    let _ = out.write_all(&pending_out);
                                    let _ = out.flush();
                                    pending_out.clear();
                                }
                                if submit {
                                    let line = cmd_buf.trim().to_string();
                                    if !line.is_empty()
                                        && cmd_history.last() != Some(&line)
                                    {
                                        cmd_history.push(line);
                                    }
                                    hist_ix = None;
                                    match execute_command(
                                        cmd_buf.trim(),
                                        &mut session,
                                        &mut switching_to,
                                    ) {
                                        Action::Quit => quitting = true,
                                        Action::Rename(new_name) => {
                                            match (&root_id, &mut naming) {
                                                (Some(conv), Some(nm)) => {
                                                    nm.name = new_name.clone();
                                                    nm.named = true;
                                                    if let Err(e) = crate::trail::record_rename(
                                                        conv,
                                                        &nm.handle,
                                                        &new_name,
                                                        host_cwd.as_deref(),
                                                    ) {
                                                        dim(&mut out, &format!("rename not recorded: {e}"));
                                                    } else {
                                                        // Re-stamp live projections so the
                                                        // new name shows in native pickers.
                                                        let stamp =
                                                            format!("{new_name} · {}", nm.handle);
                                                        for (rt, (id, path)) in owned.iter() {
                                                            let _ = crate::alembic::restamp_title(
                                                                *rt, id, path, &stamp,
                                                            );
                                                        }
                                                        dim(
                                                            &mut out,
                                                            &format!(
                                                                "renamed: {} · {new_name}",
                                                                nm.handle
                                                            ),
                                                        );
                                                        bar_dirty = bar;
                                                    }
                                                }
                                                _ => dim(
                                                    &mut out,
                                                    "nothing to rename yet — the conversation gets its name at the first switch",
                                                ),
                                            }
                                        }
                                        Action::None => {}
                                    }
                                }
                            }
                        }

                        M_VIEW => {
                            // The cockpit: act from inside the graph. Letter
                            // keys arrive as bytes or kitty presses — normalize.
                            let key: Option<u8> = match &tok {
                                Token::Byte(b) => Some(*b),
                                Token::Seq(seq) => match parse_kitty_u(seq) {
                                    Some((cp, mods, ev)) if ev != 3 && mods <= 2 => {
                                        u8::try_from(cp).ok()
                                    }
                                    Some(_) => None,
                                    // any unrecognized escape (incl. bare Esc) closes
                                    None => Some(b'q'),
                                },
                                Token::Prefix => None,
                            };
                            let mut close = false;
                            // Switch straight from the graph: leave the view
                            // first (restoring the child's screen), then the
                            // switch flow owns the screen from there.
                            let switch_to: Option<(Runtime, bool)> = match key {
                                Some(b'c') => Some((Runtime::Claude, false)),
                                Some(b'C') => Some((Runtime::Claude, true)),
                                Some(b'x') => Some((Runtime::Codex, false)),
                                Some(b'X') => Some((Runtime::Codex, true)),
                                Some(b'o') => Some((Runtime::OpenCode, false)),
                                Some(b'O') => Some((Runtime::OpenCode, true)),
                                _ => None,
                            };
                            match key {
                                Some(b't') | Some(b'q') | Some(0x0d) | Some(0x0a)
                                | Some(b' ') | Some(0x03) | Some(27) => close = true,
                                // Rename without leaving: command line over the
                                // graph, prefilled with the current name.
                                Some(b'r') => {
                                    mode = M_COMMAND;
                                    from_view = true;
                                    cmd_buf = match naming.as_ref() {
                                        Some(nm) => format!("rename {}", nm.name),
                                        None => "rename ".to_string(),
                                    };
                                    bottom_overlay(&mut out, &format!(" constant ▸ {cmd_buf}"));
                                }
                                _ => {}
                            }
                            if let Some((rt, fork)) = switch_to {
                                leave_view(&mut out, &mut view_alt, &mut view_buf);
                                mode = M_NORMAL;
                                request_switch(rt, fork, &mut session, &mut switching_to);
                            }
                            if close {
                                mode = M_NORMAL;
                                if view_alt {
                                    leave_view(&mut out, &mut view_alt, &mut view_buf);
                                } else {
                                    let _ = out.write_all(b"\x1b[2J\x1b[H");
                                    let _ = out.flush();
                                    let (c, r) = size().unwrap_or((cols, rows));
                                    force_repaint(&session, c, child_rows(r, bar));
                                }
                                bar_dirty = bar;
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
                // Carry gate: a CONTINUE switch with nothing to carry must not
                // tear the child down just to land in an empty target — cancel
                // loudly and leave the child running. Uppercase/new switches
                // always proceed (fresh is what they mean).
                if let Some(req) = &switching_to
                    && !term_sent
                {
                    let proceed = req.new || {
                        let declared = child_session
                            .as_deref()
                            .and_then(|id| crate::alembic::session_by_id(session.runtime, id))
                            .is_some();
                        declared
                            || owned
                                .get(&session.runtime)
                                .map(|(_, p)| p.exists())
                                .unwrap_or(false)
                            || crate::alembic::active_session(
                                session.runtime,
                                host_cwd.as_deref(),
                                Some(child_spawned_at),
                            )
                            .is_some()
                    };
                    if proceed {
                        terminate(&mut session.child);
                        term_sent = true;
                    } else {
                        let msg = if resumed_away {
                            " \u{26a0} can't carry: a /resume inside the child is invisible \u{b7} say one thing in that conversation, then switch (C/X/O = fresh) "
                        } else {
                            " \u{26a0} nothing to carry yet \u{b7} talk first, or C/X/O for a fresh start "
                        };
                        bar_notice = Some((msg.to_string(), std::time::Instant::now()));
                        bar_dirty = bar;
                        switching_to = None;
                    }
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
    fn relative_time_buckets() {
        let now = 1_000_000u64;
        assert_eq!(relative_time(now - 30, now), "just now");
        assert_eq!(relative_time(now - 120, now), "2m ago");
        assert_eq!(relative_time(now - 7200, now), "2h ago");
        assert_eq!(relative_time(now - 100_000, now), "yesterday");
        assert_eq!(relative_time(now - 300_000, now), "3d ago");
        assert_eq!(relative_time(0, now), "");
    }

    #[test]
    fn graph_renders_chapters_with_head_and_truncation() {
        let nm = crate::trail::Naming {
            handle: "cobalt-37".to_string(),
            name: "auth redirect bug".to_string(),
            named: true,
        };
        let chapters: Vec<crate::trail::ChapterRow> = (1..=4)
            .map(|n| crate::trail::ChapterRow {
                n,
                from: if n % 2 == 1 { "codex" } else { "claude" }.to_string(),
                to: if n % 2 == 1 { "claude" } else { "codex" }.to_string(),
                ts: 1_000_000 + n as u64,
                mode: if n == 3 { "new-fork" } else { "refresh-existing" }.to_string(),
                recorded: n > 1,
            })
            .collect();
        let view = render_graph(Some(&nm), &chapters, "codex", 40, 2_000_000);
        assert!(view.contains("cobalt-37"), "{view}");
        assert!(view.contains("auth redirect bug"));
        assert!(view.contains("ch05"), "head chapter missing: {view}");
        assert!(view.contains("you are here"));
        assert!(view.contains("ch01") && view.contains("ch04"));
        assert!(view.contains("rec \u{2713}"), "record marker missing");

        // Tiny terminal: old chapters truncate with an honest count.
        let small = render_graph(Some(&nm), &chapters, "codex", 10, 2_000_000);
        assert!(small.contains("earlier chapters"), "{small}");

        // No thread yet: says so instead of pretending.
        let empty = render_graph(None, &[], "codex", 40, 2_000_000);
        assert!(empty.contains("no thread yet"));
    }

    #[test]
    fn command_completion_verbs_runtimes_and_rename_prefill() {
        assert_eq!(complete_command("sw", None), Some("switch ".to_string()));
        assert_eq!(complete_command("s", None), Some("switch ".to_string()));
        assert_eq!(
            complete_command("switch cl", None),
            Some("switch claude".to_string())
        );
        assert_eq!(
            complete_command("new o", None),
            Some("new opencode".to_string())
        );
        // `rename ` prefills the current name for editing.
        assert_eq!(
            complete_command("rename ", Some("auth redirect bug")),
            Some("rename auth redirect bug".to_string())
        );
        // Unknown: no completion.
        assert_eq!(complete_command("zz", None), None);
    }

    #[test]
    fn sniff_resume_sees_the_picker() {
        let mut tail = Vec::new();
        let mut away = false;
        for b in b"/resume\r" {
            sniff_resume(&mut tail, &mut away, *b);
        }
        assert!(away, "typed /resume + enter missed");

        // Partial command + Enter (popup completion) still counts.
        let (mut tail, mut away) = (Vec::new(), false);
        for b in b"/res\r" {
            sniff_resume(&mut tail, &mut away, *b);
        }
        assert!(away);

        // A normal chat line never trips it; Esc clears the tail.
        let (mut tail, mut away) = (Vec::new(), false);
        for b in b"please /resume nothing\r" {
            sniff_resume(&mut tail, &mut away, *b);
        }
        assert!(!away);
        let (mut tail, mut away) = (Vec::new(), false);
        for b in b"/res" {
            sniff_resume(&mut tail, &mut away, *b);
        }
        sniff_resume(&mut tail, &mut away, 0x1b);
        sniff_resume(&mut tail, &mut away, 0x0d);
        assert!(!away, "esc should clear the tail");
    }

    #[test]
    fn track_alt_survives_split_sequences() {
        let mut active = false;
        let mut tail = Vec::new();
        // The 1049h sequence arrives split across two reads.
        track_alt(&mut active, &mut tail, b"\x1b[?10");
        assert!(!active);
        track_alt(&mut active, &mut tail, b"49h\x1b[2J");
        assert!(active, "split alt-screen enter missed");
        // Leaving, also split.
        track_alt(&mut active, &mut tail, b"bye\x1b[?1049");
        assert!(active);
        track_alt(&mut active, &mut tail, b"l");
        assert!(!active, "split alt-screen leave missed");
        // Unrelated output never flips it.
        track_alt(&mut active, &mut tail, b"plain text \x1b[31mred\x1b[0m");
        assert!(!active);
    }

    #[test]
    fn bar_text_is_exactly_terminal_width() {
        // Fits: padded to width.
        let t = bar_text(Runtime::Codex, false, false, 4, Some("fix-the-bug"), "Ctrl-B", 80);
        assert_eq!(t.chars().count(), 80);
        assert!(t.contains("codex"), "{t}");
        assert!(t.contains("ch04\u{b7}fix-the-bug"), "{t}");
        // Tools mode is visible.
        let t = bar_text(Runtime::Claude, true, false, 1, Some("x"), "Ctrl-B", 80);
        assert!(t.contains("claude+tools"), "{t}");
        // An unvalidated runtime is marked right next to its name.
        let t = bar_text(Runtime::Codex, false, true, 0, None, "Ctrl-B", 80);
        assert!(t.contains("codex\u{26a0}"), "{t}");
        let t = bar_text(Runtime::Codex, false, false, 0, None, "Ctrl-B", 80);
        assert!(t.contains("no thread yet"), "{t}");
        // Narrow terminal: truncated to width, never wider.
        let t = bar_text(
            Runtime::Codex,
            false,
            false,
            12,
            Some("a-very-long-slug-here"),
            "Ctrl-B",
            20,
        );
        assert_eq!(t.chars().count(), 20);
    }

    #[test]
    fn child_rows_reserves_one_line_only_when_the_bar_fits() {
        assert_eq!(child_rows(24, true), 23);
        assert_eq!(child_rows(24, false), 24);
        // Tiny terminal: the bar steps aside.
        assert_eq!(child_rows(3, true), 3);
    }

    #[test]
    fn tokenizer_ignores_prefix_inside_bracketed_paste() {
        // Pasted content containing the raw prefix byte must NOT enter prefix
        // mode (it would fire a real switch mid-paste). After the paste closes,
        // the prefix works again.
        let mut tk = Tokenizer::new(0x02);
        let mut toks = Vec::new();
        tk.feed(b"\x1b[200~\x02c\x1b[201~", &mut toks);
        assert!(
            !toks.iter().any(|t| matches!(t, Token::Prefix)),
            "prefix fired inside a paste"
        );
        let mut after = Vec::new();
        tk.feed(&[0x02], &mut after);
        assert!(matches!(after.as_slice(), [Token::Prefix]));
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
