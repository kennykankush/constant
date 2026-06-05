# Constant

**One conversation. Any agent runtime.**

Constant is a local-first host for agent CLIs. Start a thread in Codex, switch to
Claude Code mid-conversation, then switch back without re-explaining the work.

```text
Codex changes.
Claude changes.
The conversation stays constant.
```

Constant is early software. It works by reading and writing local runtime session
files, so read the trust boundary below before using it on important sessions.

## What It Is

Constant runs one agent CLI at a time inside a native PTY. The child keeps its
normal terminal UI, colors, keybindings, and resume behavior. Constant only sits
outside it, watches for a tmux-style prefix key, and swaps the active runtime when
you ask.

```text
constant host codex

you talk to Codex
Ctrl-B c  ->  Claude Code continues the same thread
Ctrl-B x  ->  Codex continues it again
```

The switch is not a pasted summary. Constant reads the source runtime's local
session, distills it to the conversation, writes a target-native session, and
starts the target runtime with its own resume command.

## Install

Requires Rust, plus `codex` and/or `claude` on your `PATH`.

From this repository:

```bash
cargo install --path . --locked
constant --version
```

From GitHub:

```bash
cargo install --git https://github.com/kennykankush/constant --locked
constant --version
```

Or build a local release binary:

```bash
cargo build --release
./target/release/constant --version
```

Homebrew and prebuilt release binaries are planned, but not published yet.

## Quickstart

Host Codex:

```bash
constant host codex
```

Inside a hosted session, the default prefix is `Ctrl-B`.

| Keys | Action |
| --- | --- |
| `Ctrl-B` `c` | Switch to Claude Code, carrying the conversation |
| `Ctrl-B` `x` | Switch to Codex, carrying the conversation |
| `Ctrl-B` `:` | Open the Constant command line (`switch claude`, `quit`) |
| `Ctrl-B` `d` | Detach and exit cleanly |
| `Ctrl-B` `Ctrl-B` | Send a literal `Ctrl-B` to the child runtime |

If you are already inside tmux, pick another prefix:

```bash
constant host codex --prefix C-t
CONSTANT_PREFIX=C-t constant host codex
```

Show the switch lineage for the current directory:

```bash
constant trail
constant trail --all
```

## Trust Boundary

Constant does not claim shared native memory between agent CLIs. It reads visible
local session evidence and authors a new session that the target runtime can
resume.

Constant currently writes:

- `~/.claude/projects/<cwd-slug>/<id>.jsonl`
- `~/.claude/history.jsonl`
- `~/.codex/sessions/YYYY/MM/DD/rollout-<timestamp>-<id>.jsonl`
- `~/.codex/session_index.jsonl`
- `~/.codex/state_5.sqlite`
- `~/.constant/trail.jsonl`

Original source sessions are read as seeds and are not overwritten. Constant
maintains its own runtime projections for the carried thread, then keeps those
projections in sync as you switch back and forth.

Constant redacts common secrets from carried text and strips runtime scaffold, but
it still moves conversation content across a trust boundary. Do not use it on a
session if you are not comfortable with the target runtime reading that thread.

## What Carries

Carries:

- user and assistant conversation turns
- the working directory recorded by the source runtime
- a Constant trail name so the carried session is recognizable in `/resume`

Stripped on purpose:

- runtime scaffold such as system prompts, injected environment blocks, skill
  lists, plugin lists, and memory blocks
- common secrets such as API keys, GitHub tokens, Slack tokens, authorization
  headers, password/token assignments, and email addresses

Not carried:

- tool calls and tool results
- reasoning traces
- hidden provider state
- approvals, sandbox state, prompt cache state, or private runtime internals

For coding sessions, the narrative usually survives. The machine-level tool
history does not.

## How It Works

```text
        ┌────────────────── constant host ──────────────────┐
        │  real terminal                                    │
        │    input -> prefix interceptor -> PTY -> runtime   │
        │    screen <- child output  <- PTY <- runtime       │
        └───────────────────────────────────────────────────┘

on switch:
  source runtime session
      -> alembic reader
      -> neutral thread model
      -> sanitize + redact
      -> target-native session writer
      -> target runtime resume
```

The distillation layer is named `alembic`. It is the part that knows how to load
Codex and Claude Code session formats, strip them down to the portable
conversation, and materialize the target runtime's native session shape.

## Commands

Public commands:

```bash
constant host [codex|claude] [--prefix C-t]
constant trail [--all]
constant --version
```

Debug and inspection commands:

```bash
constant distill --from codex --to claude
constant distill --session <file> --to codex
constant keys
```

`constant distill` runs the codec without opening a hosted terminal. It still
writes a target-native session, so treat it as a write operation.

## Supported Runtimes

Current support:

- Codex CLI: validated against `0.137.x`
- Claude Code: validated against `2.1.x`

The session formats are private and can change between runtime releases. Constant
has round-trip tests for the current known shapes, but a runtime update can still
require a codec refresh.

Planned runtime targets include OpenCode, Aider, and Gemini CLI.

## When Not To Use Constant

Constant is probably the wrong tool if:

- you need a lossless transfer of tool calls, reasoning, or hidden runtime state
- you do not want any writes into `~/.claude` or `~/.codex`
- your source runtime is still generating the current turn
- your installed Codex or Claude Code version is outside the validated range
- you want a multi-agent dashboard, model router, API proxy, or terminal
  multiplexer

## Development

Useful checks:

```bash
cargo test
cargo clippy --all-targets -- -D warnings
cargo build --release
```

Project map:

```text
src/
  main.rs              CLI entrypoint
  runtime.rs           runtime launch/resume commands
  host.rs              PTY host, prefix handling, switch orchestration
  trail.rs             Constant-owned lineage ledger
  alembic/
    mod.rs             distill, sanitize, redact, active-session detection
    ir.rs              neutral session model
    formats/
      claude.rs        Claude session reader/writer
      codex.rs         Codex rollout reader/writer and sqlite registration
```

The low-level session codecs in `src/alembic/formats/` and the neutral IR are
vendored from [transession](https://github.com/inmzhang/transession) (MIT; see
`src/alembic/LICENSE.transession`). Constant adds the sanitize/redact pass,
native-resume hardening, stable runtime projections, trail naming, and the live
host.

## Documentation

- [Product vision](./PRODUCT.md)
- [Architecture](./docs/architecture.md)
- [How it works](./docs/how-it-works.md)
- [The alembic cartridge](./docs/the-cartridge.md)
- [Decisions and tradeoffs](./docs/decisions-and-tradeoffs.md)
- [Changelog](./CHANGELOG.md)

## License

MIT. See [LICENSE](./LICENSE).
