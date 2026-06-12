//! The resume picker — an interactive, type-to-search session list, in the
//! spirit of the runtimes' own `/resume` pickers but across ALL of them at
//! once: every session in this directory (or everywhere), annotated with the
//! conversation handles Constant knows, names pulled from each runtime's own
//! registry where one exists.
//!
//! Opens INSTANTLY, then fills in: the first paint lists from cheap sources
//! only (directory walks, the codex registry's one query, one trail-ledger
//! read), while the per-file reads — claude title tailing, codex cwd heads —
//! stream in from a background thread, newest first. A scope flip bumps the
//! generation and stale enrichment is dropped on the floor.
//!
//! Draws on the ALTERNATE screen (the terminal restores the shell on exit),
//! raw mode for keys, no extra dependencies — the same primitives the host
//! already lives on.

use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};

use crate::alembic;
use crate::runtime::Runtime;
use crate::trail;
use crate::tui::Screen;

/// One pickable row. Naming sources are kept raw and the display name is
/// computed on read, so streamed enrichment (a claude title arriving late)
/// re-ranks the precedence without re-deriving anything else.
#[derive(Clone)]
pub struct PickEntry {
    pub runtime: Runtime,
    pub id: String,
    pub path: PathBuf,
    /// The runtime's own name for it (codex registry at load; claude's
    /// customTitle/first-message streams in from the enrichment thread).
    pub runtime_title: Option<String>,
    /// (name, handle, named) when the constant trail knows this session.
    pub trail: Option<(String, String, bool)>,
    pub cwd: Option<String>,
    pub mtime_secs: u64,
}

impl PickEntry {
    /// What a person knows it by: a user-locked trail name wins, then the
    /// runtime's own title, then the auto trail name, then the id.
    pub fn display(&self) -> String {
        match (&self.trail, &self.runtime_title) {
            (Some((name, _, true)), _) => name.clone(),
            (_, Some(t)) => t.clone(),
            (Some((name, _, false)), None) => name.clone(),
            (None, None) => self.id.clone(),
        }
    }

    pub fn handle(&self) -> Option<&str> {
        self.trail.as_ref().map(|(_, h, _)| h.as_str())
    }

    /// True when the constant trail knows this session (rendered bold).
    pub fn known(&self) -> bool {
        self.trail.is_some()
    }
}

/// Gather pickable sessions, newest first — the FAST pass (no per-file
/// reads; see the module docs for what streams in afterwards).
pub fn entries(cwd: Option<&std::path::Path>) -> Vec<PickEntry> {
    let naming = trail::naming_index();
    let mut out = Vec::new();
    for rt in [
        Runtime::Codex,
        Runtime::Claude,
        Runtime::OpenCode,
        Runtime::Gemini,
    ] {
        for s in alembic::list_sessions_lazy(rt, cwd) {
            let mtime_secs = s
                .mtime
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            out.push(PickEntry {
                runtime: rt,
                trail: naming.get(&s.id).cloned(),
                runtime_title: s.title.filter(|t| !t.is_empty()),
                id: s.id,
                path: s.path,
                cwd: s.cwd,
                mtime_secs,
            });
        }
    }
    out.sort_by_key(|e| std::cmp::Reverse(e.mtime_secs));
    out
}

/// Case-insensitive filter over display name, id, runtime label, and handle.
pub fn filter<'a>(entries: &'a [PickEntry], query: &str) -> Vec<&'a PickEntry> {
    let q = query.to_lowercase();
    entries
        .iter()
        .filter(|e| {
            q.is_empty()
                || e.display().to_lowercase().contains(&q)
                || e.id.to_lowercase().contains(&q)
                || e.runtime.label().contains(&q)
                || e
                    .handle()
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

/// Rows one screen of the list holds — the unit PgUp/PgDn flip by, and the
/// same number draw() windows to (one definition, or paging and drawing drift).
fn page_size() -> usize {
    crate::tui::dimensions().1.saturating_sub(6).max(3)
}

fn load(scope: Scope, cwd: Option<&std::path::Path>) -> Vec<PickEntry> {
    let t = std::time::Instant::now();
    let mut v = match scope.place {
        Place::Cwd => entries(cwd),
        Place::All => entries(None),
    };
    if scope.constant_only {
        v.retain(|e| e.known());
    }
    if std::env::var_os("CONSTANT_DEBUG_TIMING").is_some() {
        eprintln!("[timing] load: {}ms, {} rows\r", t.elapsed().as_millis(), v.len());
    }
    v
}

/// One streamed enrichment result, tagged with its load generation.
enum Enrich {
    Row {
        ix: usize,
        title: Option<String>,
        cwd: Option<String>,
    },
    Done,
}

/// Spawn the enrichment sweep for one load generation: the per-file reads
/// the fast pass skipped, newest first. Returns true when there is anything
/// to sweep (so the caller knows whether to show the scanning indicator).
fn spawn_enrichment(generation: u64, rows: &[PickEntry], tx: mpsc::Sender<(u64, Enrich)>) -> bool {
    let work: Vec<(usize, PathBuf, bool, bool)> = rows
        .iter()
        .enumerate()
        .filter_map(|(ix, e)| {
            let need_title = e.runtime == Runtime::Claude && e.runtime_title.is_none();
            let need_cwd = e.runtime == Runtime::Codex && e.cwd.is_none();
            (need_title || need_cwd).then(|| (ix, e.path.clone(), need_title, need_cwd))
        })
        .collect();
    if work.is_empty() {
        return false;
    }
    std::thread::spawn(move || {
        for (ix, path, need_title, need_cwd) in work {
            let title = need_title
                .then(|| alembic::claude_quick_title(&path))
                .flatten();
            let cwd = need_cwd
                .then(|| alembic::codex_session_cwd(&path))
                .flatten();
            if (title.is_some() || cwd.is_some())
                && tx.send((generation, Enrich::Row { ix, title, cwd })).is_err()
            {
                return; // picker is gone
            }
        }
        let _ = tx.send((generation, Enrich::Done));
    });
    true
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
    let (tx, rx) = mpsc::channel::<(u64, Enrich)>();
    let mut generation: u64 = 0;
    let mut all_entries = load(scope, start_cwd.as_deref());
    let mut scanning = spawn_enrichment(generation, &all_entries, tx.clone());
    let mut query = String::new();
    let mut selected: usize = 0;
    let mut page: usize = 0;
    let mut dirty = true;

    let _screen = Screen::enter()?;
    loop {
        // Apply whatever the sweep has sent since the last tick.
        while let Ok((generation_of, enrich)) = rx.try_recv() {
            if generation_of != generation {
                continue; // a stale sweep from before a scope flip
            }
            match enrich {
                Enrich::Row { ix, title, cwd } => {
                    if let Some(e) = all_entries.get_mut(ix) {
                        if e.runtime_title.is_none() {
                            e.runtime_title = title;
                        }
                        if e.cwd.is_none() {
                            e.cwd = cwd;
                        }
                        dirty = true;
                    }
                }
                Enrich::Done => {
                    scanning = false;
                    dirty = true;
                }
            }
        }

        if dirty {
            let visible = filter(&all_entries, &query);
            // Pages are fixed windows; the selection lives inside the
            // current one (a shrunken filter pulls both back into range).
            let ps = page_size();
            let pages = visible.len().div_ceil(ps).max(1);
            page = page.min(pages - 1);
            let start = page * ps;
            let end = (start + ps).min(visible.len());
            if selected < start || selected >= end {
                selected = start;
            }
            draw(
                &visible,
                &query,
                selected,
                page,
                scope,
                scanning,
                start_cwd.as_deref(),
            )?;
            dirty = false;
        }

        // Tick: keys when they come, enrichment drained in between.
        if !event::poll(Duration::from_millis(60))? {
            continue;
        }
        match event::read()? {
            Event::Key(k) => {
                // Only key PRESSES (kitty-capable terminals also report releases).
                if k.kind == event::KeyEventKind::Release {
                    continue;
                }
                dirty = true;
                let visible_len = filter(&all_entries, &query).len();
                let ps = page_size();
                let page_start = page * ps;
                let page_end = (page_start + ps).min(visible_len);
                match (k.code, k.modifiers) {
                    (KeyCode::Esc, _) => return Ok(None),
                    (KeyCode::Char('c'), KeyModifiers::CONTROL) => return Ok(None),
                    (KeyCode::Enter, _) => {
                        let visible = filter(&all_entries, &query);
                        if let Some(e) = visible.get(selected) {
                            return Ok(Some((*e).clone()));
                        }
                    }
                    // Arrows stay INSIDE the page; only the page keys turn it.
                    (KeyCode::Up, _) => {
                        if selected > page_start {
                            selected -= 1;
                        }
                    }
                    (KeyCode::Down, _) => {
                        if selected + 1 < page_end {
                            selected += 1;
                        }
                    }
                    (KeyCode::PageUp, _) => {
                        if page > 0 {
                            page -= 1;
                            selected = page * ps;
                        }
                    }
                    (KeyCode::PageDown, _) => {
                        if page_end < visible_len {
                            page += 1;
                            selected = page * ps;
                        }
                    }
                    (KeyCode::Home, _) => {
                        page = 0;
                        selected = 0;
                    }
                    (KeyCode::End, _) => {
                        selected = visible_len.saturating_sub(1);
                        page = selected / ps;
                    }
                    (KeyCode::Tab, _) => {
                        scope.place = match scope.place {
                            Place::Cwd => Place::All,
                            Place::All if start_cwd.is_some() => Place::Cwd,
                            Place::All => Place::All,
                        };
                        generation += 1;
                        all_entries = load(scope, start_cwd.as_deref());
                        scanning = spawn_enrichment(generation, &all_entries, tx.clone());
                        selected = 0;
                        page = 0;
                    }
                    (KeyCode::Char('t'), KeyModifiers::CONTROL) => {
                        scope.constant_only = !scope.constant_only;
                        generation += 1;
                        all_entries = load(scope, start_cwd.as_deref());
                        scanning = spawn_enrichment(generation, &all_entries, tx.clone());
                        selected = 0;
                        page = 0;
                    }
                    (KeyCode::Backspace, _) => {
                        query.pop();
                        selected = 0;
                        page = 0;
                    }
                    (KeyCode::Char(c), m)
                        if !m.contains(KeyModifiers::CONTROL)
                            && !m.contains(KeyModifiers::ALT) =>
                    {
                        query.push(c);
                        selected = 0;
                        page = 0;
                    }
                    _ => {}
                }
            }
            Event::Resize(_, _) => dirty = true,
            _ => {}
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw(
    visible: &[&PickEntry],
    query: &str,
    selected: usize,
    page: usize,
    scope: Scope,
    scanning: bool,
    cwd: Option<&std::path::Path>,
) -> Result<()> {
    let (width, _) = crate::tui::dimensions();
    let list_rows = page_size();
    let page_start = page * list_rows;

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
    let scan_note = if scanning {
        " \u{b7} scanning names\u{2026}"
    } else {
        ""
    };
    let q_shown = if query.is_empty() {
        format!("{DIM}type to search{RESET}")
    } else {
        format!("{BOLD}{}{RESET}", crate::term_safe(query))
    };
    s.push_str(&format!(
        "  \u{25b8} {q_shown}    {DIM}scope: {scope_label}{scan_note}{RESET}\r\n\r\n"
    ));

    // Rows.
    if visible.is_empty() {
        s.push_str(&format!("  {DIM}nothing matches{RESET}\r\n"));
    }
    for (i, e) in visible
        .iter()
        .enumerate()
        .skip(page_start)
        .take(list_rows)
    {
        let color = trail::runtime_paint(e.runtime.label());
        let age = trail::ago(e.mtime_secs);
        let name_budget = if scope.place == Place::Cwd {
            width.saturating_sub(34)
        } else {
            width.saturating_sub(58)
        };
        let name = trail::clip(&crate::term_safe(&e.display()), name_budget.max(16));
        let mut name = if e.known() {
            format!("{BOLD}{name}{RESET}")
        } else {
            name
        };
        // The title is the protagonist; the handle decorates it, dim.
        if let Some(h) = e.handle() {
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

    // Footer: position + page, then the keys.
    let pages = visible.len().div_ceil(list_rows).max(1);
    s.push_str(&format!(
        "\r\n  {DIM}{} of {} \u{b7} page {}/{pages} \u{b7} enter resume \u{b7} \u{2191}\u{2193} within \u{b7} pgup/pgdn turn \u{b7} tab scope \u{b7} ^t constant \u{b7} esc{RESET}\r\n",
        if visible.is_empty() { 0 } else { selected + 1 },
        visible.len(),
        page + 1,
    ));

    let mut out = std::io::stdout();
    out.write_all(s.as_bytes())?;
    out.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(rt: Runtime, id: &str, title: &str, ts: u64) -> PickEntry {
        PickEntry {
            runtime: rt,
            id: id.into(),
            path: PathBuf::new(),
            runtime_title: Some(title.into()),
            trail: None,
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

    #[test]
    fn display_precedence_locked_beats_runtime_beats_auto_beats_id() {
        let mut entry = e(Runtime::Codex, "019e993b", "registry title", 1);
        // Runtime title over an auto trail name…
        entry.trail = Some(("auto name".into(), "cobalt-37".into(), false));
        assert_eq!(entry.display(), "registry title");
        // …but a user-locked trail name wins over everything.
        entry.trail = Some(("my name".into(), "cobalt-37".into(), true));
        assert_eq!(entry.display(), "my name");
        // No runtime title: the auto trail name serves.
        entry.runtime_title = None;
        entry.trail = Some(("auto name".into(), "cobalt-37".into(), false));
        assert_eq!(entry.display(), "auto name");
        // Nothing known: the id is the last resort.
        entry.trail = None;
        assert_eq!(entry.display(), "019e993b");
    }
}
