//! The resume picker — an interactive, type-to-search session list, in the
//! spirit of the runtimes' own `/resume` pickers but across ALL of them at
//! once: every session in this directory (or everywhere), annotated with the
//! conversation handles Constant knows, names pulled from each runtime's own
//! registry where one exists.
//!
//! Draws on the ALTERNATE screen (the terminal restores the shell on exit),
//! raw mode for keys, no extra dependencies — the same primitives the host
//! already lives on.

use std::io::{IsTerminal, Write};
use std::path::PathBuf;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, size};

use crate::alembic;
use crate::runtime::Runtime;
use crate::trail;

/// One pickable row.
#[derive(Clone)]
pub struct PickEntry {
    pub runtime: Runtime,
    pub id: String,
    /// What a person knows it by: `handle · name` when the trail knows the
    /// session, else the runtime's own registry title, else a dim id.
    pub display: String,
    /// True when `display` came from the trail (rendered bold).
    pub known: bool,
    pub cwd: Option<String>,
    pub mtime_secs: u64,
}

/// Gather pickable sessions, newest first.
pub fn entries(cwd: Option<&std::path::Path>) -> Vec<PickEntry> {
    let mut out = Vec::new();
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
            let (display, known) = match trail::label_for_session(&s.id) {
                Some(l) => (l, true),
                None => match s.title.as_deref().filter(|t| !t.is_empty()) {
                    Some(t) => (t.to_string(), false),
                    None => (s.id.clone(), false),
                },
            };
            out.push(PickEntry {
                runtime: rt,
                id: s.id,
                display,
                known,
                cwd: s.cwd,
                mtime_secs,
            });
        }
    }
    out.sort_by_key(|e| std::cmp::Reverse(e.mtime_secs));
    out
}

/// Case-insensitive filter over display, id, and runtime label.
pub fn filter<'a>(entries: &'a [PickEntry], query: &str) -> Vec<&'a PickEntry> {
    let q = query.to_lowercase();
    entries
        .iter()
        .filter(|e| {
            q.is_empty()
                || e.display.to_lowercase().contains(&q)
                || e.id.to_lowercase().contains(&q)
                || e.runtime.label().contains(&q)
        })
        .collect()
}

/// RAII raw-mode + alt-screen guard: the shell comes back no matter how we
/// leave (including on error paths).
struct Screen;
impl Screen {
    fn enter() -> Result<Self> {
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

/// Run the picker. Returns the chosen entry, or None on cancel.
/// `start_cwd` seeds the [cwd] filter; Tab widens to everywhere.
pub fn pick(start_cwd: Option<PathBuf>) -> Result<Option<PickEntry>> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        anyhow::bail!("the resume picker needs an interactive terminal");
    }

    let mut scoped = start_cwd.is_some();
    let mut all_entries = entries(if scoped { start_cwd.as_deref() } else { None });
    let mut query = String::new();
    let mut selected: usize = 0;
    let mut offset: usize = 0;

    let _screen = Screen::enter()?;
    loop {
        let visible = filter(&all_entries, &query);
        if selected >= visible.len() {
            selected = visible.len().saturating_sub(1);
        }

        draw(&visible, &query, selected, &mut offset, scoped, start_cwd.as_deref())?;

        match event::read()? {
            Event::Key(k) => {
                // Only key PRESSES (kitty-capable terminals also report releases).
                if k.kind == event::KeyEventKind::Release {
                    continue;
                }
                match (k.code, k.modifiers) {
                    (KeyCode::Esc, _) => return Ok(None),
                    (KeyCode::Char('c'), KeyModifiers::CONTROL) => return Ok(None),
                    (KeyCode::Enter, _) => {
                        if let Some(e) = visible.get(selected) {
                            return Ok(Some((*e).clone()));
                        }
                    }
                    (KeyCode::Up, _) => selected = selected.saturating_sub(1),
                    (KeyCode::Down, _) => {
                        if selected + 1 < visible.len() {
                            selected += 1;
                        }
                    }
                    (KeyCode::Tab, _) => {
                        scoped = !scoped;
                        all_entries =
                            entries(if scoped { start_cwd.as_deref() } else { None });
                        selected = 0;
                        offset = 0;
                    }
                    (KeyCode::Backspace, _) => {
                        query.pop();
                        selected = 0;
                    }
                    (KeyCode::Char(c), m)
                        if !m.contains(KeyModifiers::CONTROL)
                            && !m.contains(KeyModifiers::ALT) =>
                    {
                        query.push(c);
                        selected = 0;
                    }
                    _ => {}
                }
            }
            Event::Resize(_, _) => {}
            _ => {}
        }
    }
}

fn draw(
    visible: &[&PickEntry],
    query: &str,
    selected: usize,
    offset: &mut usize,
    scoped: bool,
    cwd: Option<&std::path::Path>,
) -> Result<()> {
    let (cols, rows) = size().unwrap_or((80, 24));
    let width = cols as usize;
    let list_rows = (rows as usize).saturating_sub(6).max(3);

    // Keep the selection inside the window.
    if selected < *offset {
        *offset = selected;
    }
    if selected >= *offset + list_rows {
        *offset = selected + 1 - list_rows;
    }

    const DIM: &str = "\x1b[2m";
    const BOLD: &str = "\x1b[1m";
    const RESET: &str = "\x1b[0m";
    const INV: &str = "\x1b[7m";

    let mut s = String::new();
    s.push_str("\x1b[2J\x1b[H");

    // Header.
    s.push_str(&format!(
        "  {BOLD}constant{RESET} \u{2014} resume a session\r\n\r\n"
    ));
    let scope_label = if scoped {
        format!(
            "[{}]",
            cwd.and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "cwd".to_string())
        )
    } else {
        "[everywhere]".to_string()
    };
    let q_shown = if query.is_empty() {
        format!("{DIM}type to search{RESET}")
    } else {
        format!("{BOLD}{}{RESET}", crate::term_safe(query))
    };
    s.push_str(&format!(
        "  \u{25b8} {q_shown}    {DIM}scope:{RESET} {scope_label} {DIM}(tab toggles){RESET}\r\n\r\n"
    ));

    // Rows.
    if visible.is_empty() {
        s.push_str(&format!("  {DIM}nothing matches{RESET}\r\n"));
    }
    for (i, e) in visible
        .iter()
        .enumerate()
        .skip(*offset)
        .take(list_rows)
    {
        let color = trail::runtime_paint(e.runtime.label());
        let age = trail::ago(e.mtime_secs);
        let name_budget = if scoped {
            width.saturating_sub(34)
        } else {
            width.saturating_sub(58)
        };
        let name = trail::clip(&crate::term_safe(&e.display), name_budget.max(16));
        let name = if e.known {
            format!("{BOLD}{name}{RESET}")
        } else {
            name
        };
        // Everywhere-scope shows each session's home, shortened.
        let home = std::env::var("HOME").unwrap_or_default();
        let place = if scoped {
            String::new()
        } else {
            e.cwd
                .as_deref()
                .map(|c| {
                    let c = if !home.is_empty() && c.starts_with(&home) {
                        format!("~{}", &c[home.len()..])
                    } else {
                        c.to_string()
                    };
                    format!("  {DIM}{}{RESET}", trail::clip(&crate::term_safe(&c), 28))
                })
                .unwrap_or_default()
        };
        let line = format!(
            " {DIM}{age:>8}{RESET}  {color}{:<9}{RESET} {name}{place}",
            e.runtime.label()
        );
        if i == selected {
            s.push_str(&format!(" {INV}\u{276f}{RESET}{line}\r\n"));
        } else {
            s.push_str(&format!("  {line}\r\n"));
        }
    }

    // Footer.
    s.push_str(&format!(
        "\r\n  {DIM}{} of {} \u{b7} enter resume \u{b7} \u{2191}\u{2193} browse \u{b7} tab scope \u{b7} esc exit{RESET}\r\n",
        if visible.is_empty() { 0 } else { selected + 1 },
        visible.len(),
    ));

    let mut out = std::io::stdout();
    out.write_all(s.as_bytes())?;
    out.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(rt: Runtime, id: &str, display: &str, ts: u64) -> PickEntry {
        PickEntry {
            runtime: rt,
            id: id.into(),
            display: display.into(),
            known: false,
            cwd: None,
            mtime_secs: ts,
        }
    }

    #[test]
    fn filter_matches_name_id_and_runtime() {
        let list = vec![
            e(Runtime::Codex, "019e993b", "I want to market constant", 2),
            e(Runtime::Claude, "abcd1234", "auth bug hunt", 1),
        ];
        assert_eq!(filter(&list, "market").len(), 1);
        assert_eq!(filter(&list, "MARKET").len(), 1);
        assert_eq!(filter(&list, "abcd").len(), 1);
        assert_eq!(filter(&list, "claude").len(), 1);
        assert_eq!(filter(&list, "").len(), 2);
        assert_eq!(filter(&list, "zzz").len(), 0);
    }
}
