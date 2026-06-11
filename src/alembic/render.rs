//! The renderer — the "prepared desk" for carried conversations.
//!
//! Doctrine: the CONVERSATION is total and lives in the record; the CONTEXT a
//! runtime wakes up to is a compiled VIEW of it. Today's default view is the
//! whole thread verbatim. The paged view rearranges it for attention physics:
//!
//!   [head card]  orientation + goal + how to recall filed turns
//!   [index]      one line per filed turn, each with a recall address
//!   [tail]       the most recent turns, verbatim
//!
//! Two laws, fixed in stone:
//!   1. The renderer makes ZERO model calls. It indexes; it never summarizes.
//!      A table of contents cannot hallucinate.
//!   2. Nothing is lost — every filed turn is one `constant recall` away in
//!      the record volume this very carry wrote. Indexed is not dropped.
//!
//! Synthetic head/index messages start with `[constant:` — a marker
//! `is_scaffold` recognizes, so the NEXT distillation strips them instead of
//! carrying stale desk furniture forward (self-cleaning, like the runtimes'
//! own injected scaffold).

use super::ir::{ContentBlock, MessageEvent, SessionEvent, UniversalSession};

/// Verbatim-tail budget for the paged view, in characters (a rough token
/// proxy; ~6k tokens). The tail always keeps at least [`MIN_TAIL_TURNS`].
pub const TAIL_BUDGET_CHARS: usize = 24_000;
/// The tail never shrinks below this many turns, budget or not.
pub const MIN_TAIL_TURNS: usize = 4;

/// What the paged render did, for the receipt.
#[derive(Debug, Default, Clone, Copy)]
pub struct RenderStats {
    /// Turns kept verbatim in the tail (read by tests and the coming
    /// continuation-fidelity benchmark).
    #[allow(dead_code)]
    pub verbatim: usize,
    /// Turns filed into the index (recallable from the record, not dropped).
    pub indexed: usize,
}

/// One conversational turn: (1-based turn number, role, joined text).
///
/// THE shared numbering between the index a projection shows and what
/// `constant recall` resolves — both sides must call this and nothing else,
/// or addresses drift.
pub fn message_turns(session: &UniversalSession) -> Vec<(usize, String, String)> {
    let mut turns = Vec::new();
    for event in &session.events {
        let SessionEvent::Message(message) = event else {
            continue;
        };
        let text = joined_text(message);
        if text.trim().is_empty() {
            continue;
        }
        turns.push((turns.len() + 1, message.role.clone(), text));
    }
    turns
}

fn joined_text(message: &MessageEvent) -> String {
    message
        .blocks
        .iter()
        .filter_map(|b| b.text.as_deref())
        .collect::<Vec<_>>()
        .join("\n")
}

/// First line of a turn, clipped for the index.
fn preview(text: &str, max: usize) -> String {
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let mut out: String = line.chars().take(max).collect();
    if line.chars().count() > max {
        out.push('\u{2026}');
    }
    out
}

/// Best-effort one-line git anchor for the head card (branch · changes ·
/// last commit). Deterministic reads only; None when not a repo.
pub fn git_anchor(cwd: Option<&std::path::Path>) -> Option<String> {
    let dir = cwd?;
    let run = |args: &[&str]| -> Option<String> {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    let branch = run(&["branch", "--show-current"])?;
    let dirty = run(&["status", "--porcelain"])
        .map(|s| s.lines().count())
        .unwrap_or(0);
    let last = run(&["log", "-1", "--format=%h %s"]).unwrap_or_default();
    Some(format!(
        "branch {} \u{b7} {} modified \u{b7} last commit: {}",
        if branch.is_empty() { "detached" } else { &branch },
        dirty,
        preview(&last, 60),
    ))
}

/// Compile the paged view: replace the session's events with
/// head card + index + verbatim tail. Returns what was kept vs filed.
///
/// The record volume for this hop must already hold the FULL thread — the
/// caller is responsible for writing the record BEFORE rendering (the index
/// addresses point into that volume).
#[allow(clippy::too_many_arguments)]
pub fn render_paged(
    session: &mut UniversalSession,
    handle: &str,
    name: &str,
    chapter: u32,
    from_label: &str,
    to_label: &str,
    anchor: Option<&str>,
    tail_budget_chars: usize,
) -> RenderStats {
    let turns = message_turns(session);
    if turns.is_empty() {
        return RenderStats::default();
    }

    // Choose the verbatim tail: newest turns whose combined size fits the
    // budget, never fewer than MIN_TAIL_TURNS.
    let mut tail_start = turns.len();
    let mut used = 0usize;
    for (i, (_, _, text)) in turns.iter().enumerate().rev() {
        let cost = text.chars().count() + 64;
        if used + cost > tail_budget_chars && turns.len() - i > MIN_TAIL_TURNS {
            break;
        }
        used += cost;
        tail_start = i;
    }
    let stats = RenderStats {
        verbatim: turns.len() - tail_start,
        indexed: tail_start,
    };

    let index_note = if stats.indexed > 0 {
        format!(
            "{} earlier turns are FILED in Constant's record (see the index at \
             the very top of this conversation) \u{2014} they are NOT lost. Read \
             any filed turn back exactly as it was said:\n\
             \u{a0}\u{a0}\u{a0} constant recall {handle} ch{chapter:02} <turn or range, e.g. 12 or 12-18>\n",
            stats.indexed
        )
    } else {
        "The whole conversation above is verbatim.\n".to_string()
    };
    let arrival = format!(
        "[constant: taking over]\n\
         You are {to_label}, continuing the conversation \u{201c}{name}\u{201d} ({handle}), \
         chapter {chapter}, taking over from {from_label}.\n\
         {anchor_line}\
         {index_note}\
         Verify state before acting; the working tree is the source of truth.",
        anchor_line = match anchor {
            Some(a) => format!("REPO: {a}\n"),
            None => String::new(),
        },
    );

    // The tail: every original event from the first kept turn onward (tool
    // events riding between kept turns stay interleaved, untouched).
    let cut_event_ix = nth_turn_event_index(session, tail_start);
    let tail = session.events.split_off(cut_event_ix);

    // Layout: [index] + [verbatim tail] + [arrival card]. The card rides the
    // BOTTOM — claude's UI anchors a resumed session there, and the recency
    // edge is where model attention is sharpest (recitation).
    let mut events: Vec<SessionEvent> = Vec::with_capacity(tail.len() + 2);
    if stats.indexed > 0 {
        let mut toc = String::from(
            "[constant: index of filed turns \u{2014} retrieve verbatim with `constant recall`]\n",
        );
        for (n, role, text) in &turns[..tail_start] {
            toc.push_str(&format!(
                "ch{chapter:02}\u{b7}{n}  {role}\u{2192} {}\n",
                preview(text, 88)
            ));
        }
        events.push(synthetic_user(&toc));
    }
    events.extend(tail);
    events.push(synthetic_user(&arrival));
    session.events = events;
    stats
}

/// Event-list index where the (0-based) `turn_ix`-th conversational turn
/// begins; events.len() when past the end.
fn nth_turn_event_index(session: &UniversalSession, turn_ix: usize) -> usize {
    let mut seen = 0usize;
    for (i, event) in session.events.iter().enumerate() {
        if let SessionEvent::Message(message) = event
            && !joined_text(message).trim().is_empty()
        {
            if seen == turn_ix {
                return i;
            }
            seen += 1;
        }
    }
    session.events.len()
}

fn synthetic_user(text: &str) -> SessionEvent {
    SessionEvent::Message(MessageEvent {
        id: None,
        parent_id: None,
        role: "user".to_string(),
        timestamp: None,
        blocks: vec![ContentBlock::text("text", text)],
        metadata: Default::default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(role: &str, text: &str) -> SessionEvent {
        SessionEvent::Message(MessageEvent {
            id: None,
            parent_id: None,
            role: role.to_string(),
            timestamp: None,
            blocks: vec![ContentBlock::text("text", text)],
            metadata: Default::default(),
        })
    }

    fn session(turn_texts: &[(&str, &str)]) -> UniversalSession {
        let mut s = UniversalSession::new("r-0001".to_string());
        for (role, text) in turn_texts {
            s.events.push(msg(role, text));
        }
        s
    }

    #[test]
    fn turn_numbering_is_stable_and_shared() {
        let s = session(&[
            ("user", "first"),
            ("assistant", "second"),
            ("user", ""), // empty: not a turn
            ("user", "third"),
        ]);
        let turns = message_turns(&s);
        assert_eq!(turns.len(), 3);
        assert_eq!(turns[0], (1, "user".into(), "first".into()));
        assert_eq!(turns[2], (3, "user".into(), "third".into()));
    }

    #[test]
    fn paged_render_files_old_turns_and_keeps_the_tail_verbatim() {
        let long = "x".repeat(400);
        let mut texts: Vec<(String, String)> = Vec::new();
        for i in 0..30 {
            texts.push((
                if i % 2 == 0 { "user" } else { "assistant" }.to_string(),
                format!("turn {i} {long}"),
            ));
        }
        let refs: Vec<(&str, &str)> = texts
            .iter()
            .map(|(r, t)| (r.as_str(), t.as_str()))
            .collect();
        let mut s = session(&refs);

        // Small budget: most turns must be filed, tail stays verbatim.
        let stats = render_paged(&mut s, "cobalt-37", "auth bug", 4, "codex", "claude", None, 3_000);
        assert!(stats.indexed > 0, "nothing was filed");
        assert!(stats.verbatim >= MIN_TAIL_TURNS);
        assert_eq!(stats.indexed + stats.verbatim, 30);

        let turns = message_turns(&s);
        // Layout: index first, tail verbatim, arrival card LAST (the bottom
        // is where claude anchors a resumed session AND the recency edge).
        let last = &turns.last().unwrap().2;
        assert!(last.starts_with("[constant: taking over]"), "no arrival card: {last}");
        assert!(last.contains("constant recall cobalt-37 ch04"));
        let second_last = &turns[turns.len() - 2].2;
        assert!(second_last.starts_with("turn 29"), "tail not verbatim: {second_last}");
        // The index opens the conversation, listing only filed turns.
        assert!(turns[0].2.contains("index of filed turns"));
        assert!(turns[0].2.contains("ch04\u{b7}1  user\u{2192} turn 0"));
        assert!(
            !turns[0].2.contains(&format!("\u{b7}{}  ", stats.indexed + 1)),
            "index leaked a verbatim-tail turn"
        );
    }

    #[test]
    fn short_conversations_keep_everything_verbatim() {
        let mut s = session(&[("user", "hi"), ("assistant", "hello")]);
        let stats = render_paged(&mut s, "ash-52", "hi", 1, "codex", "claude", None, TAIL_BUDGET_CHARS);
        assert_eq!(stats.indexed, 0);
        assert_eq!(stats.verbatim, 2);
        // Arrival card at the bottom, no index message.
        let turns = message_turns(&s);
        assert_eq!(turns.len(), 3); // 2 original + arrival
        assert!(turns[2].2.starts_with("[constant: taking over]"));
        assert!(turns[0].2 == "hi", "originals must stay first and verbatim");
        assert!(!turns.iter().any(|(_, _, t)| t.contains("index of filed")));
    }
}
