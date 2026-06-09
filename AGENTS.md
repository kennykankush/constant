# Constant Agent Instructions

Constant is a local-first continuity tool for agent CLIs: one conversation,
carried live between Codex, Claude Code, and OpenCode (Gemini as a source).

Default posture: preserve the product spine before building. Do not turn this
into a generic multi-agent dashboard, model router, usage tracker, or wrapper
library.

## Core Frame

```text
read one runtime's local session        (alembic loader -> neutral IR)
distill it to the conversation          (sanitize + redact + receipt)
record it                               (~/.constant/snapshots — the record)
materialize a target-native session     (alembic writer / registry step)
resume the target runtime natively      (claude -r / codex resume / opencode -s)
```

The claim is not native shared memory. The claim is portable conversation
continuity — and the neutral record is the durable artifact; native sessions
are reprintable projections of it.

## Invariants (do not regress)

1. **Originals are seeds, never targets.** A carry never overwrites the user's
   source session; Constant ping-pongs its own projection pair.
2. **All materializations are atomic** (temp sibling + fsync + rename). A
   failed or interrupted write must leave the previous file intact.
3. **Identity is declared before it is detected.** Resumed ids are tracked;
   fresh claude children get a minted `--session-id`; filesystem detection is
   the fenced (cwd + spawn-time) fallback, not the primary.
4. **Redaction applies to every carried string**, including nested tool
   payloads. Reasoning traces never cross runtimes.
5. **Lossiness is declared** — every carry prints a receipt (kept / dropped /
   redacted). No silent drops.
6. **Required registry steps are real errors.** Codex's sqlite row and
   OpenCode's `opencode import` must succeed or the carry fails loudly (with a
   fresh-launch fallback) — never a session that resume can't find.
7. **The host is a pass-through proxy, not a compositor.** One child,
   forwarded verbatim; only input is sniffed for the prefix; the status bar
   reserves a row instead of parsing output.
8. **The harness never dies mid-gesture.** Failed switches degrade (target
   resumed → target fresh → previous runtime); instant child exits are caught
   and recovered once.

## Before Editing

1. Read `README.md` (current behavior) and `CHANGELOG.md` (recent changes).
2. If touching a codec, inspect the runtime's REAL local session format first —
   the formats are undocumented and drift between releases; the round-trip
   tests are the early warning.
3. Treat `~/.claude`, `~/.codex`, `~/.gemini`, the OpenCode store, auth files,
   history files, and logs as sensitive. Never display raw tokens.
4. **Never run `gemini` in a directory whose store holds legacy-format chats**
   — its migration has deleted them. `gemini --version` only.
5. OpenCode is written ONLY through `opencode import`; never write its sqlite.

## Testing

```bash
cargo test                                  # unit + integration (isolated stores)
cargo clippy --all-targets -- -D warnings   # CI gate
```

Integration tests isolate every store via env (`HOME`, `CODEX_HOME`,
`CLAUDE_CONFIG_DIR`, `XDG_DATA_HOME`) — they must never touch real session
stores. Tests that need a real runtime binary (e.g. `opencode import`) skip
when it is absent.

## Language

Use "runtime" or "agent CLI" in user-facing language.

Consistent terms:

- Constant
- carry (public verb; "distillation" is the internal step)
- alembic (the still: loaders, sanitize/redact, writers)
- neutral IR / the record (snapshot volumes under `~/.constant/snapshots`)
- projection (a Constant-owned native session)
- trail (the lineage ledger), receipt (what a carry kept/dropped/redacted)
- source runtime / target runtime
