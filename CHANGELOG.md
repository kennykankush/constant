# Changelog

All notable changes to Constant are recorded here. This project adheres to
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
- **`constant ps`** (alias `live`) — every agent CLI process alive on the
  machine right now: runtime, uptime, the session it holds (read from its own
  args), the conversation name when the trail knows it, and its working
  directory. Read-only — one `ps` walk plus best-effort cwd lookup; wrapper
  processes (dtach, `sh -c`, login shells, Constant's own host) are never
  double-counted, and an agent's launcher/native-binary pair dedupes to one
  entry. `--json` for scripts.
- **Integrity test suite.** Sixteen new regression tests covering the
  adversarial surface: hostile session ids cannot escape the record vault
  (path traversal), embedded ANSI/control bytes never reach the terminal,
  secrets are verified redacted in BOTH the projection and the record, the
  vault is owner-only, corrupted ledger lines are tolerated, a blocked record
  warns but never blocks a carry, codex discovery picks the right cwd over the
  newest file, ambiguous ids across stores are refused, symlinked reuse
  targets pointing at the source are refused, codex torn-tail tolerance,
  gemini loader shape mapping, opencode export-trailer parsing, and IR
  forward-compatibility with unknown future fields. (Plus a hardening the
  tests forced: record-vault directory components now allow only
  `[A-Za-z0-9_-]`.)
- **CI matrix.** Tests now run on macOS as well as Linux, and a dedicated CI
  job installs the real opencode binary so the import→export round trip (and
  upstream drift) is exercised on every push.

## [0.2.0] - The record, the third runtime, and the hardened carry

One release, three layers: a bank-grade hardening pass over every write,
identity decision, and failure path in the carry pipeline; the record (per-hop
neutral snapshots that make native sessions reprintable projections); and two
new runtimes — OpenCode in full, Gemini as a carry source.

### Fixed
- **Atomic projection writes.** Both codec writers now materialize into a tmp
  sibling, fsync, and rename into place — a crash or full disk mid-switch can no
  longer destroy the projection holding the conversation's newest turns. A
  failed write provably leaves the previous projection intact (regression-tested).
- **Spawn-time fence restored.** Carry-seed detection is scoped to sessions
  touched at/after the hosted child was spawned (plus cwd), so an old session in
  the same directory — or a concurrent codex/claude running there — can no
  longer be adopted as the seed on a first switch.
- **Declared session identity.** Fresh claude children are launched with a
  minted `--session-id`, and every resumed child's id is tracked — source
  resolution now prefers the session the harness positively KNOWS the child
  owns over filesystem inference. Session-id lookups resolve newest-file-wins,
  so stale duplicate rollouts are never carried.
- **Codex registry writes are real errors.** The two divergent sqlite upserts
  merged into one; a failed `threads` upsert now fails the carry (with fresh
  fallback) instead of silently producing a session `codex resume` can't find.
- **Switch failures degrade instead of dying.** A target that can't launch
  falls back: target resumed → target fresh → previous runtime. A child that
  exits within 2s of a spawn (rejected resume, missing binary) is detected and
  relaunched fresh once, with the carry preserved in the trail.
- **Torn-tail tolerance.** A half-written final transcript line (child killed
  mid-flush) no longer voids the whole carry; mid-file corruption still fails
  loudly. SIGTERM grace extended to 1s so large flushes finish.
- **Redaction invariant enforced on the carry path.** `sanitize` now drops
  nested `block.data` payloads and per-message metadata (previously only the IR
  export did), and the redactor set gains AWS access keys, PEM private-key
  blocks, and bare JWTs.
- **Bracketed-paste guard.** Pasted content containing the raw prefix byte can
  no longer trigger a phantom runtime switch mid-paste.
- **Ledger hardening.** Trail writes surface their failures (a silent miss can
  fork a conversation on re-host); recorded cwds are canonicalized and compared
  symlink-tolerantly; the ledger-reconciliation core is now pure and unit-tested
  (ping-pong stable pair, sibling isolation, moved-path identity).
- Store walkers no longer follow symlinks (a cycle can't hang a switch);
  `has_conversation` parses lines instead of substring-matching (embedded
  AGENTS.md text can't make an empty session look conversational); synthetic
  claude records carry the source conversation's real model name; the debug log
  moved to a per-process temp path.

### Added
- **Third runtime, doubled: OpenCode (full) + Gemini (carry source).**
  - **OpenCode** is a complete runtime: host it (`constant host opencode`),
    switch to it live (prefix `o`/`O`), carry into it (`--to opencode`), out
    of it (`ses_…` ids resolve automatically via `opencode export`), resume it
    (`opencode -s <id>`), list it (`sessions`, with db-backed titles). The
    writer goes through opencode's own supported door — export-shaped JSON +
    `opencode import`, which validates, preserves our session ids, and
    upserts on refresh (the stable pair works natively). Tool history maps
    both ways, so `--with-tools` works on day one. Covered by a real
    import→export round-trip test (skips when the binary is absent).
  - **Gemini** is a carry source: existing gemini conversations (sessions,
    thoughts, tool calls) load, list, and carry into codex/claude/opencode.
    Carrying INTO gemini lands after one live landing-pad verification —
    in a scratch directory only: gemini 0.40's storage migration can DELETE
    legacy-format chats (it did; see docs).
  - Internals: a neutral-IR codec per runtime (hub-and-spoke — no pairwise
    translators), a pure-Rust SHA-256 for gemini's `projectHash = sha256(cwd)`
    discovery evidence, and read-only sqlite discovery for opencode.
- **Persistent status bar.** A tmux-style bar lives on the terminal's bottom
  row while hosting: active runtime (`+tools` when carrying tool history),
  trail position and conversation slug, and the prefix keys. Constant stays a
  pass-through proxy — the child PTY is simply told the terminal is one row
  shorter, a scroll region protects the bar row from inline scrolling, and the
  bar repaints only when the child has been idle for a beat (no escape
  injection mid-paint). Auto-disables on tiny terminals; `--no-bar` (on `host`
  and `resume`) turns it off.
- **`--with-tools` carry (experimental).** Opt-in on `carry`, `host`, and
  `resume`: tool calls and results are carried across the runtime boundary
  instead of dropped. Every string anywhere in their JSON payloads goes through
  the full redactor set, oversized tool outputs are capped (a file dump must
  not ride into the target's context window), and reasoning stays dropped
  (model-internal, never carried). Tool records written into claude sessions
  carry the strict resume schema (`entrypoint`/`requestId`). Record volumes
  written from a with-tools carry hold the (redacted, capped) tools too, and
  `restore` reprints whatever the record holds. The receipt distinguishes
  `kept N tool events` from `dropped N tool events`.
- **`constant resume [QUERY]`** — re-host a conversation straight from the
  trail: picks its latest projection and opens it live in the harness with the
  session identity declared from birth (no filesystem detection). No QUERY
  resumes the newest conversation in the current directory; QUERY matches the
  slug or conversation id (`--in codex|claude` picks a side, `--list` shows
  candidates, `--all` widens scope). If every live projection has been
  deleted, resume reprints one from the conversation's latest record volume
  first. Without a TTY it prints the native resume command instead, so it
  stays scriptable.
- **The record: per-hop IR snapshots.** Every carry now writes the distilled
  conversation to `~/.constant/snapshots/<conversation>/tNN-from-<runtime>.json`
  (atomic, owner-only, post-redaction) BEFORE materializing the native copy,
  and the trail row references its volume. The IR becomes the durable record;
  native sessions become reprintable projections of it — crash recovery and
  format-drift immunity in one move. A failed record write never blocks a
  switch, but is always announced.
- **`constant snapshots [--all]`** — list the record volumes per conversation
  (from the ledger, with missing files marked).
- **`constant restore SNAPSHOT [--to codex|claude] [--json]`** — reprint a
  fresh native session from any record volume (always mints; never overwrites
  anything, least of all the record). Defaults to the runtime the record came
  from; the restore is logged to the trail so lineage stays joined.
- **Carry receipt.** Every switch and headless carry reports what the
  distillation did: `carried N turns · dropped M tool events · K redactions` —
  in the host trail line, `carry` output, and `--json` (`receipt`).

### Changed
- A switch now loads + distills the source transcript once (previously twice),
  and format detection reads one line instead of the whole file — switch latency
  on long conversations roughly halved.
- `d` is labeled honestly as quit (the hosted CLI exits with it); `detach`
  remains accepted in the command line.

## [0.1.4] - Source overwrite guard hardening

### Fixed
- Move the donor-protection invariant into the core alembic layer: a carry now
  refuses to materialize over the source session even if a future caller or
  corrupted trail passes the donor path as the reuse target.

## [0.1.3] - Orientation and explicit continuations

### Added
- Add `constant status` as the quick orientation command for runtime readiness,
  current trail state, and latest sessions.
- Change `constant trail` to show current projections by default, with
  `constant trail --events` preserving the raw append-only switch ledger.
- Add explicit new-continuation routing (`--new` and hosted uppercase switch
  shortcuts) so users can intentionally branch instead of refreshing an existing
  projection.

### Fixed
- Keep `status` cheap by reading only the newest transcript title per runtime.
- Hide deleted projection files from the current trail view while keeping their
  historical events visible under `--events`.

## [0.1.2] - Homebrew tap readiness

### Fixed
- Align the crate version with the release tag so `constant --version` matches
  the artifact version Homebrew installs.

### Added
- Prepare direct-repository Homebrew tap support via `Formula/constant.rb`.

## [0.1.0] - Genesis

The first release line: one conversation, any agent runtime. Host an agent CLI
inside a Constant PTY or use the headless CLI, carrying the thread via the
`alembic` cartridge (distill -> target's native session -> native resume).

### Added
- **Live runtime switch** - host `codex` or `claude` in a Constant PTY and switch
  between them with a tmux-style prefix (`Ctrl-B` then `c`/`x`), carrying the
  conversation. Prefix is recognized in both legacy and Kitty keyboard encodings.
- **Headless `carry`** - carry a conversation without opening a hosted terminal:
  `constant carry --from codex --to claude`, or target an explicit
  `--session <path-or-id>`. Prints the native resume command; `--dry-run` previews
  without writing, and `--json` gives machine-readable output. `distill` remains
  as an alias.
- **Session discovery** - `constant sessions [--from codex|claude] [--all]
  [--titles] [--json]` lists carryable sessions. The default scope is the current
  directory; `--titles` reads transcripts only when requested.
- **Neutral IR export** - `constant export --from codex --out FILE` or
  `constant export --session <path-or-id>` writes the distilled, redacted neutral
  conversation model. Without `--out`, it prints JSON to stdout; it refuses to
  overwrite the source session.
- **Environment preflight** - `constant doctor [--json]` reports runtime CLI
  versions, session-store presence, Codex SQLite presence, and the validated
  codec version lines.
- **`alembic` cartridge** - distills the active session to the pure conversation
  (scaffold stripped, secrets redacted, tool/reasoning dropped), transcodes it
  into the target runtime's native format, and resumes it natively.
- **Trail** - every Constant projection is named `constant·tNN·from-<src>·<slug>`
  and stamped into the runtime's native resume picker (codex `title`, claude
  `custom-title`). `constant trail [--all]` prints the lineage from
  `~/.constant/trail.jsonl`; lineage and numbering survive re-hosts.
- **`constant --version`**.

### Safety
- **Originals are never overwritten (F1).** Live host switches and headless
  carries both mint and ping-pong Constant-owned projection sessions; the user's
  original codex/claude sessions are read as the seed but never written. A
  codex->claude->codex round trip leaves the untouched original plus two stable
  projections - no proliferation.
- **Read-only inspection modes** - `doctor`, `sessions`, `sessions --titles`,
  `carry --dry-run`, and stdout `export` do not write runtime homes. `export
  --out` writes only the requested file, and guards against clobbering the source
  session.
- **Secret redaction** of emails / `sk-` / `gh*_` / `xox*` / `key=value` from any
  carried text, compiled once.
- **Graceful child teardown** - `SIGTERM` (flush/cleanup) before `SIGKILL`.
- **Codec drift guard** - per-runtime round-trip tests fail loudly if a CLI's
  session format changes under us.

### Supported CLI versions
Validated against **codex 0.137.x** and **claude 2.1.x**. Session formats are
undocumented and can change between releases; the round-trip tests are the early
warning, and the carry falls back to a fresh session if distillation fails.

### Known limitations
- A long conversation may exceed a smaller runtime's context window on carry
  (the carry is the full thread; summarization-to-fit is not yet implemented).
- A single event loop hosts one child at a time (S3); compacted-codex summaries
  are not carried (L3). Both are documented trade-offs for this release.
- A `/resume` to a *different* conversation inside the hosted child is not
  auto-followed on the next switch — it's indistinguishable from an unrelated
  same-directory session, so Constant keeps carrying the tracked pair rather than
  risk hijacking the carry with someone else's session.
