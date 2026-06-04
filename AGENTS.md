# Constant Agent Instructions

Constant is an early-stage dev tool for cross-runtime conversation continuity.

Default posture: preserve the product spine before building. Do not turn this into a generic multi-agent dashboard, model router, usage tracker, or wrapper library.

## Core Frame

Constant's job is:

```text
read one agent runtime's local session state
compile a continuation packet
start another runtime with that packet
```

The claim is not native shared memory. The claim is portable conversation continuity.

## Before Editing

1. Read `README.md`.
2. Read `PRODUCT.md`.
3. If touching implementation, inspect the relevant local runtime source format before assuming schemas.
4. Treat `.claude`, `.codex`, auth files, history files, and logs as sensitive.

## V0 Boundary

V0 is read-only against agent runtime homes.

Allowed:

- read local session JSONL
- read SQLite metadata in readonly mode
- write Constant-owned artifacts under `.constant/`
- generate continuation packets
- launch target CLIs only when explicitly requested

Avoid:

- modifying `~/.claude` or `~/.codex`
- reading secret-bearing files unless the task explicitly requires a presence/auth check
- displaying raw auth, env, or token values
- silently using API billing paths
- building a TUI before the packet compiler works

## Source Research

Important local source material:

```text
/Users/hamulia/dev/ai-usage-app/research/codex-telemetry-study.md
/Users/hamulia/dev/ai-usage-app/research/engineering/claude-code-data-surfaces/notes.md
/Users/hamulia/dev/ai-usage-app/research/engineering/codex-architecture-surface-area/notes.md
/Users/hamulia/dev/ai-usage-app/research/engineering/the-claude-local-study/notes.md
/Users/hamulia/chat/README.md
```

## Language

Use "runtime" or "agent CLI" in user-facing language. Avoid "harness" unless discussing internal adapters.

Use these terms consistently:

- Constant
- runtime reader
- neutral thread model
- continuation packet
- source runtime
- target runtime
- packet compiler
- launcher

## First Serious Build

The first working proof should be:

```bash
constant packet --from codex --latest --to claude
```

It should write a readable markdown packet. Manual execution through `claude -p` is acceptable for the first proof.
