# Constant

One conversation. Any agent runtime.

Constant is a local-first dev tool for carrying conversation continuity across agent CLIs like Codex, Claude Code, OpenCode, Aider, and future runtimes.

The core idea is simple:

```text
Codex changes.
Claude changes.
OpenCode changes.
The conversation stays constant.
```

Constant does not try to merge the private native memory of every agent. That is not the real primitive. Instead, Constant owns a neutral outer thread state, reads local session artifacts from each runtime, compiles a continuation packet, and gives that packet to the next runtime so it can continue coherently.

```text
~/.codex/sessions/**/*.jsonl        ~/.claude/projects/**/*.jsonl
~/.codex/state_5.sqlite             ~/.claude/sessions/*.json
              \                     /
               \                   /
             runtime readers / adapters
                       |
               neutral thread model
                       |
             continuation packet compiler
                       |
           claude -p / codex exec / opencode
```

## Why This Exists

Right now, switching from Codex to Claude Code means starting a new conversation and re-explaining the state by hand. The real work is trapped inside each agent's local mansion: transcript files, session IDs, summaries, tool history, cwd, branch, changed files, and the emotional/thread texture of the conversation.

Constant turns that into a portable room state.

The first goal is not a multi-agent dashboard. It is not a model router. It is not another wrapper that merely runs several CLIs.

The first goal is:

```text
Take an active conversation from one agent runtime and continue it in another without manually explaining everything again.
```

## Product Promise

```bash
constant latest --from codex --to claude --dry-run
constant packet --from claude --session latest > continuation.md
constant run --from codex --to claude
```

The output is a continuation packet:

```md
# Constant Continuation Packet

You are continuing a conversation that began in Codex.

## Current Goal
...

## What Happened So Far
...

## Recent Raw Turns
...

## Decisions
...

## Workspace State
cwd:
branch:
changed files:

## Runtime Notes
The prior runtime was Codex. You are now Claude Code.
Continue from this packet faithfully.
```

## What Constant Is

- A cross-runtime conversation continuity layer.
- A packet compiler for agent handoff.
- A local reader for `.claude`, `.codex`, and later other runtime state folders.
- A neutral thread model above provider-specific session formats.
- A safe bridge that starts read-only and makes every injected packet auditable.

## What Constant Is Not

- Not a claim that Claude and Codex literally share hidden session memory.
- Not a replacement for Claude Code, Codex, OpenCode, or Aider.
- Not an API proxy.
- Not a multi-model voting dashboard.
- Not a usage tracker, though it reuses some of the same local substrate Burnrate studied.

## First Milestone

Build the smallest proof:

```bash
constant packet --from codex --latest
```

It should:

1. Find the latest Codex session.
2. Read `~/.codex/state_5.sqlite` for metadata.
3. Read the matching rollout JSONL for user/assistant/tool events.
4. Normalize the useful thread state.
5. Write `.constant/packets/<timestamp>-codex-to-claude.md`.

Then manually run:

```bash
claude -p "$(cat .constant/packets/<timestamp>-codex-to-claude.md)"
```

If Claude can accurately say where the prior Codex conversation left off, the primitive works.

## Current Prototype

The first local proof is now a dependency-free Node CLI.

```bash
npm exec -- constant doctor
npm exec -- constant list --runtime codex
npm exec -- constant packet --from codex --latest --to claude
```

Direct Node invocation works too:

```bash
node ./bin/constant.js doctor
node ./bin/constant.js list --runtime codex
node ./bin/constant.js packet --from codex --latest --to claude
```

The packet command writes:

```text
.constant/packets/<timestamp>-codex-to-claude.md
```

The implementation stays read-only against runtime homes. It reads Codex rollout JSONL, joins `state_5.sqlite` metadata through the local `sqlite3` binary when available, normalizes user/assistant/tool events, and writes Constant-owned artifacts under `.constant/`.

Manual continuation is still the V0 path:

```bash
claude -p "$(cat .constant/packets/<timestamp>-codex-to-claude.md)"
```

Start with [PRODUCT.md](./PRODUCT.md) before expanding the implementation.
