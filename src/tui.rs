//! Shared primitives for Constant's interactive views (the resume picker,
//! the trail explorer): the alternate-screen/raw-mode guard and the gate
//! that decides whether an interactive view may open at all.

use std::io::Write;

use anyhow::Result;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};

/// The interactive palette — every full-screen view draws with these.
pub const DIM: &str = "\x1b[2m";
pub const BOLD: &str = "\x1b[1m";
pub const RESET: &str = "\x1b[0m";
pub const INV: &str = "\x1b[7m";

/// True when both stdin and stdout are terminals — the gate every
/// interactive view shares. Piped invocations keep the printable paths,
/// so scripts and tests never meet a raw-mode screen.
pub fn interactive() -> bool {
    use std::io::IsTerminal;
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

/// Terminal (cols, rows) with sane floors — some PTYs report 0×0 before a
/// winsize is set, which would collapse every layout budget to its minimum.
pub fn dimensions() -> (usize, usize) {
    let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
    (
        if cols == 0 { 80 } else { cols as usize },
        if rows == 0 { 24 } else { rows as usize },
    )
}

/// RAII raw-mode + alt-screen guard: the shell comes back no matter how we
/// leave (including on error paths).
pub struct Screen;

impl Screen {
    pub fn enter() -> Result<Self> {
        enable_raw_mode()?;
        let mut out = std::io::stdout();
        let _ = out.write_all(b"\x1b[?1049h\x1b[?25l");
        let _ = out.flush();
        Ok(Screen)
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        let mut out = std::io::stdout();
        let _ = out.write_all(b"\x1b[?1049l\x1b[?25h");
        let _ = out.flush();
        let _ = disable_raw_mode();
    }
}
