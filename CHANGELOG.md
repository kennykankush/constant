# Changelog

All notable changes to Constant are recorded here. This project adheres to
[Semantic Versioning](https://semver.org/).

## [0.5.1] - Recall opens the volume the ledger recorded

### Fixed
- **`constant recall <handle> chNN` opened the wrong record volume.** It
  reconstructed the conventional `chNN-from-<runtime>.json` path instead of the
  path the ledger actually recorded, so after a reseat, compaction, restore, or
  import — anywhere the physical volume number diverged from the chapter number —
  it surfaced a different chapter's turns under misleading `[chNN·N]` addresses.
  Recall now reads the ledger's `snapshot` (falling back to reconstruction only
  for legacy rows written before that field), exactly as `audit` and
  `latest_snapshot` already do. Read-only throughout — no record was ever at
  risk. Covered by a regression test.

### Validated
- **Codex `0.141` → `0.142`.** The continue-eval drift matrix went 36/36
  verbatim message-by-message identical across every source→target pair on live
  codex 0.142.3 (claude 2.1.170, opencode 1.17.3).

## [0.5.0] - Observability and the handover

New surfaces onto structure Constant already owns, plus a runtime-validation
refresh.

### Added
- **`constant route --json`** — the fork-graph the trail reconstructs, as JSON
  (alias, parent, mode, active, per-node resume command), so external tools read
  a conversation's lineage without scraping the debug view.
- **`constant ps --deep`** — joins each live agent to the trail: which chapter of
  the thread it holds, and a flag when two live agents are double-booked on one
  conversation. Adds `chapter`/`shared_with` to `ps --json`.
- **`constant audit`** — a read-only command reporting, per chapter, what a paged
  render keeps verbatim vs files (recallable), via a pure `render_stats` that
  shares the tail math with the renderer so the two can't drift. Missing or
  corrupt record volumes are surfaced, never silently dropped.
- **`Ctrl-B H` — the handover gesture.** Pastes a sign-out request into the
  hosted agent (bracketed paste + Enter); the departing mind briefs its
  successor — goal, decisions and why, dead-ends, next step, gotchas — and the
  sign-out rides the carried tail (recitation at the recency edge). Input-only:
  the host never parses the child's output.
- **`constant host --yolo`** — spawn the child runtime with its own
  approvals-bypass flag (codex `--dangerously-bypass-approvals-and-sandbox`,
  claude `--dangerously-skip-permissions`), opt-in and never leaked outside the
  hosted session.

### Validated
- **Codex `0.139` → `0.141`, OpenCode `1.14` → `1.17`.** The continue-eval drift
  matrix went 36/36 verbatim message-by-message identical across every
  source→target pair on the live runtimes (codex 0.141, claude 2.1.170,
  opencode 1.17.3).

## [0.4.1] - Compact in place

### Added
- **Compact-in-place.** Pressing the hosted runtime's own switch key (`Ctrl-B`
  then the current runtime) re-lays the live thread in a fresh projection of the
  same runtime, asking `[v]erbatim · [c]ompact` rather than no-opping.

### Fixed
- A `collapsible_match` lint in the explorer surfaced by CI's newer clippy.

## [0.4.0] - v2 genesis (conversation virtual memory)

The doctrine: the CONVERSATION is total and immutable in the record; the CONTEXT
a runtime wakes up to is a compiled VIEW of it. The pager, retrofitted onto
third-party CLIs from outside.

### Added
- **`constant recall`** — read filed turns back from the record by stable address
  (`ch04·12`), verbatim, speaker-labeled, output-capped.
- **The paged renderer** (`--render paged`) — head card + deterministic index +
  verbatim tail. Zero model calls (a table of contents cannot hallucinate);
  `--tail CHARS` overrides the budget.
- **The trail explorer** — `constant trail` in a terminal becomes a place:
  type-to-search → a conversation → its chapters → the filed turns, verbatim.
- **The resume picker** — instant open, names streamed in, true pagination, scope
  as two axes (place × the constant-trail lens).
- **Carry by name** — `carry --session` resolves handles and names; duplicates of
  one conversation auto-pick newest, different conversations get a picker.
- **A traversable cockpit** — `Ctrl-B t` graph gains an ↑↓ cursor; Enter lands in
  the selected chapter's projection; the head shows where you are.
- One design language across every printout; lineage tags (`origin` · `chNN`).

## [0.3.3]

Maintenance release with no functional changes — cut to verify the update
doorbell end to end on a live install.

## [0.3.2] - The doorbell

How an existing install learns an update exists — at every door, the way
the runtimes themselves do it.

### Added
- **Update offer at startup.** When a newer release is known and you're on a
  real terminal, `constant host` (and `resume`) asks `update now? [y/N]`
  before hosting. `y` runs the right updater for how the binary was
  installed — `brew upgrade` for brew, the curl installer for standalone
  binaries, printed instructions for cargo — and relaunches the new
  Constant with the same arguments. Anything else continues as-is.
- **Update notices.** A quiet startup line (`constant v0.3.3 is available —
  brew upgrade …`) read from a local cache, so launch never waits on the
  network; the background version sweep refreshes it at most ~once a day.
  Drift warnings now point at the fix when one exists (`⚠ codex 0.140
  unvalidated · constant v0.3.3 is out — brew upgrade`) instead of only
  warning. `constant doctor` always live-checks and reports
  `constant : 0.3.2 (latest)` or the available upgrade (text and `--json`).
- **Local-first honesty:** the check is one anonymous GitHub release-API
  GET that sends nothing, at most daily (always, on explicit `doctor`);
  `CONSTANT_NO_UPDATE_CHECK=1` disables all of it.

## [0.3.1] - Surviving codex 0.139

The first live verification pass met real codec drift: codex 0.139 changed
how it tracks sessions, and a switch could carry the wrong conversation, or
silently land in an empty target. One night, three rounds of fixes — each
one driven by evidence from a live machine.

### Fixed
- **Codex 0.139 made `/resume` invisible to file-based detection** (it reads
  its registry without touching the resumed session file, and creates files
  lazily). Switch-time detection for codex now reads codex's own sources, in
  order: the **event log** (`logs_2.sqlite` — the only witness to a `/resume`
  that hasn't been talked in yet), then the **thread registry**
  (`state_5.sqlite`), then the file scan. Every candidate is validated: the
  session file must exist, hold a conversation, and match the host directory.
  A `/resume`-away inside codex now carries correctly even if you never typed
  a word in the resumed conversation.
- **The AGENTS.md scaffold can no longer become a conversation's name.**
  0.139 injects project instructions as a plain user message; title
  derivation and harvesting now reject scaffold at both layers.
- **Closing the trail graph no longer blanks an inline-painting child.** The
  view opens on the terminal's alternate screen when the child (codex 0.139,
  claude) paints inline — closing restores the screen pixel-perfect, and
  child output produced while the view was open is replayed, not discarded.
  Full-screen children keep the resize-repaint path.

### Added
- **The carry gate.** A continue-switch with nothing carryable is cancelled
  loudly — the child keeps running — instead of tearing it down into an
  empty target. Uppercase switches (new continuation) always proceed.
- **Version warnings where they matter.** The host checks installed runtime
  versions in the background; an unvalidated runtime shows `⚠` next to its
  name in the status bar with a held notice, instead of `doctor` knowing
  quietly. Validated codex line bumped to **0.139.x** after a live pass.
- **Loud degradation theater.** Failed and empty carries hold a `⚠` notice
  in the bar (was a scroll-away dim line); a "successful" carry of ≤1 turns
  shows `⚠` instead of `✓`.

## [0.3.0] - Names, the control room, and conversations that travel

The identity release: every conversation gets a stable handle, a human title,
and chapters; the host gets a control room (trail graph, switch theater, a
real `:` line); `constant ps` shows every live agent on the machine; and
`pack`/`unpack` carry a whole conversation — record and lineage — across
machines. Underneath, sixteen adversarial integrity tests and a wider CI
matrix (macOS + real opencode) guard all of it.

### Added
- **`constant pack` / `constant unpack` — conversations cross machines.**
  `pack HANDLE` bundles a conversation (its ledger rows, verbatim, plus every
  record volume) into one portable file; `unpack` on another machine writes
  the volumes into the local vault (never overwriting — volumes are
  immutable), appends the rows idempotently with snapshot paths rewritten to
  the local vault and foreign projection paths blanked (a pack carries the
  RECORD; native sessions reprint on arrival), and re-mints the handle only
  if the local registry already gave it to a different conversation. Then
  `constant resume <handle>` wakes the conversation from the record. The
  slogan, completed: the conversation stays constant while the machine
  changes.
- **The cockpit.** The trail graph (`Ctrl-B t`) now acts: `c`/`x`/`o`
  (shift = new fork) switch runtimes straight from the graph, and `r` opens
  the command line over it prefilled with `rename <current name>`.
- **The control room.** `Ctrl-B t` toggles Constant's own full-screen view of
  the conversation: a colored, GitLab-style chapter graph — one dot per
  chapter, rails between them, fork/restore glyphs, relative times, record
  markers, and a "you are here" head. No compositing: the child's output is
  paused while the view is open and the child repaints on exit via a resize
  wiggle. **Switch theater:** after every carry, the receipt is held in the
  status bar for a few seconds (`✓ ch04 → claude · auth bug · carried 14
  turns …`) instead of flashing past. **`:` line quality:** Tab completion
  for verbs and runtimes, `rename ` + Tab prefills the current name for
  editing, and ↑/↓ recall command history.
- **The naming redesign: handles, titles, chapters.** Every conversation now
  has a stable **handle** — one color word + 2-digit tail (`cobalt-37`),
  suggested by `sha256(conversation id)` but DECIDED by the ledger registry
  (a taken handle deterministically extends its tail), so collisions are
  impossible by construction and the shape can never be confused with
  opencode's adjective-noun slugs. On top sits the **title** (the glance
  layer) with strict precedence: an explicit rename — `constant rename` or
  `:rename` inside a hosted session — locks it forever; otherwise a
  runtime-generated title is harvested (opencode's titles, a `/rename` done
  inside Claude Code); otherwise a smart birth-slug (leading filler words
  stripped). Renames re-stamp claude/codex native pickers immediately. And
  carries are now **chapters**: `ch04` everywhere `t04` used to be (events,
  snapshots listing, status bar, record volume filenames), with the native
  picker stamp leading with the human name: `auth bug · ch04 ← codex ·
  cobalt-37`. Handles work as addresses everywhere (`resume cobalt-37` is
  never ambiguous); existing conversations get handles retroactively.
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
