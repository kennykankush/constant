# Constant Product Vision

Status: founding vision document (2026-05). The product has since shipped and
evolved past the "continuation packet" mechanics described below: Constant is
now a live PTY harness with a tmux-style runtime switch, a neutral-IR carry
pipeline (`alembic`), a durable per-hop record under `~/.constant/snapshots/`,
and four runtimes (Codex, Claude Code, OpenCode full; Gemini as carry source).
See `README.md` for current behavior. This document is preserved as the
original product thinking — the *why* still stands; the *how* moved on.

Constant was born in Warroom from a specific frustration: a conversation can become rich inside Codex, but if Hadi wants Claude Code's continuation, he has to start a new room and explain everything again. The same is true in the other direction. The agent runtime changes, and the thread breaks.

The raw instinct:

> What if it is one conversation, but the active runtime can change?

Not model switching. Runtime switching.

Not "ask Claude and Codex side by side." Not "multi-agent dashboard." Not "one wrapper command for every CLI." The deeper idea is that the conversation should be portable. Codex, Claude Code, OpenCode, Aider, Gemini CLI, and future runtimes should be able to receive enough shared state to continue the same thread.

## Experience Promise

The user should feel:

```text
I was just talking to Codex.
Now Claude Code can pick up the same thread.
Now Codex can take it back.
The room moved. The work did not reset.
```

The key word is **constant**:

```text
Codex changes.
Claude changes.
OpenCode changes.
The conversation stays constant.
```

## Product Shape

Constant is a local-first CLI and library that reads agent runtime state, normalizes it, compiles continuation packets, and launches another runtime with the packet.

Initial command shape:

```bash
constant packet --from codex --latest
constant packet --from claude --session <id>
constant latest --from codex --to claude --dry-run
constant run --from claude --to codex
constant doctor
```

Later:

```bash
constant switch claude
constant switch codex
constant shell
```

The first product should feel like `git`, `gh`, `uv`, or `rsync`: a practical local tool with sharp nouns and predictable output.

## The Honest Claim

Constant can make conversation continuity work. It cannot magically merge hidden provider state.

Do not claim:

```text
Claude literally imports Codex's internal session memory.
Codex literally imports Claude's private tool state.
Every plugin, approval, compaction, and hidden prompt becomes shared.
```

Do claim:

```text
Constant reads the visible local evidence from one runtime, builds a high-signal continuation packet, and gives that packet to the next runtime.
```

That is enough to make the experience useful.

## Prior Art And Distinction

Existing adjacent products mostly solve different problems:

- Multi-agent dashboards coordinate several agents.
- CLI adapters normalize how to run several agents.
- Usage trackers read `.claude` and `.codex` for cost, token, and session metrics.
- `~/chat` already proved stateless neighbor summons through self-reads, transcript logs, and blackboard folders.

Constant's sharper primitive:

```text
conversation portability across agent runtimes
```

`~/chat` solved:

```text
summon a neighbor into a host room
```

Constant solves:

```text
move the host room between runtimes
```

## Source Substrate

Burnrate research already mapped much of the local anatomy.

Claude Code surfaces:

```text
~/.claude/projects/<encoded-cwd>/<sessionId>.jsonl
~/.claude/sessions/*.json
~/.claude/history.jsonl
~/.claude/usage-data/session-meta/*.json
~/.claude/usage-data/facets/*.json
~/.claude/file-history/<uuid>/v#
```

Codex surfaces:

```text
~/.codex/sessions/YYYY/MM/DD/*.jsonl
~/.codex/state_5.sqlite
~/.codex/history.jsonl
~/.codex/session_index.jsonl
~/.codex/logs_2.sqlite
~/.codex/memories/
```

Important local references:

```text
/Users/hamulia/dev/ai-usage-app/research/codex-telemetry-study.md
/Users/hamulia/dev/ai-usage-app/research/engineering/claude-code-data-surfaces/notes.md
/Users/hamulia/dev/ai-usage-app/research/engineering/codex-architecture-surface-area/notes.md
/Users/hamulia/dev/ai-usage-app/research/engineering/the-claude-local-study/notes.md
/Users/hamulia/chat/README.md
/Users/hamulia/chat/AGENTS.md
```

Burnrate code references:

```text
/Users/hamulia/dev/ai-usage-app/Sources/BurnrateCore/UsageSnapshotSource.swift
/Users/hamulia/dev/ai-usage-app/Sources/BurnrateCore/ClaudeUsageWatcher.swift
/Users/hamulia/dev/ai-usage-app/Sources/BurnrateCore/UsageModels.swift
```

These are not product dependencies yet. They are evidence that the local state exists and can be parsed.

## System Shape

Constant should be built in layers.

### 1. Runtime Readers

Each reader knows how to discover and parse one runtime's local session format.

Initial readers:

- `claude`: reads `~/.claude/projects/**/*.jsonl`, `~/.claude/sessions/*.json`, optional facets/session-meta.
- `codex`: reads `~/.codex/sessions/**/*.jsonl`, joins metadata from `~/.codex/state_5.sqlite`.

Later readers:

- `opencode`
- `aider`
- `gemini`
- `cursor`
- `custom`

### 2. Neutral Thread Model

Provider-specific events become one neutral shape:

```ts
type Runtime = "claude" | "codex" | "opencode" | "aider" | "gemini";

type ThreadEvent = {
  runtime: Runtime;
  sessionId: string;
  timestamp?: string;
  role: "user" | "assistant" | "tool" | "system";
  text?: string;
  toolName?: string;
  cwd?: string;
  gitBranch?: string;
  files?: string[];
  metadata?: Record<string, unknown>;
};

type ThreadState = {
  sourceRuntime: Runtime;
  sourceSessionId: string;
  title?: string;
  cwd?: string;
  gitBranch?: string;
  model?: string;
  createdAt?: string;
  updatedAt?: string;
  events: ThreadEvent[];
};
```

Keep this model boring. The intelligence should live in the packet compiler, not in a prematurely clever schema.

### 3. Context Selector

Constant cannot inject infinite history.

The selector should prepare:

- recent raw turns, ideally 8 to 12 turns
- a durable summary
- important decisions
- unresolved questions
- tool/file activity
- cwd and git state
- changed files
- source runtime notes

Use raw turns for texture. Use summaries for long-range continuity.

### 4. Continuation Packet Compiler

The packet is the core product.

It should be markdown first because markdown is readable, auditable, and directly ingestible by every agent CLI.

Packet shape:

```md
# Constant Continuation Packet

You are continuing a conversation that began in <source runtime>.

## Current Goal
...

## What Happened So Far
...

## Recent Raw Turns
...

## Decisions
...

## Open Questions
...

## Workspace State
cwd:
git branch:
changed files:

## Runtime Notes
The prior runtime was <source>. You are now <target>.
Do not claim native memory of the prior session. Continue from this packet faithfully.

## User's Next Request
...
```

The packet should not be an apologetic dump. It should feel like a competent chief of staff walking the next runtime into the room.

### 5. Launcher

Launchers convert packets into runtime invocations.

Initial:

```bash
claude -p "$(cat packet.md)"
codex exec "$(cat packet.md)"
```

Preferred safer versions:

```bash
claude -p --output-format stream-json < packet.md
codex exec --json - < packet.md
```

Before launching Claude, Constant must run billing/auth checks:

- is `ANTHROPIC_API_KEY` set?
- does `claude auth status` report `authMethod: claude.ai` or Console/API?
- is the subscription type visible?
- should the user be warned about API credits?

Before launching Codex:

- check `codex login status` or equivalent auth status
- check sandbox/approval policy
- warn before `danger-full-access`

### 6. Audit Log

Every run writes:

```text
.constant/
  packets/
    <timestamp>-<from>-to-<to>.md
  runs/
    <timestamp>/
      source.json
      packet.md
      target.stream.jsonl
      target.final.md
```

The user should always be able to see exactly what was injected.

## Trust And Safety Boundaries

V0 must be read-only against runtime homes.

Allowed:

- read session JSONL
- read SQLite metadata with readonly access
- read project git status
- write Constant's own `.constant/` artifacts
- launch a target runtime only when explicitly requested

Forbidden in V0:

- edit `.claude`
- edit `.codex`
- edit auth files
- scrape browser cookies
- read or display raw secrets
- silently continue into API billing
- mutate the user's repository without the active runtime doing it through its normal permission model

Sensitive files:

```text
~/.codex/auth.json
~/.claude/.credentials.json
Keychain items
raw history.jsonl
prompt-input/debug dumps
logs with environment snippets
```

Constant may use presence/metadata from sensitive surfaces only when needed. It should not display secret-bearing contents.

## First Prototype Scope

V0 should be a CLI with no TUI.

Target stack can be decided later, but the simplest likely stack is TypeScript/Node because it fits npm distribution and agent CLI environments.

V0 commands:

```bash
constant doctor
constant list --runtime codex
constant list --runtime claude
constant packet --from codex --latest --to claude
constant packet --from claude --latest --to codex
constant run --from codex --latest --to claude
```

V0 success criteria:

1. It can find latest Claude and Codex sessions.
2. It can parse user/assistant turns from both.
3. It can join Codex rollout files to `state_5.sqlite` metadata.
4. It can identify cwd, title, branch, model, and session ID.
5. It can generate a readable packet under `.constant/packets/`.
6. A target runtime can use that packet to accurately restate the thread and continue.

No more than that.

## Product Taste

Constant should feel local, sober, and sharp.

Avoid:

- dashboard-first thinking
- agent hype
- "AI OS" branding before the primitive works
- pretending to merge private state
- ornate language in generated packets
- giant context dumps that make the target runtime worse

Prefer:

- precise commands
- readable packet artifacts
- dry-run by default for first use
- explicit source and target runtime labels
- auth/billing guardrails
- local-first privacy
- honest limitations

Generated packet tone:

```text
plain, direct, high-signal
```

No fake-deep framing. No startup pitch language. The target runtime needs operational continuity, not a manifesto.

## Open Questions

- Should Constant store its own global thread database, or stay per-project with `.constant/` folders?
- Should `constant run` default to print/non-interactive mode, or should it open an interactive target runtime with the packet as the first prompt?
- Can Codex app-server provide better structured thread reads than direct JSONL parsing?
- How much should Constant summarize itself versus asking the source runtime to summarize before switching?
- Should packet compilation be deterministic first, then optional LLM compression later?
- How should Constant detect and redact secrets in raw turns?
- What is the right context budget for each target runtime?
- How should it handle binary/image attachments?
- How should it represent tool results without flooding the packet?

## Likely Build Order

1. Create repo scaffold and package.
2. Implement file discovery for Claude and Codex.
3. Implement neutral event extraction.
4. Generate deterministic packets with no LLM summarizer.
5. Add `doctor` auth/billing checks.
6. Add `run` launcher for `claude -p` and `codex exec`.
7. Add packet size budgeting and redaction.
8. Add optional rolling summary.
9. Add OpenCode adapter.
10. Only then consider a live shell or TUI.

## One Line

Constant keeps the conversation constant while the active agent runtime changes.
