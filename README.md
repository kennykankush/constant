<h1 align="center">Constant</h1>

<p align="center">
  <strong>One conversation. Any agent runtime.</strong>
</p>

<p align="center">
  <img alt="status: alpha" src="https://img.shields.io/badge/status-alpha-f59e0b?style=flat-square">
  <img alt="built with Rust" src="https://img.shields.io/badge/built%20with-Rust-b7410e?style=flat-square&logo=rust&logoColor=white">
  <img alt="runtimes: Codex and Claude Code" src="https://img.shields.io/badge/runtimes-Codex%20%2B%20Claude-111827?style=flat-square">
  <img alt="local first" src="https://img.shields.io/badge/local--first-no%20API%20proxy-10b981?style=flat-square">
  <img alt="license: MIT" src="https://img.shields.io/badge/license-MIT-2563eb?style=flat-square">
</p>

<p align="center">
  <a href="#install">Install</a> ·
  <a href="#quickstart">Quickstart</a> ·
  <a href="#headless-cli">Headless CLI</a> ·
  <a href="#trust-boundary">Trust Boundary</a> ·
  <a href="#how-it-works">How It Works</a> ·
  <a href="#development">Development</a>
</p>

Constant is a local-first continuity tool for agent CLIs. Start a thread in
Codex, carry it to Claude Code, then carry it back without re-explaining the
work.

```text
Codex changes.
Claude changes.
The conversation stays constant.
```

Constant is early software. It works by reading and writing local runtime session
files, so read the trust boundary below before using it on important sessions.

## What It Is

Constant has two surfaces:

- `constant host`: run one agent CLI at a time inside a native PTY and switch the
  live runtime with a tmux-style prefix key.
- `constant carry`: headless carry into a target runtime's native session and
  print the resume command.

In host mode, the child keeps its normal terminal UI, colors, keybindings, and
resume behavior. Constant sits outside it, watches for a prefix key, and swaps
the active runtime when you ask.

```text
constant host codex

you talk to Codex
Ctrl-B c  ->  Claude Code continues the same thread
Ctrl-B x  ->  Codex continues it again
```

The switch is not a pasted summary. Constant reads the source runtime's local
session, distills it to the conversation, writes a target-native session, and
starts the target runtime with its own resume command.

The same carry path is available without opening a hosted terminal:

```bash
constant carry --from codex --to claude
# carried -> claude session <id>
# resume with: claude -r <id>
```

## Install

If you are on macOS and use Homebrew, use this:

```bash
brew tap kennykankush/constant https://github.com/kennykankush/constant
brew install kennykankush/constant/constant
constant --version
constant doctor
```

That is the recommended install path. The explicit
`kennykankush/constant/constant` name avoids ambiguity with any other formula
named `constant`.

You also need the agent CLIs you want to switch between:

```bash
codex --version   # optional, for Codex support
claude --version  # optional, for Claude Code support
```

### Alternative: curl installer

Use this if you do not want Homebrew, or you are on Linux x86_64:

```bash
curl -fsSL https://raw.githubusercontent.com/kennykankush/constant/main/scripts/install.sh | sh
constant --version
constant doctor
```

The installer downloads the latest prebuilt binary, verifies its checksum,
smoke-tests it, and installs it to `~/.local/bin`. If `~/.local/bin` is not on
your `PATH`, it prints the exact export line to add.

Supported prebuilt targets:

- macOS Apple Silicon
- macOS Intel
- Linux x86_64

### Developers: install from source

Use this only if you are hacking on Constant or want to build directly from the
repo:

```bash
cargo install --git https://github.com/kennykankush/constant --locked
```

From a local checkout:

```bash
cargo install --path . --locked
cargo build --release
./target/release/constant --version
```

## Quickstart

Check your local runtime setup and current project state:

```bash
constant doctor
constant status
```

Host Codex:

```bash
constant host codex
```

Inside a hosted session, the default prefix is `Ctrl-B`.

| Keys | Action |
| --- | --- |
| `Ctrl-B` `c` | Continue in Claude Code, refreshing the existing Claude projection if one exists |
| `Ctrl-B` `C` | Create a new Claude continuation |
| `Ctrl-B` `x` | Continue in Codex, refreshing the existing Codex projection if one exists |
| `Ctrl-B` `X` | Create a new Codex continuation |
| `Ctrl-B` `:` | Open the Constant command line (`switch claude`, `new claude`, `quit`) |
| `Ctrl-B` `d` | Detach and exit cleanly |
| `Ctrl-B` `Ctrl-B` | Send a literal `Ctrl-B` to the child runtime |

If you are already inside tmux, pick another prefix:

```bash
constant host codex --prefix C-t
CONSTANT_PREFIX=C-t constant host codex
```

Show the current Constant projections for this directory:

```bash
constant trail
constant route
constant trail --all
```

`constant trail` shows the current projection per runtime. If the same projection
was updated multiple times, it is shown as `synced Nx` instead of repeated as a
new conversation. To inspect the raw append-only switch ledger:

```bash
constant trail --events
```

`constant route` is the debug view. It reconstructs the fork graph Constant knows
about and labels projections with readable aliases like `codex[1]` and
`claude[1.1]`.

## Headless CLI

Use the headless commands when you want continuity without hosting an interactive
terminal.

List carryable sessions:

```bash
constant sessions
constant sessions --from codex --titles
constant sessions --all --json
```

By default, `sessions` scopes to the current directory. `--all` scans all known
runtime stores. `--titles` reads transcripts to derive a preview title, so it is
slower on large stores. In text output, `·` marks a session known to be empty
when titles were requested.

Preview a carry without writing anything:

```bash
constant carry --from codex --to claude --dry-run
constant carry --from codex --to claude --dry-run --debug
constant carry --session <path-or-session-id> --to codex --dry-run --json
```

Carry into the target runtime's native session and print the resume command:

```bash
constant carry --from codex --to claude
constant carry --session <path-or-session-id> --to codex
```

By default, `carry` continues the current target projection for the conversation:
if `codex[1]` already has `claude[1.1]`, another carry to Claude updates that
same Claude session. Use `--new` when you want a separate target continuation:

```bash
constant carry --from codex --to claude --new
# codex[1] -> claude[1.2]
```

`distill` is kept as an alias for `carry`, but `carry` is the public verb.

Export the distilled, redacted neutral IR:

```bash
constant export --from codex --out thread.constant.json
constant export --session <path-or-session-id>
```

`export` writes to `--out` when provided, otherwise it prints JSON to stdout. It
refuses to overwrite the source session.

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

F1 invariant: original runtime sessions are seeds, not targets. `host` and
`carry` write or update Constant-owned projection sessions, then resume those
projections. They do not overwrite the original source session.

Command write behavior:

- `constant doctor`: reads CLI/version/store presence only
- `constant status`: reads runtime readiness, latest sessions, and the trail ledger
- `constant trail`: reads the Constant trail ledger
- `constant route`: reads the Constant trail ledger and target projection paths
- `constant sessions`: reads session metadata; `--titles` also reads transcripts
- `constant carry --dry-run`: reads and distills, writes nothing
- `constant carry`: writes a target-native projection and updates the trail
- `constant host`: writes only when you switch runtimes
- `constant export`: writes only the requested `--out` file, or stdout

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
constant carry --to codex|claude [--from codex|claude | --session <path-or-id>] [--json] [--dry-run] [--debug] [--new]
constant sessions [--from codex|claude] [--all] [--titles] [--json]
constant export (--from codex|claude | --session <path-or-id>) [--out FILE]
constant doctor [--json]
constant status [--all]
constant trail [--all] [--events]
constant route [--all] [--session <path-or-id>]
constant --version
```

Debug and inspection commands:

```bash
constant distill --from codex --to claude
constant keys
```

`constant distill` is the older name for `constant carry`; the internal cartridge
still calls this step distillation. `constant keys` prints raw key bytes for
debugging prefix behavior.

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

## License

MIT.
