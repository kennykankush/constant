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
    /// Turns kept verbatim in the tail (surfaced by `constant audit`).
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

/// Where the verbatim tail begins (0-based turn index): the newest turns whose
/// combined size fits the budget, never fewer than [`MIN_TAIL_TURNS`]. The one
/// tail-selection function — both [`render_paged`] and [`render_stats`] call it,
/// so the audit's numbers and the real render can never disagree.
fn tail_start_for(turns: &[(usize, String, String)], tail_budget_chars: usize) -> usize {
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
    tail_start
}

/// What a paged render WOULD do to this session, computed WITHOUT mutating it:
/// how many newest turns stay verbatim vs how many older turns get filed into
/// the index. The read-only half of the renderer — `constant audit` uses it to
/// show, per hop, what the pager keeps on the desk vs files in the cabinet.
pub fn render_stats(session: &UniversalSession, tail_budget_chars: usize) -> RenderStats {
    let turns = message_turns(session);
    if turns.is_empty() {
        return RenderStats::default();
    }
    let tail_start = tail_start_for(&turns, tail_budget_chars);
    RenderStats {
        verbatim: turns.len() - tail_start,
        indexed: tail_start,
    }
}

/// First line of a turn, clipped for the index (also the trail explorer's
/// turn-index rows — one previewer, so the two views never drift).
pub(crate) fn preview(text: &str, max: usize) -> String {
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

    // Choose the verbatim tail (shared math with `render_stats`, so the audit's
    // numbers and the real render can never drift apart).
    let tail_start = tail_start_for(&turns, tail_budget_chars);
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
    let arrival = arrival_card(name, handle, chapter, from_label, to_label, anchor, &index_note);

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

/// The "[constant: taking over]" handover card the arriving mind reads. Shared
/// by [`render_paged`] and [`render_delta`] so the two cards can never drift.
#[allow(clippy::too_many_arguments)]
fn arrival_card(
    name: &str,
    handle: &str,
    chapter: u32,
    from_label: &str,
    to_label: &str,
    anchor: Option<&str>,
    index_note: &str,
) -> String {
    format!(
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
    )
}

/// Build a session holding ONLY the foreign segment to APPEND to a runtime's
/// existing projection on a return-switch: the turns at/after `already_present`
/// (a 0-based turn count, in [`message_turns`] order) plus a fresh arrival card.
///
/// The returning mind's own earlier turns stay in its native projection,
/// byte-for-byte — this is just what happened elsewhere while it was away, so
/// the new turns are recent and kept VERBATIM (never indexed). Returns `None`
/// when there is nothing new (`already_present` is at or past the last turn).
#[allow(clippy::too_many_arguments)]
pub fn render_delta(
    source: &UniversalSession,
    already_present: usize,
    handle: &str,
    name: &str,
    chapter: u32,
    from_label: &str,
    to_label: &str,
    anchor: Option<&str>,
) -> Option<UniversalSession> {
    let turns = message_turns(source);
    if already_present >= turns.len() {
        return None;
    }
    let new_count = turns.len() - already_present;

    // Every original event from the first NEW turn onward (tool/reasoning events
    // riding between kept turns stay interleaved, same rule as the paged tail).
    let cut = nth_turn_event_index(source, already_present);
    let mut events: Vec<SessionEvent> = source.events[cut..].to_vec();

    let index_note = format!(
        "The {new_count} turn{s} above {are} what happened in {from_label} while you \
         were away \u{2014} verbatim and complete; everything before them is your own \
         prior thread, untouched.\n",
        s = if new_count == 1 { "" } else { "s" },
        are = if new_count == 1 { "is" } else { "are" },
    );
    let card = arrival_card(name, handle, chapter, from_label, to_label, anchor, &index_note);
    events.push(synthetic_user(&card));

    let mut delta = UniversalSession::new(source.metadata.session_id.clone());
    delta.metadata = source.metadata.clone();
    delta.events = events;
    Some(delta)
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
    fn render_stats_is_read_only_and_agrees_with_render_paged() {
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

        // render_stats must NOT mutate the session (it's the audit's read-only eye).
        let s = session(&refs);
        let stats = render_stats(&s, 3_000);
        assert_eq!(s.events.len(), 30, "render_stats mutated the session");
        assert_eq!(stats.indexed + stats.verbatim, 30);
        assert!(stats.verbatim >= MIN_TAIL_TURNS);

        // …and it must agree with what the real paged render actually does.
        let mut rendered_session = session(&refs);
        let rendered = render_paged(
            &mut rendered_session,
            "h",
            "n",
            1,
            "codex",
            "claude",
            None,
            3_000,
        );
        assert_eq!(
            (stats.verbatim, stats.indexed),
            (rendered.verbatim, rendered.indexed),
            "audit numbers drifted from the real render"
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

    #[test]
    fn render_delta_returns_only_new_turns_plus_card() {
        let s = session(&[
            ("user", "one"),
            ("assistant", "two"),
            ("user", "three"),
            ("assistant", "four"),
        ]);
        // 2 turns already in the projection → delta = turns 3 & 4 + arrival card.
        let delta = render_delta(&s, 2, "ash-52", "thread", 3, "codex", "claude", None)
            .expect("delta should exist");
        let turns = message_turns(&delta);
        assert_eq!(turns.len(), 3, "two new turns + the arrival card");
        assert_eq!(turns[0].2, "three");
        assert_eq!(turns[1].2, "four");
        assert!(turns[2].2.starts_with("[constant: taking over]"), "no card: {}", turns[2].2);
        // The card declares the new turns verbatim, never references recall.
        assert!(turns[2].2.contains("while you"));
        assert!(!turns[2].2.contains("constant recall"));
        // Source is untouched (render_delta is non-mutating).
        assert_eq!(message_turns(&s).len(), 4);
        // Nothing new → None.
        assert!(render_delta(&s, 4, "h", "n", 1, "codex", "claude", None).is_none());
        assert!(render_delta(&s, 9, "h", "n", 1, "codex", "claude", None).is_none());
    }
}
