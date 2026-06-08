# Changelog

All notable changes to Constant are recorded here. This project adheres to
[Semantic Versioning](https://semver.org/).

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
