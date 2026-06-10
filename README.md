<h1 align="center">Constant</h1>

<p align="center">
  <strong>One conversation. Any agent runtime.</strong>
</p>

<p align="center">
  <img alt="status: alpha" src="https://img.shields.io/badge/status-alpha-f59e0b?style=flat-square">
  <img alt="built with Rust" src="https://img.shields.io/badge/built%20with-Rust-b7410e?style=flat-square&logo=rust&logoColor=white">
  <img alt="runtimes: Codex, Claude, OpenCode + Gemini source" src="https://img.shields.io/badge/runtimes-Codex%20%2B%20Claude%20%2B%20OpenCode%20%2B%20Gemini%20(src)-111827?style=flat-square">
  <img alt="local first" src="https://img.shields.io/badge/local--first-no%20API%20proxy-10b981?style=flat-square">
  <img alt="license: MIT" src="https://img.shields.io/badge/license-MIT-2563eb?style=flat-square">
</p>

<p align="center">
  <a href="#install">Install</a> ·
  <a href="#quickstart">Quickstart</a> ·
  <a href="#the-record">The Record</a> ·
  <a href="#headless-cli">Headless CLI</a> ·
  <a href="#trust-boundary">Trust Boundary</a> ·
  <a href="#how-it-works">How It Works</a> ·
  <a href="#development">Development</a>
</p>

Constant is a local-first continuity tool for agent CLIs. Start a thread in
Codex, carry it to Claude Code or OpenCode, then carry it back without
re-explaining the work.

```text
Codex changes.
Claude changes.
The conversation stays constant.
```

Constant is early software. It works by reading and writing local runtime session
files, so read the trust boundary below before using it on important sessions.

## What It Is

Constant has three surfaces:

- `constant host`: run one agent CLI at a time inside a native PTY and switch the
  live runtime with a tmux-style prefix key. A persistent status bar shows the
  active runtime and your place in the thread.
- `constant carry`: headless carry into a target runtime's native session and
  print the resume command.
- the record: every carry first writes the distilled conversation as a neutral
  snapshot under `~/.constant/snapshots/`, so any hop can be listed
  (`constant snapshots`), reprinted (`constant restore`), or re-hosted
  (`constant resume`) — even if the native session files are later deleted.

In host mode, the child keeps its normal terminal UI, colors, keybindings, and
resume behavior. Constant sits outside it, watches for a prefix key, and swaps
the active runtime when you ask.

```text
constant host codex

you talk to Codex
Ctrl-B c  ->  Claude Code continues the same thread
Ctrl-B o  ->  OpenCode continues it
Ctrl-B x  ->  Codex takes it back
```

The switch is not a pasted summary. Constant reads the source runtime's local
session, distills it to the conversation, records a snapshot, writes a
target-native session, and starts the target runtime with its own resume
command. Every carry prints a receipt of exactly what moved:

```text
cobalt-37 · fix-the-bug · ch02 codex → claude · continue ·
carried 14 turns · dropped 6 tool events · 2 redactions
```

The same carry path is available without opening a hosted terminal:

```bash
constant carry --from codex --to claude
# carried -> claude session <id>
# carried 14 turns · dropped 6 tool events · 2 redactions
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
codex --version     # optional, for Codex support
claude --version    # optional, for Claude Code support
opencode --version  # optional, for OpenCode support
gemini --version    # optional, for Gemini (carry source)
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
| `Ctrl-B` `x` | Continue in Codex |
| `Ctrl-B` `X` | Create a new Codex continuation |
| `Ctrl-B` `o` | Continue in OpenCode |
| `Ctrl-B` `O` | Create a new OpenCode continuation |
| `Ctrl-B` `t` | Toggle the cockpit — the chapter graph, with actions: `c`/`x`/`o` switch from inside it, `r` rename |
| `Ctrl-B` `:` | Open the Constant command line (`switch claude`, `rename auth bug`, `quit`; Tab completes, ↑/↓ recall history) |
| `Ctrl-B` `d` | Quit Constant (the hosted CLI exits with it) |
| `Ctrl-B` `Ctrl-B` | Send a literal `Ctrl-B` to the child runtime |

A status bar on the bottom row shows the hosted runtime, the thread position
(`ch04·fix-the-bug`), and the prefix keys. The child is simply told the terminal
is one row shorter — Constant stays a pass-through proxy, not a compositor.
Disable it with `--no-bar`.

If you are already inside tmux, pick another prefix:

```bash
constant host codex --prefix C-t
CONSTANT_PREFIX=C-t constant host codex
```

Every conversation gets a stable **handle** (`cobalt-37` — one color word + a
2-digit tail, pinned in the ledger, collision-proof by registry) and a
**title** (your `rename` wins forever; otherwise a runtime-generated title is
harvested when one exists; otherwise the first contentful words). Carries are
**chapters** (`ch04`): each chapter is one runtime's turn narrating the thread.

```bash
constant rename auth bug          # name it; native pickers are re-stamped
constant resume cobalt-37         # handles are never ambiguous
```

Re-host a conversation later, straight from the ledger:

```bash
constant resume                 # newest conversation in this directory
constant resume fix-the-bug     # match by slug or conversation id
constant resume --list --all    # see everything resumable
```

`resume` wakes the conversation's latest projection live in the harness. If
every native projection has been deleted, it reprints one from the latest
record snapshot first.

See every agent conversation alive on the machine right now (any runtime,
hosted or not — read-only):

```bash
constant ps
# live agent sessions (14):
#   claude   01-15:29:00   fix-the-bug   fefd68b4-5ee…   ~/dev/fantopy-hadi
#   codex    01-15:27:35   -             019e9a25-91e…   ~/dev/belvedere
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

Move a conversation to another machine — the record travels, native sessions
reprint on arrival:

```bash
constant pack cobalt-37                  # → cobalt-37.constant-pack.json
# copy the file to the other machine, then:
constant unpack cobalt-37.constant-pack.json
constant resume cobalt-37                # wakes from the record, same handle
```

## The Record

Every carry writes the distilled conversation to
`~/.constant/snapshots/<conversation>/chNN-from-<runtime>.json` — atomically,
owner-only, post-redaction, BEFORE the native copy is materialized. The neutral
snapshot is the durable record; native sessions are reprintable projections of
it.

```bash
constant snapshots            # list record volumes for this directory
constant snapshots --all
constant restore <snapshot>   # reprint a fresh native session from any hop
constant restore <snapshot> --to opencode
```

`restore` always mints a new session — it never overwrites anything, least of
all the record. Restores are logged to the trail, so lineage stays joined. If a
runtime update ever rejects Constant's sessions, or a cleanup deletes them, the
conversation reprints from its record.

## Headless CLI

Use the headless commands when you want continuity without hosting an interactive
terminal.

List carryable sessions:

```bash
constant sessions
constant sessions --from gemini --all
constant sessions --from opencode --titles   # OpenCode titles come from its own db
constant sessions --all --json
```

By default, `sessions` scopes to the current directory. `--all` scans all known
runtime stores. `--titles` reads transcripts to derive a preview title, so it is
slower on large stores. In text output, `·` marks a session known to be empty
when titles were requested.

Preview a carry without writing anything:

```bash
constant carry --from codex --to claude --dry-run
constant carry --session <path-or-session-id> --to opencode --dry-run --json
```

Carry into the target runtime's native session and print the resume command:

```bash
constant carry --from codex --to claude
constant carry --session ses_…  --to codex       # OpenCode ids resolve automatically
constant carry --session <gemini session file> --to claude
```

By default, `carry` continues the current target projection for the conversation:
if `codex[1]` already has `claude[1.1]`, another carry to Claude updates that
same Claude session. Use `--new` when you want a separate target continuation:

```bash
constant carry --from codex --to claude --new
# codex[1] -> claude[1.2]
```

Carry tool history too (experimental):

```bash
constant carry --from codex --to opencode --with-tools
```

`--with-tools` (also on `host` and `resume`) carries tool calls and results
across the boundary instead of dropping them. Every string in their payloads is
redacted, oversized tool outputs are capped, and reasoning is never carried.
The receipt distinguishes `kept N tool events` from `dropped N tool events`.

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
- the OpenCode store, via `opencode import` only (its own supported,
  validating entry point — Constant never writes OpenCode's sqlite directly)
- `~/.constant/trail.jsonl`, `~/.constant/snapshots/`, `~/.constant/cache/`

Gemini support is currently **read-only**: Constant loads gemini sessions as
carry sources and never writes into `~/.gemini` (carrying INTO gemini is gated
until its current session-storage behavior is verified — gemini's own store
migration has been observed deleting legacy-format chats, so Constant refuses
to guess).

All writes are atomic (temp file + rename): a crash mid-switch cannot leave a
torn session, and the previous projection always survives a failed write.

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
- `constant trail` / `constant route` / `constant snapshots`: read the ledger
- `constant sessions`: reads session metadata; `--titles` also reads transcripts
- `constant carry --dry-run`: reads and distills, writes nothing
- `constant carry`: writes a record snapshot + a target-native projection, updates the trail
- `constant restore`: writes a fresh native projection from a record volume
- `constant resume`: hosts an existing projection; writes only when you switch
- `constant host`: writes only when you switch runtimes
- `constant export`: writes only the requested `--out` file, or stdout

## What Carries

Carries:

- user and assistant conversation turns
- the working directory recorded by the source runtime
- a Constant trail name so the carried session is recognizable in the target's
  resume picker
- with `--with-tools`: tool calls and results (redacted, size-capped)

Stripped on purpose:

- runtime scaffold such as system prompts, injected environment blocks, skill
  lists, plugin lists, and memory blocks
- secrets: API keys, GitHub/Slack tokens, AWS access keys, private-key blocks,
  JWTs, authorization headers, password/token assignments, and email addresses

Not carried:

- tool calls and tool results (unless `--with-tools`)
- reasoning traces (never — model-internal)
- hidden provider state
- approvals, sandbox state, prompt cache state, or private runtime internals

Every carry prints a receipt of what was kept, dropped, and redacted — the
lossiness is declared, never silent.

## How It Works

```text
        ┌────────────────── constant host ──────────────────┐
        │  real terminal                                    │
        │    input -> prefix interceptor -> PTY -> runtime   │
        │    screen <- child output  <- PTY <- runtime       │
        │    [ status bar on a protected bottom row ]        │
        └───────────────────────────────────────────────────┘

on switch:
  source runtime session
      -> alembic reader (codex | claude | gemini | opencode)
      -> neutral thread model
      -> sanitize + redact            (receipt)
      -> record snapshot              (~/.constant/snapshots/…)
      -> target-native session writer (codex | claude | opencode)
      -> target runtime resume
```

The distillation layer is named `alembic`. It is the part that knows how to load
each runtime's session format, strip it down to the portable conversation, and
materialize the target runtime's native session shape. Runtimes plug in as
codecs around the neutral model — N runtimes mean N codecs, not N² translators.

## Commands

Public commands:

```bash
constant host [codex|claude|opencode] [--prefix C-t] [--with-tools] [--no-bar]
constant resume [QUERY] [--in RT] [--list] [--all] [--prefix C-t] [--with-tools] [--no-bar]
constant carry --to codex|claude|opencode [--from RT | --session <path-or-id>] [--json] [--dry-run] [--debug] [--new] [--with-tools]
constant sessions [--from RT] [--all] [--titles] [--json]
constant rename [--of HANDLE] NEW NAME...
constant pack HANDLE [--out FILE]
constant unpack FILE
constant ps [--json]
constant snapshots [--all]
constant restore <snapshot> [--to RT] [--json]
constant export (--from RT | --session <path-or-id>) [--out FILE]
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

- Codex CLI: full (host, carry in/out, resume) — validated against `0.139.x`
- Claude Code: full — validated against `2.1.x`
- OpenCode: full — validated against `1.14.x`; reads via `opencode export`,
  writes via `opencode import`, resumes via `opencode -s <id>`
- Gemini CLI: carry **source** — validated against `0.40.x`; existing gemini
  conversations load, list, and carry into the other runtimes

The session formats are private and can change between runtime releases. Constant
has round-trip tests for the current known shapes, but a runtime update can still
require a codec refresh — and if one ever rejects Constant's sessions, the
conversation reprints from its record snapshot.

Planned: carrying into Gemini; Aider.

## When Not To Use Constant

Constant is probably the wrong tool if:

- you need a lossless transfer of reasoning traces or hidden runtime state
- you do not want any writes into `~/.claude`, `~/.codex`, or the OpenCode store
- your source runtime is still generating the current turn
- your installed runtime version is outside the validated range
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
  main.rs              CLI entrypoint (carry, resume, restore, snapshots, …)
  runtime.rs           runtime launch/resume commands
  host.rs              PTY host, prefix handling, status bar, switch orchestration
  trail.rs             Constant-owned lineage ledger + record snapshot paths
  alembic/
    mod.rs             distill, sanitize, redact, receipts, discovery, registry steps
    ir.rs              neutral session model
    sha256.rs          minimal SHA-256 (gemini keys its store by sha256(cwd))
    formats/
      claude.rs        Claude session reader/writer
      codex.rs         Codex rollout reader/writer
      gemini.rs        Gemini session reader (carry source)
      opencode.rs      OpenCode export-shape reader/writer (import/export based)
```

The low-level Codex/Claude session codecs and the neutral IR are
vendored from [transession](https://github.com/inmzhang/transession) (MIT; see
`src/alembic/LICENSE.transession`). Constant adds the sanitize/redact pass,
native-resume hardening, stable runtime projections, the record, trail naming,
the Gemini and OpenCode codecs, and the live host.

## License

MIT.
