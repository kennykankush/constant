//! The trail explorer — `constant trail`, interactive. The filing cabinet
//! gets a glass front: conversations → chapters → filed turns, zooming in
//! with Enter and back out with Esc, every level readable without leaving
//! the terminal.
//!
//!   level 0  the trail       one row per conversation (type-to-search, Tab scope)
//!   level 1  a conversation  its chapter chain, live projections, record status
//!   level 2  a chapter       the record volume's turn index (type-to-search)
//!   level 3  a turn          the verbatim text, scrollable; ←/→ walk the turns
//!
//! The turn view is `constant recall` made browsable — same volumes, same
//! ch·turn addresses (each view prints the equivalent recall command, so the
//! explorer teaches the addressing scheme as you read). Read-only throughout:
//! the explorer never writes the ledger or the vault. `r` on a conversation
//! returns its handle so the caller can wake it hosted — the same path as
//! `constant resume HANDLE`.

use std::io::Write;
use std::path::PathBuf;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};

use crate::alembic;
use crate::runtime::Runtime;
use crate::trail;
use crate::tui::{BOLD, DIM, INV, RESET, Screen};

/// One conversational turn from a record volume: (turn number, role, text).
type Turn = (usize, String, String);

enum Level {
    Trail,
    Conversation,
    Chapter,
    Turn,
}

struct State {
    // level 0 — the trail
    scope_all: bool,
    cwd: Option<PathBuf>,
    convs: Vec<trail::ConversationView>,
    conv_query: String,
    conv_sel: usize,
    conv_off: usize,
    // level 1 — one conversation
    conv: Option<trail::ConversationView>,
    chapters: Vec<trail::ChapterRow>,
    chap_sel: usize,
    // level 2 — one chapter's record volume
    chapter_n: u32,
    turns: Vec<Turn>,
    turn_query: String,
    turn_sel: usize,
    turn_off: usize,
    // level 3 — one turn, verbatim
    scroll: usize,
    // one-shot status line (cleared on the next key)
    notice: Option<String>,
}

fn conv_matches(c: &trail::ConversationView, q: &str) -> bool {
    let q = q.to_lowercase();
    q.is_empty()
        || c.name.to_lowercase().contains(&q)
        || c.handle.to_lowercase().contains(&q)
        || c.slug.to_lowercase().contains(&q)
}

fn turn_matches(t: &Turn, q: &str) -> bool {
    let q = q.to_lowercase();
    q.is_empty() || t.1.to_lowercase().contains(&q) || t.2.to_lowercase().contains(&q)
}

/// Soft word-wrap one logical line into rows of at most `width` chars
/// (hard break when a single word overflows the row).
fn wrap_line(line: &str, width: usize, out: &mut Vec<String>) {
    let width = width.max(8);
    let chars: Vec<char> = line.chars().collect();
    let mut start = 0;
    while chars.len() - start > width {
        let slice = &chars[start..start + width];
        let brk = slice
            .iter()
            .rposition(|c| *c == ' ')
            .filter(|p| *p > 0)
            .unwrap_or(width);
        out.push(chars[start..start + brk].iter().collect());
        start += brk;
        while start < chars.len() && chars[start] == ' ' {
            start += 1;
        }
    }
    out.push(chars[start..].iter().collect());
}

fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        wrap_line(line, width, &mut out);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

/// Keep `selected` visible inside a `rows`-tall window starting at `offset`.
fn window(selected: usize, offset: &mut usize, rows: usize) {
    if selected < *offset {
        *offset = selected;
    }
    if selected >= *offset + rows {
        *offset = selected + 1 - rows;
    }
}

fn home_short(path: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    if !home.is_empty() && path.starts_with(&home) {
        format!("~{}", &path[home.len()..])
    } else {
        path.to_string()
    }
}

/// Read the turns of one chapter's record volume.
fn chapter_turns(conv_id: &str, row: &trail::ChapterRow) -> Result<Vec<Turn>> {
    let from = Runtime::parse(&row.from)?;
    let path = trail::snapshot_path(conv_id, row.n, from)
        .ok_or_else(|| anyhow::anyhow!("record vault unavailable"))?;
    let text = std::fs::read_to_string(&path)?;
    let session: alembic::ir::UniversalSession = serde_json::from_str(&text)?;
    Ok(alembic::render::message_turns(&session))
}

/// Run the explorer. Returns a conversation handle when the user asked to
/// resume one (`r`), or None on plain exit.
pub fn explore(start_cwd: Option<PathBuf>) -> Result<Option<String>> {
    if !crate::tui::interactive() {
        anyhow::bail!("the trail explorer needs an interactive terminal (or use --plain)");
    }

    // Start scoped to the folder when it has trail; widen automatically when
    // the folder is empty but the trail elsewhere isn't.
    let mut scope_all = start_cwd.is_none();
    let mut convs = trail::conversations(if scope_all {
        None
    } else {
        start_cwd.as_deref()
    });
    if convs.is_empty() && !scope_all {
        let everywhere = trail::conversations(None);
        if !everywhere.is_empty() {
            scope_all = true;
            convs = everywhere;
        }
    }
    if convs.is_empty() {
        println!("no trail yet \u{2014} start one with `constant host`");
        return Ok(None);
    }

    let mut st = State {
        scope_all,
        cwd: start_cwd,
        convs,
        conv_query: String::new(),
        conv_sel: 0,
        conv_off: 0,
        conv: None,
        chapters: Vec::new(),
        chap_sel: 0,
        chapter_n: 0,
        turns: Vec::new(),
        turn_query: String::new(),
        turn_sel: 0,
        turn_off: 0,
        scroll: 0,
        notice: None,
    };
    let mut level = Level::Trail;

    let _screen = Screen::enter()?;
    loop {
        match level {
            Level::Trail => draw_trail(&mut st)?,
            Level::Conversation => draw_conversation(&mut st)?,
            Level::Chapter => draw_chapter(&mut st)?,
            Level::Turn => draw_turn(&mut st)?,
        }

        let ev = event::read()?;
        let key = match ev {
            Event::Key(k) if k.kind != KeyEventKind::Release => k,
            Event::Resize(_, _) => continue,
            _ => continue,
        };
        st.notice = None;
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return Ok(None);
        }

        match level {
            Level::Trail => {
                let visible: Vec<usize> = (0..st.convs.len())
                    .filter(|i| conv_matches(&st.convs[*i], &st.conv_query))
                    .collect();
                match key.code {
                    KeyCode::Esc if !st.conv_query.is_empty() => {
                        st.conv_query.clear();
                        st.conv_sel = 0;
                    }
                    KeyCode::Esc => return Ok(None),
                    KeyCode::Enter | KeyCode::Right => {
                        if let Some(ix) = visible.get(st.conv_sel) {
                            let conv = st.convs[*ix].clone();
                            st.chapters = trail::chapters(&conv.conversation);
                            st.chap_sel = st.chapters.len().saturating_sub(1);
                            st.conv = Some(conv);
                            level = Level::Conversation;
                        }
                    }
                    KeyCode::Up => st.conv_sel = st.conv_sel.saturating_sub(1),
                    KeyCode::Down => {
                        if st.conv_sel + 1 < visible.len() {
                            st.conv_sel += 1;
                        }
                    }
                    KeyCode::Tab => {
                        st.scope_all = !st.scope_all;
                        st.convs = trail::conversations(if st.scope_all {
                            None
                        } else {
                            st.cwd.as_deref()
                        });
                        st.conv_sel = 0;
                        st.conv_off = 0;
                    }
                    KeyCode::Backspace => {
                        st.conv_query.pop();
                        st.conv_sel = 0;
                    }
                    KeyCode::Char(c)
                        if !key.modifiers.contains(KeyModifiers::CONTROL)
                            && !key.modifiers.contains(KeyModifiers::ALT) =>
                    {
                        st.conv_query.push(c);
                        st.conv_sel = 0;
                    }
                    _ => {}
                }
            }
            Level::Conversation => match key.code {
                KeyCode::Esc | KeyCode::Left => level = Level::Trail,
                KeyCode::Char('r') => {
                    if let Some(conv) = &st.conv {
                        return Ok(Some(conv.handle.clone()));
                    }
                }
                KeyCode::Enter | KeyCode::Right => {
                    let Some(conv) = &st.conv else { continue };
                    let Some(row) = st.chapters.get(st.chap_sel) else {
                        st.notice = Some("no chapters recorded yet".to_string());
                        continue;
                    };
                    if !row.recorded {
                        st.notice = Some(format!(
                            "ch{:02}'s record volume is missing on this machine",
                            row.n
                        ));
                        continue;
                    }
                    match chapter_turns(&conv.conversation, row) {
                        Ok(turns) if !turns.is_empty() => {
                            st.chapter_n = row.n;
                            st.turns = turns;
                            st.turn_query.clear();
                            st.turn_sel = 0;
                            st.turn_off = 0;
                            level = Level::Chapter;
                        }
                        Ok(_) => st.notice = Some("that volume holds no turns".to_string()),
                        Err(e) => st.notice = Some(format!("could not open the volume: {e}")),
                    }
                }
                KeyCode::Up => st.chap_sel = st.chap_sel.saturating_sub(1),
                KeyCode::Down if st.chap_sel + 1 < st.chapters.len() => st.chap_sel += 1,
                _ => {}
            },
            Level::Chapter => {
                let visible: Vec<usize> = (0..st.turns.len())
                    .filter(|i| turn_matches(&st.turns[*i], &st.turn_query))
                    .collect();
                match key.code {
                    KeyCode::Esc if !st.turn_query.is_empty() => {
                        st.turn_query.clear();
                        st.turn_sel = 0;
                    }
                    KeyCode::Esc | KeyCode::Left => level = Level::Conversation,
                    KeyCode::Enter | KeyCode::Right => {
                        if visible.get(st.turn_sel).is_some() {
                            st.scroll = 0;
                            level = Level::Turn;
                        }
                    }
                    KeyCode::Up => st.turn_sel = st.turn_sel.saturating_sub(1),
                    KeyCode::Down => {
                        if st.turn_sel + 1 < visible.len() {
                            st.turn_sel += 1;
                        }
                    }
                    KeyCode::PageUp => st.turn_sel = st.turn_sel.saturating_sub(10),
                    KeyCode::PageDown => {
                        st.turn_sel = (st.turn_sel + 10).min(visible.len().saturating_sub(1));
                    }
                    KeyCode::Backspace => {
                        st.turn_query.pop();
                        st.turn_sel = 0;
                    }
                    KeyCode::Char(c)
                        if !key.modifiers.contains(KeyModifiers::CONTROL)
                            && !key.modifiers.contains(KeyModifiers::ALT) =>
                    {
                        st.turn_query.push(c);
                        st.turn_sel = 0;
                    }
                    _ => {}
                }
            }
            Level::Turn => {
                let visible: Vec<usize> = (0..st.turns.len())
                    .filter(|i| turn_matches(&st.turns[*i], &st.turn_query))
                    .collect();
                match key.code {
                    KeyCode::Esc => level = Level::Chapter,
                    KeyCode::Left => {
                        if st.turn_sel > 0 {
                            st.turn_sel -= 1;
                            st.scroll = 0;
                        }
                    }
                    KeyCode::Right => {
                        if st.turn_sel + 1 < visible.len() {
                            st.turn_sel += 1;
                            st.scroll = 0;
                        }
                    }
                    KeyCode::Up => st.scroll = st.scroll.saturating_sub(1),
                    KeyCode::Down => st.scroll += 1,
                    KeyCode::PageUp => st.scroll = st.scroll.saturating_sub(20),
                    KeyCode::PageDown => st.scroll += 20,
                    KeyCode::Home => st.scroll = 0,
                    _ => {}
                }
            }
        }
    }
}

fn paint(runtime: &str, tty_color: bool) -> String {
    if tty_color {
        format!("{}{}{RESET}", trail::runtime_paint(runtime), runtime)
    } else {
        runtime.to_string()
    }
}

fn flush(s: String) -> Result<()> {
    let mut out = std::io::stdout();
    out.write_all(s.as_bytes())?;
    out.flush()?;
    Ok(())
}

// --- level 0: the trail ---

fn draw_trail(st: &mut State) -> Result<()> {
    let (width, rows) = crate::tui::dimensions();
    let list_rows = rows.saturating_sub(6).max(3);

    let visible: Vec<&trail::ConversationView> = st
        .convs
        .iter()
        .filter(|c| conv_matches(c, &st.conv_query))
        .collect();
    if st.conv_sel >= visible.len() {
        st.conv_sel = visible.len().saturating_sub(1);
    }
    window(st.conv_sel, &mut st.conv_off, list_rows);

    let mut s = String::from("\x1b[2J\x1b[H");
    s.push_str(&format!(
        "  {BOLD}constant{RESET} \u{2014} the trail\r\n\r\n"
    ));
    let scope = if st.scope_all {
        "[everywhere]".to_string()
    } else {
        format!(
            "[{}]",
            st.cwd
                .as_deref()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "folder".to_string())
        )
    };
    let q = if st.conv_query.is_empty() {
        format!("{DIM}type to search{RESET}")
    } else {
        format!("{BOLD}{}{RESET}", crate::term_safe(&st.conv_query))
    };
    s.push_str(&format!(
        "  \u{25b8} {q}    {DIM}scope:{RESET} {scope} {DIM}(tab toggles){RESET}\r\n\r\n"
    ));

    if visible.is_empty() {
        s.push_str(&format!("  {DIM}nothing matches{RESET}\r\n"));
    }
    for (i, c) in visible.iter().enumerate().skip(st.conv_off).take(list_rows) {
        let handle = crate::term_safe(&c.handle);
        let chain_plain: Vec<(String, String)> = c
            .projections
            .iter()
            .map(|p| (p.runtime.clone(), format!("ch{:02}", p.last_n)))
            .collect();
        let chain_plain_len: usize = if chain_plain.is_empty() {
            0
        } else {
            chain_plain
                .iter()
                .map(|(r, ch)| r.chars().count() + 1 + ch.chars().count())
                .sum::<usize>()
                + (chain_plain.len() - 1) * 3
        };
        let name_budget = width
            .saturating_sub(14 + handle.chars().count() + 3 + chain_plain_len + 2)
            .max(16);
        let name = trail::clip(&crate::term_safe(&c.name), name_budget);
        let chain = chain_plain
            .iter()
            .map(|(r, ch)| {
                format!("{}{r}{RESET} {DIM}{ch}{RESET}", trail::runtime_paint(r))
            })
            .collect::<Vec<_>>()
            .join(&format!(" {DIM}\u{2192}{RESET} "));
        let line = format!(
            " {DIM}{:>8}{RESET}  {BOLD}{name}{RESET} {DIM}\u{b7} {handle}{RESET}  {chain}",
            trail::ago(c.last_ts)
        );
        if i == st.conv_sel {
            s.push_str(&format!(" {INV}\u{276f}{RESET}{line}\r\n"));
        } else {
            s.push_str(&format!("  {line}\r\n"));
        }
    }

    s.push_str(&format!(
        "\r\n  {DIM}{} of {} \u{b7} enter open \u{b7} \u{2191}\u{2193} browse \u{b7} tab scope \u{b7} esc exit{RESET}\r\n",
        if visible.is_empty() { 0 } else { st.conv_sel + 1 },
        visible.len(),
    ));
    flush(s)
}

// --- level 1: one conversation ---

fn draw_conversation(st: &mut State) -> Result<()> {
    let Some(conv) = &st.conv else {
        return Ok(());
    };
    let mut s = String::from("\x1b[2J\x1b[H");
    let handle = crate::term_safe(&conv.handle);
    let name = trail::clip(&crate::term_safe(&conv.name), 64);
    s.push_str(&format!("  {BOLD}{name}{RESET}  {DIM}\u{b7} {handle}", ));
    if let Some(c) = &conv.cwd {
        s.push_str(&format!(" \u{b7} {}", crate::term_safe(&home_short(c))));
    }
    s.push_str(&format!("{RESET}\r\n\r\n"));

    if conv.projections.is_empty() {
        s.push_str(&format!(
            "  {DIM}no live projections \u{2014} resume reprints from the record{RESET}\r\n"
        ));
    } else {
        let chain = conv
            .projections
            .iter()
            .map(|p| {
                let times = if p.refreshes > 1 {
                    format!(" \u{d7}{}", p.refreshes)
                } else {
                    String::new()
                };
                format!(
                    "{} {DIM}ch{:02}{times}{RESET}",
                    paint(&p.runtime, true),
                    p.last_n
                )
            })
            .collect::<Vec<_>>()
            .join(&format!(" {DIM}\u{2192}{RESET} "));
        s.push_str(&format!("  {DIM}lives in:{RESET} {chain}\r\n"));
    }
    s.push_str(&format!(
        "  {DIM}\u{21b3} constant resume {handle}{RESET}\r\n\r\n"
    ));

    s.push_str(&format!("  {DIM}chapters \u{b7} the record{RESET}\r\n"));
    if st.chapters.is_empty() {
        s.push_str(&format!("  {DIM}none recorded yet{RESET}\r\n"));
    }
    for (i, row) in st.chapters.iter().enumerate() {
        let hop_plain = format!("{} \u{2192} {}", row.from, row.to);
        let pad = " ".repeat(22usize.saturating_sub(hop_plain.chars().count()));
        let hop = format!(
            "{} {DIM}\u{2192}{RESET} {}{pad}",
            paint(&crate::term_safe(&row.from), true),
            paint(&crate::term_safe(&row.to), true)
        );
        let status = if row.recorded {
            "ok     ".to_string()
        } else {
            format!("{DIM}missing{RESET}")
        };
        let line = format!(
            "ch{:02}  {hop} {status}  {DIM}{}{RESET}",
            row.n,
            trail::ago(row.ts)
        );
        if i == st.chap_sel {
            s.push_str(&format!(" {INV}\u{276f}{RESET} {line}\r\n"));
        } else {
            s.push_str(&format!("   {line}\r\n"));
        }
    }

    s.push_str("\r\n");
    if let Some(n) = &st.notice {
        s.push_str(&format!("  {n}\r\n"));
    }
    s.push_str(&format!(
        "  {DIM}enter read a chapter \u{b7} r resume hosted \u{b7} \u{2191}\u{2193} \u{b7} esc back{RESET}\r\n"
    ));
    flush(s)
}

// --- level 2: one chapter's turn index ---

fn draw_chapter(st: &mut State) -> Result<()> {
    let Some(conv) = &st.conv else {
        return Ok(());
    };
    let (width, rows) = crate::tui::dimensions();
    let list_rows = rows.saturating_sub(7).max(3);

    let visible: Vec<&Turn> = st
        .turns
        .iter()
        .filter(|t| turn_matches(t, &st.turn_query))
        .collect();
    if st.turn_sel >= visible.len() {
        st.turn_sel = visible.len().saturating_sub(1);
    }
    window(st.turn_sel, &mut st.turn_off, list_rows);

    let mut s = String::from("\x1b[2J\x1b[H");
    let name = trail::clip(&crate::term_safe(&conv.name), 48);
    s.push_str(&format!(
        "  {BOLD}{name}{RESET}  {DIM}\u{b7} ch{:02} \u{b7} {} turns{RESET}\r\n\r\n",
        st.chapter_n,
        st.turns.len()
    ));
    let q = if st.turn_query.is_empty() {
        format!("{DIM}type to search the turns{RESET}")
    } else {
        format!("{BOLD}{}{RESET}", crate::term_safe(&st.turn_query))
    };
    s.push_str(&format!("  \u{25b8} {q}\r\n\r\n"));

    if visible.is_empty() {
        s.push_str(&format!("  {DIM}nothing matches{RESET}\r\n"));
    }
    let preview_budget = width.saturating_sub(26).max(20);
    for (i, (n, role, text)) in visible
        .iter()
        .map(|t| (&t.0, &t.1, &t.2))
        .enumerate()
        .skip(st.turn_off)
        .take(list_rows)
    {
        let head = alembic::render::preview(text, preview_budget);
        let line = format!(
            "{DIM}ch{:02}\u{b7}{n:<4}{RESET} {DIM}{role:>9}\u{2192}{RESET} {}",
            st.chapter_n,
            crate::term_safe(&head)
        );
        if i == st.turn_sel {
            s.push_str(&format!(" {INV}\u{276f}{RESET} {line}\r\n"));
        } else {
            s.push_str(&format!("   {line}\r\n"));
        }
    }

    s.push_str(&format!(
        "\r\n  {DIM}{} of {} \u{b7} enter read \u{b7} \u{2191}\u{2193} \u{b7} esc back \u{b7} cli: constant recall {} ch{:02} <turn>{RESET}\r\n",
        if visible.is_empty() { 0 } else { st.turn_sel + 1 },
        visible.len(),
        crate::term_safe(&conv.handle),
        st.chapter_n,
    ));
    flush(s)
}

// --- level 3: one turn, verbatim ---

fn draw_turn(st: &mut State) -> Result<()> {
    let Some(conv) = &st.conv else {
        return Ok(());
    };
    let (width, rows) = crate::tui::dimensions();
    let body_rows = rows.saturating_sub(5).max(3);

    let visible: Vec<&Turn> = st
        .turns
        .iter()
        .filter(|t| turn_matches(t, &st.turn_query))
        .collect();
    if st.turn_sel >= visible.len() {
        st.turn_sel = visible.len().saturating_sub(1);
    }
    let Some((n, role, text)) = visible.get(st.turn_sel).map(|t| (&t.0, &t.1, &t.2)) else {
        return Ok(());
    };

    let lines = wrap_text(&crate::term_safe(text), width.saturating_sub(4));
    let max_scroll = lines.len().saturating_sub(body_rows);
    if st.scroll > max_scroll {
        st.scroll = max_scroll;
    }

    let mut s = String::from("\x1b[2J\x1b[H");
    let name = trail::clip(&crate::term_safe(&conv.name), 40);
    s.push_str(&format!(
        "  {BOLD}{name}{RESET}  {DIM}\u{b7} ch{:02}\u{b7}{n} \u{b7} {role} \u{b7} {} of {}{RESET}\r\n\r\n",
        st.chapter_n,
        st.turn_sel + 1,
        visible.len()
    ));

    for line in lines.iter().skip(st.scroll).take(body_rows) {
        s.push_str(&format!("  {line}\r\n"));
    }

    let more = if st.scroll < max_scroll {
        format!(" \u{b7} \u{2193} {} more lines", max_scroll - st.scroll)
    } else {
        String::new()
    };
    s.push_str(&format!(
        "\r\n  {DIM}\u{2191}\u{2193} scroll{more} \u{b7} \u{2190}\u{2192} walk turns \u{b7} esc back \u{b7} cli: constant recall {} ch{:02} {n}{RESET}\r\n",
        crate::term_safe(&conv.handle),
        st.chapter_n,
    ));
    flush(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_breaks_at_spaces_and_hard_breaks_long_words() {
        let wrapped = wrap_text("the quick brown fox jumps over the lazy dog", 16);
        assert!(wrapped.iter().all(|l| l.chars().count() <= 16), "{wrapped:?}");
        assert_eq!(wrapped.join(" "), "the quick brown fox jumps over the lazy dog");

        let long = "x".repeat(40);
        let wrapped = wrap_text(&long, 16);
        assert!(wrapped.len() >= 3);
        assert!(wrapped.iter().all(|l| l.chars().count() <= 16));

        // Empty lines survive (paragraph breaks stay visible).
        let wrapped = wrap_text("a\n\nb", 16);
        assert_eq!(wrapped, vec!["a", "", "b"]);
    }

    #[test]
    fn turn_filter_searches_role_and_content() {
        let t: Turn = (9, "user".into(), "the Norway problem in YAML".into());
        assert!(turn_matches(&t, ""));
        assert!(turn_matches(&t, "norway"));
        assert!(turn_matches(&t, "USER"));
        assert!(!turn_matches(&t, "zzz"));
    }
}
