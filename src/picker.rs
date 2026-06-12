//! The resume picker — an interactive, type-to-search session list, in the
//! spirit of the runtimes' own `/resume` pickers but across ALL of them at
//! once: every session in this directory (or everywhere), annotated with the
//! conversation handles Constant knows, names pulled from each runtime's own
//! registry where one exists.
//!
//! Draws on the ALTERNATE screen (the terminal restores the shell on exit),
//! raw mode for keys, no extra dependencies — the same primitives the host
//! already lives on.

use std::io::Write;
use std::path::PathBuf;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};

use crate::alembic;
use crate::runtime::Runtime;
use crate::trail;
use crate::tui::Screen;

/// One pickable row.
#[derive(Clone)]
pub struct PickEntry {
    pub runtime: Runtime,
    pub id: String,
    /// What a person knows it by: the conversation name (trail name when
    /// known, else the runtime's own registry title, else the id).
    pub display: String,
    /// Constant's handle when the trail knows this session — pure decoration.
    pub handle: Option<String>,
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
            let runtime_title = s.title.as_deref().filter(|t| !t.is_empty());
            let (display, handle, known) = match trail::naming_parts_for_session(&s.id) {
                // A runtime-side rename outranks an AUTO trail name; a trail
                // name the user locked (constant rename / :rename) wins.
                Some((name, handle, named)) => {
                    let display = match runtime_title {
                        Some(t) if !named => t.to_string(),
                        _ => name,
                    };
                    (display, Some(handle), true)
                }
                None => match runtime_title {
                    Some(t) => (t.to_string(), None, false),
                    None => (s.id.clone(), None, false),
                },
            };
            out.push(PickEntry {
                runtime: rt,
                id: s.id,
                display,
                handle,
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
                || e
                    .handle
                    .as_deref()
                    .map(|h| h.to_lowercase().contains(&q))
                    .unwrap_or(false)
        })
        .collect()
}

/// WHERE the picker looks: this folder or every folder. Orthogonal to the
/// trail lens — place and lens are two axes, not one cycle.
#[derive(Clone, Copy, PartialEq)]
enum Place {
    Cwd,
    All,
}

/// The picker's scope: a place (Tab toggles) plus the constant lens (Ctrl-T)
/// that narrows whichever place you're in to conversations the trail knows.
#[derive(Clone, Copy, PartialEq)]
struct Scope {
    place: Place,
    constant_only: bool,
}

fn load(scope: Scope, cwd: Option<&std::path::Path>) -> Vec<PickEntry> {
    let mut v = match scope.place {
        Place::Cwd => entries(cwd),
        Place::All => entries(None),
    };
    if scope.constant_only {
        v.retain(|e| e.known);
    }
    v
}

/// Run the picker. Returns the chosen entry, or None on cancel.
/// `start_cwd` seeds the [folder] place; Tab toggles folder ↔ everywhere,
/// Ctrl-T lays the constant lens over either.
pub fn pick(start_cwd: Option<PathBuf>) -> Result<Option<PickEntry>> {
    if !crate::tui::interactive() {
        anyhow::bail!("the resume picker needs an interactive terminal");
    }

    let mut scope = Scope {
        place: if start_cwd.is_some() {
            Place::Cwd
        } else {
            Place::All
        },
        constant_only: false,
    };
    let mut all_entries = load(scope, start_cwd.as_deref());
    let mut query = String::new();
    let mut selected: usize = 0;
    let mut offset: usize = 0;

    let _screen = Screen::enter()?;
    loop {
        let visible = filter(&all_entries, &query);
        if selected >= visible.len() {
            selected = visible.len().saturating_sub(1);
        }

        draw(&visible, &query, selected, &mut offset, scope, start_cwd.as_deref())?;

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
                        scope.place = match scope.place {
                            Place::Cwd => Place::All,
                            Place::All if start_cwd.is_some() => Place::Cwd,
                            Place::All => Place::All,
                        };
                        all_entries = load(scope, start_cwd.as_deref());
                        selected = 0;
                        offset = 0;
                    }
                    (KeyCode::Char('t'), KeyModifiers::CONTROL) => {
                        scope.constant_only = !scope.constant_only;
                        all_entries = load(scope, start_cwd.as_deref());
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
    scope: Scope,
    cwd: Option<&std::path::Path>,
) -> Result<()> {
    let (width, rows) = crate::tui::dimensions();
    let list_rows = rows.saturating_sub(6).max(3);

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
    // The scope reads as place + lens: `[everywhere · constant]`.
    let place_label = match scope.place {
        Place::Cwd => cwd
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "folder".to_string()),
        Place::All => "everywhere".to_string(),
    };
    let scope_label = if scope.constant_only {
        format!("[{place_label} \u{b7} {BOLD}constant{RESET}{DIM}]")
    } else {
        format!("[{place_label}]")
    };
    let q_shown = if query.is_empty() {
        format!("{DIM}type to search{RESET}")
    } else {
        format!("{BOLD}{}{RESET}", crate::term_safe(query))
    };
    s.push_str(&format!(
        "  \u{25b8} {q_shown}    {DIM}scope: {scope_label}{RESET}\r\n\r\n"
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
        let name_budget = if scope.place == Place::Cwd {
            width.saturating_sub(34)
        } else {
            width.saturating_sub(58)
        };
        let name = trail::clip(&crate::term_safe(&e.display), name_budget.max(16));
        let mut name = if e.known {
            format!("{BOLD}{name}{RESET}")
        } else {
            name
        };
        // The title is the protagonist; the handle decorates it, dim.
        if let Some(h) = &e.handle {
            name.push_str(&format!(" {DIM}\u{b7} {}{RESET}", crate::term_safe(h)));
        }
        // Everywhere-scope shows each session's home, shortened.
        let home = std::env::var("HOME").unwrap_or_default();
        let place = if scope.place == Place::Cwd {
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
        "\r\n  {DIM}{} of {} \u{b7} enter resume \u{b7} \u{2191}\u{2193} browse \u{b7} tab folder/everywhere \u{b7} ^t constant only \u{b7} esc exit{RESET}\r\n",
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
            handle: None,
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
