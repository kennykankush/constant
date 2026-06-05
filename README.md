# Constant

**One conversation. Any agent runtime.**

Constant is a local-first terminal harness that hosts an agent CLI (Codex, Claude
Code) and lets you **switch the active runtime mid-conversation** — the room moves,
the work doesn't reset.

```text
Codex changes.
Claude changes.
The conversation stays constant.
```

You're talking to Codex. You hit `Ctrl-B c`. You're now in Claude Code, continuing
the *same* conversation — it has the whole thread, no re-explaining. Hit `Ctrl-B x`
and you're back in Codex. One conversation, two (or more) interchangeable minds.

---

## How it works

Constant is the outer process; the agent CLI runs *inside* it in a PTY, fully
native (colors, TUI, keys all pass through). A tmux-style prefix key drops you into
Constant's control layer to switch runtimes.

```text
        ┌──────────────── constant host ─────────────────┐
          you type ─▶ [interceptor] ─▶ PTY ─▶ codex (native)
          screen   ◀──────────────────── PTY ◀── codex
        └─────────────────────────────────────────────────┘
                    Ctrl-B c  → switch to claude
                    Ctrl-B x  → switch to codex

   on switch:  read the session the runtime is in
               │  distill → neutral IR → sanitize → target's native format
               ▼
               resume the target natively (claude -r / codex resume)
```

The switch doesn't paste a summary or a briefing — it **transcodes the actual
session into the target's native format and resumes it**, so the next runtime
reads the conversation as its own scrollback. The agents are stateless between
launches; their "memory" is just a transcript file on disk, and Constant writes
that file. (See [PRODUCT.md](./PRODUCT.md) for the vision and [AGENTS.md](./AGENTS.md)
for the design spine.)

The distillation layer is named **alembic** — it distills a session down to the
pure conversation: runtime scaffold stripped, secrets redacted, tool/reasoning
noise dropped, then re-crystallized in the target's format.

## Quickstart

Requires Rust, plus `codex` and/or `claude` on your `PATH`.

```bash
cargo build --release
./target/release/constant host codex      # host codex inside Constant
```

Inside a hosted session, the prefix is **`Ctrl-B`** (like tmux):

| Keys            | Action                                            |
| --------------- | ------------------------------------------------- |
| `Ctrl-B` `c`    | switch to **claude**, carrying the conversation   |
| `Ctrl-B` `x`    | switch to **codex**, carrying the conversation    |
| `Ctrl-B` `:`    | command line (`switch claude`, `quit`)            |
| `Ctrl-B` `d`    | detach (exit the harness cleanly)                 |
| `Ctrl-B` `Ctrl-B` | send a literal `Ctrl-B` to the child            |

Inside tmux (which also uses `Ctrl-B`), pick another prefix:

```bash
constant host codex --prefix C-t       # or: CONSTANT_PREFIX=C-t constant host codex
```

## Commands

```bash
constant host [codex|claude] [--prefix C-t]   # the harness (default: codex)
constant distill --from codex --to claude     # transcode the latest convo (no launch)
constant distill --session <file> --to codex  #   …or a specific session file
constant keys                                  # raw key probe (debug input encodings)
```

`constant distill` is the codec on its own — handy for inspecting what a switch
would carry, with no TTY and no model calls.

## What carries (and what doesn't)

- **Carries cleanly:** the conversation — every user and assistant turn — verbatim.
- **Stripped on purpose:** runtime scaffold (system prompts, skill/plugin lists,
  memory blocks) and secrets (API keys, tokens, emails are redacted).
- **Dropped (lossy across runtimes):** tool calls, tool results, and reasoning.
  The *narrative* of a coding session survives; the agentic tool history does not —
  tool schemas aren't 1:1 between runtimes.

Each conversation lives as a **stable pair** of ids — one per runtime — that a
switch reuses and keeps in sync, so ping-ponging doesn't spawn a pile of new
sessions. Both incarnations show up in each CLI's own `/resume`.

## Honest limits

- **Not read-only.** Constant writes synthetic sessions into `~/.claude` and
  `~/.codex` (and codex's `state_5.sqlite`). It never displays raw secrets and
  redacts them from carried text, but it does author sessions in the runtime homes.
- **Private formats, version-fragile.** Session schemas are undocumented and move
  between releases. Verified against `codex 0.137.x` / `claude 2.1.x`; a runtime
  update can require a codec refresh.
- **Mid-turn switches** carry up to the last *persisted* turn — if you switch while
  a runtime is still generating, that in-flight turn isn't on disk yet.
- **Conversation-only.** Tool/reasoning history is intentionally not carried (yet).

## What Constant is not

- Not a claim that the runtimes share hidden native memory — it reads visible local
  session evidence and re-authors it for the next runtime.
- Not a multi-agent dashboard, model router, or API proxy.
- Not a terminal multiplexer — it hosts *one* runtime at a time and passes its
  output through untouched (no compositing), which is why it stays small.

## Architecture

```text
src/
  main.rs              CLI: host / distill / keys
  runtime.rs           runtime defs + fresh/resume launch commands
  host.rs              the PTY harness: raw-input FSM, prefix interception,
                       switch orchestration, stable-pair thread map, terminal restore
  alembic/
    mod.rs             distill: find active session, sanitize (strip + redact),
                       transcode, native resume, codex /resume visibility
    ir.rs              neutral session model
    formats/{claude,codex}.rs   per-runtime readers + writers
    LICENSE.transession
```

## Attribution

The low-level session codecs in `src/alembic/formats/` and the neutral IR are
vendored from [transession](https://github.com/inmzhang/transession) (MIT — see
`src/alembic/LICENSE.transession`). Alembic adds the sanitize/redact pass, the
native-resume schema matching, and the stable-pair switching the harness needs.

## Status

Early. The codex ↔ claude live switch works end to end — verified carrying real
conversations both directions, resumable from each CLI's own `/resume`. More
runtimes (OpenCode, Aider, Gemini) and carrying tool history are next.
