# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.11.1] - 2026-10-04

### Fixed

- `hours_back: 0` no longer fails every window-tool call (#25). The served
  schema states `minimum: 1` so a schema-honouring client cannot send it, and
  the handler reads `0` (and `null`) as "omitted" — the tool's default window,
  never the whole store. `all: true` with `hours_back: 0` reaches the whole
  store the caller explicitly asked for; `all: true` with a real window is
  still rejected as the contradiction it is. `total_recall`'s `hours_back`
  reads `0` the same way, closing the last route to the adapter's unbounded
  mechanism except `all: true`.
- `list_sessions` returns rows again (#25). The row count it renders is
  derived from the response budget (`max_bytes`, default 16,384) instead of a
  fixed 200 fat rows that the 16 KiB flood cap replaced whole, so a default
  call returns rows and states `held_back`. A row larger than the whole
  budget is still handed to flood control, which is the escape hatch every
  other report has.
- Unknown parameters are rejected by name on every tool (#25): `hour_back`
  or `directorys` is answered with the unrecognised field and the fields the
  tool does accept, instead of running the call on the default scope.
- Numeric inputs are validated in the house form on every tool (#25):
  negatives and over-ceiling values for `hours_back`, `limit`, `offset`,
  `max_bytes`, `max_record_bytes`, `line`, `start` and `end` return
  "<tool>: `<field>` is out of range — <cheap form>" instead of a raw serde
  type error, and the served schemas state the same `minimum`/`maximum` the
  handlers enforce. `line_histogram`'s `mode`/`line`/`start`/`end` sets are
  validated as a set: an unknown mode, `mode: extract` with no selector,
  `end` without `start`, `start` past `end`, and a selector without
  `mode: extract` are all named errors.

## [0.11.0] - 2026-10-04

### Added

- The window tools (`list_sessions`, `she_said_he_said_action`,
  `index_sessions`, `do_android_dream_of_electric_sheep`) take `all: true`
  as the explicit whole-store opt-in — every session of every project the
  store holds — and reject `all: true` combined with `hours_back`: pass
  one, not both. The adapter keeps `0` as its unbounded mechanism; only
  `all: true` reaches it.

### Changed

- The scope contract: every tool defaults to the smallest useful scope,
  work is bounded where the data lives, breadth is an explicit opt-in, and an
  unscoped request is rejected with the cheap forms named — never clamped,
  never silently run (README, docs/src/tools.md).
- `hours_back` is a window in hours, `1` or more, on the four window tools:
  `hours_back: 0` is rejected as a tool error naming the cheap forms —
  never clamped, never "no bound". The old "0 = no bound" default is gone
  from the MCP layer; the CLI keeps its own doctrine (`--hours 0` is the
  unbounded stream, the command typed is the scope asked for).
- `index_sessions` defaults to `hours_back: 24` — the natural call indexes
  the last day, not the whole store; whole-store indexing is `all: true`,
  explicit.
- The window tools' schema descriptions speak in one voice: `session_id`
  "Empty = the window (hours_back / directory / all)", hours_back "a window
  in hours, 1 or more; 0 is rejected", and every "0 = no bound" is gone
  from the MCP schema strings.

## [0.10.0] - 2026-10-03

### Changed

- The two LLM prompts of the recall path are byte-bounded: the current-state
  summary keeps the newest messages and the user-goals summary keeps the newest
  user messages, each under a 200 KB payload budget (~50K tokens, ~3s of
  ingest at the documented 1,000,000 input tokens/minute free-tier ceiling).
  A session too large to send in one piece now costs a bounded prompt instead of
  a request that cannot be ingested: past the vendor's input ceiling it was a
  hard error, and under it the ingest alone outran the client's deadline, which
  is why `total_recall` timed out on a multi-gigabyte store (issue #17).
  Truncation is tail-kept at a line and character boundary, never mid-character,
  and never silent.
- The recent-rollouts table holds at most 200 rows, the most recent in the
  window, and states how many older sessions in the window it did not print.

### Fixed

- A bounded recall prompt now says what it dropped: a marker line inside the
  prompt names the number of items and the number of bytes, and the timing
  comment at the top of the `total_recall` response carries both prompts' byte
  sizes and dropped counts.
- The vendored `line_histogram.awk` is published to its shared staged path
  atomically (write beside, rename over) instead of truncated and rewritten in
  place. Concurrent capped calls — parallel tool calls, or the test binaries
  cargo runs side by side — could previously read a half-written script, and a
  call that lost the race returned no bucket distribution at all: the overflow
  marker silently dropped the histogram it exists to carry. Measured on the old
  staging, 1 to 8 of 96 concurrent histogram calls came back empty.

## [0.9.2] - 2026-10-03

### Fixed

- An omitted `session_id` deserialized as a hard failure instead of the
  documented "Empty = most recent" default (issue #18). Six params structs
  lacked `#[serde(default)]` on the field — profile, extract messages,
  extract user messages, extract by type, compact and total_recall — so
  `total_recall` with only `hours_back` failed with "missing field
  `session_id`" while `{"session_id": ""}` worked. The description was
  right; the struct was wrong.

## [0.9.1] - 2026-10-02

### Added

- Flood control, generic and open-ended. Every tool response that can
  overflow without its own bound routes through one capped helper:
  `max_bytes` (default 16,384, settable on `she_said_he_said_action`,
  `do_android_dream_of_electric_sheep` and `total_recall`), the whole
  response written to a private file under `$TMPDIR/total-recall/`
  (mode 600), a line-boundary cut, and a line-oriented EOF marker naming
  the tool, the byte counts, the line count, the file's path, the 24-hour
  prune policy and the file's line histogram. JSON responses are never
  torn: an overflowing JSON response returns the marker alone. The
  bounded-extraction tools keep their own envelope as their flood control.
- The `line_histogram` MCP tool, running the vendored
  `scripts/line_histogram.awk` (the author's gist
  0454936144ee8dbc55bdc96ef532555e, byte-identical, embedded at compile
  time) with a direct `awk -f` spawn: histogram mode profiles any file in
  ten buckets (2 MB in, ~2 KB out), extract mode pages line ranges.

### Changed

- `she_said_he_said_action`, `index_sessions` and
  `do_android_dream_of_electric_sheep` take one `session_id` (partial
  match) instead of a sessions array, matching the rest of the tool
  surface. `list_sessions` — the one listing tool — takes a `hours_back`
  cutoff that now defaults to 240 hours (10 days; 0 = no bound), so an
  unbounded full-store list is no longer the default.
- The markdown docs no longer repeat the generated parameter surfaces:
  `tools/list` for the MCP tools and `total-recall --help` for the CLI are
  the authority, and the prose states the generic contracts that apply to
  tools added later.

### Fixed

- Redaction panicked on multibyte text: the prefix scanner walked
  byte-wise and sliced inside an em-dash, so compacting any session whose
  prose contains one died. Compaction of the authoring session was the
  reproduction.
- Overflow files collided when two capped calls landed in the same second,
  the second silently overwriting the first's file; the name now carries
  millis, pid and a per-process counter.

## [0.9.0] - 2026-10-02

### Fixed

- The `compact_session` MCP tool description no longer claims `--provider`
  selects the vendor. The flag is a CLI option; the MCP server constructs the
  compiled-in default provider and never read it, so the description
  advertised a control that did not exist. It now states the vendor compiled
  in as the default without naming the flag at all, and a new invariant test
  fails if any tool description mentions `--provider` again.

### Changed

- The test suite's scratch directories are unique per call, from one shared
  helper (`tests/common/scratch.rs`), with a hygiene test failing if any test
  file reintroduces a name-keyed temporary path. Two test files were observed
  failing under concurrent runs before the change: `sheep.rs` (four
  concurrent runs, all four red) and `rollout_opencode.rs` (four runs, all
  four red, on "table session already exists" and "disk I/O error").
- Every spawned test child runs outside the repository working tree with the
  vendor keys removed from its environment. `dotenvy` walks up parent
  directories, so a child started from the crate root could have read the
  developer's `.env`.
- Timing assertions moved out of the test suite and into a criterion bench
  (`bench_committed_fixtures`, over committed fixtures). The old 10 ms
  formatting budget was a load detector — it failed twice at 12.04 ms and
  21.02 ms against a ~2.5 ms median on a loaded machine, which is a bound no
  budget can be both tight enough to guard and loose enough to survive.

## [0.8.0] - 2026-10-01

### Added

- Secret redaction in the prompt path. `redact_secrets()` replaces every
  recognised credential shape with a `[REDACTED:<kind>]` marker before session
  content is sent to a vendor: vendor key shapes (`sk_`, `sk-`, `sk-ant-`,
  `tvly-`, `ctx7sk-`, Slack, GitLab, Google), GitHub tokens, `Bearer` header
  values, `key = value` assignments for credential-named keys, PEM private-key
  blocks, and JWTs. Unconditional — no flag, no environment variable, no
  vendor-by-vendor opt-out. It runs in prompt assembly (before the 500-char
  tool-result snip, so a key cannot be cut in half and have its prefix
  shipped) and again in `mercury::send_guarded`, so a future prompt builder
  that forgets the call still cannot put a key on the wire. SECURITY.md now
  documents what is redacted and the two limits of shape matching. This
  restores to the Rust implementation the redaction that 0.2.1 shipped in
  `compact.py` and that was lost when that file was deleted as dead code.
- LLM vendors are compile-time cargo features. `default = ["mercury", "mistral"]`;
  a `--no-default-features` build makes no LLM call, needs no API key, and does
  not even load a `.env` (`dotenvy` is behind the vendor features). Vendor URLs,
  models, keys and dotenv loading are behind `cfg`, and provider construction
  funnels through `mercury::provider_for`.
- `compact` honours `--provider`, which it previously ignored. An unrecognised
  provider is now an error instead of a silent fall-back to Mercury.
- `TOTAL_RECALL_{VIBE,CLAUDE,CODEX,OPENCODE}_ROOT` storage-root overrides and
  the `TOTAL_RECALL_SANDBOX=1` guard that fails closed rather than reading a
  live store.
- A documentation site at https://prompt-cult.github.io/total-recall/, built
  from `book.toml` + `docs/src` with mdBook and deployed to GitHub Pages by the
  official `upload-pages-artifact` / `deploy-pages` path. PRs build the book and
  run a link check (mdBook resolves links but does not verify that a target
  exists); only a push to main deploys.

### Changed

- MCP tools stay registered when their vendor is compiled out, returning a tool
  error naming the feature to rebuild with, rather than vanishing from
  `tools/list` and breaking clients that bind by name.
- The MCP server key is `total-recall` in both registration snippets (was
  `compaction`); MCP tool names derive from it, so the old key showed the tool
  surface under the wrong identity. The snippets now carry
  `INCEPTION_API_KEY` in their environment blocks, because a spawned server
  only resolves `.env` relative to a working directory the MCP client controls.
- `docs/opencode-store-vacuum.md` replaces the root-level `vacuum.md`, which
  pointed at a gitignored rehearsal artefact no cloner could have.
- Documentation DoD: `LICENSE-MIT` and `LICENSE-APACHE` (dual
  `MIT OR Apache-2.0`), `SECURITY.md`, `CONTRIBUTING.md`,
  `CODE_OF_CONDUCT.md`, this changelog, and the Cargo package metadata
  (`license`, `repository`, `homepage`, `readme`, `keywords`, `categories`,
  `exclude`).
- Release assets are laid out for the tool fetchers: each platform leg stages
  `total-recall_<version>_<target-triple>.tar.gz` with the single executable at
  the archive root, plus a `SHA256SUMS` covering them. Archives are
  reproducible (ustar, zeroed mtime, uid/gid 0, `gzip -n`), so the same source
  yields the same bytes. `mise use github:prompt-cult/total-recall` installs
  from a release without a `matching=` hint.

### Fixed

- Unreadable rollout data is an error, never an empty session. Every adapter
  read path swallowed I/O failure and returned zero messages, so a missing
  file, a directory where `messages.jsonl` should be, and an unopenable
  database each looked exactly like a store with nothing in it.
  `RolloutAdapter` read methods now return `ReadResult<T> = Result<T, String>`
  with messages naming the failed path; the `File::create("/dev/null")` and
  `Mmap::ok() -> &[]` fallbacks are gone; `list_sessions` sets `read_error` for
  codex, claude and mock, matching the rule vibe already used; malformed JSONL
  lines are traced with their line number; MCP tools answer `isError` and the
  CLI exits 2. Tool response payloads are unchanged — no field added, removed
  or renamed.
- `scripts/opencode-db-vacuum.sh`: `OPENDOC_DB` → `OPENCODE_DB`, and a dropped
  `OPENDOC_SKIP_PROCESS_CHECK` line that documented a flag `cmd_check` never
  read.
- Removed committed scratch: the tracked borrow-checker (`test_borrow.rs`) and
  its compiled binary from the repository root, plus the `test_borrow`
  artefacts the tree carried since 0.6.1.
- Test scratch directories are unique per call rather than per process. The
  vendor-feature tests keyed a child process's working directory on the pid,
  which every test in one binary shares, so a helper deleted the directory out
  from under a sibling thread's running child — an intermittent failure that
  only bit right after a recompile.

## [0.7.0] - 2026-09-24

### Added

- Opt-in `profile_session` cache. `--cache` (CLI global flag) and the `cache`
  argument (MCP) serve a profile from `tr_<session-id>_meta.json` in the
  adapter's shadow root, with staleness checked against one cheap
  `time_updated` source (a single `SELECT time_updated` row for opencode, the
  source file's mtime elsewhere) and a 15 s tolerance. Fresh → served from
  cache; stale, missing or corrupt → recomputed and rewritten. Cached payloads
  are validated on read through an RFC 8927 JSON Type Definition schema
  (`schemas/profile-cache.jtd`) compiled by `jtd-codegen`; a payload failing
  that gate is treated as corrupt. With the flag off, behaviour is
  byte-for-byte the pre-cache path and no cache file is created.
- Cheap most-recent-session resolution (#11).

### Fixed

- Claude adapter against the real `~/.claude/projects` store (#13): nested
  project-dir session discovery, uuid-stem ids, `ai-title`, timestamps,
  directory, thinking blocks, array-form `tool_result`, `subagents/` excluded.

## [0.6.2] - 2026-09-22

### Fixed

- Bounded extraction emits compact JSON and computes bounding on the same
  compact serialization that is emitted, so the payload respects `max_bytes`
  exactly with no pretty-print inflation past the contract.
- When even one record cannot fit under the budget after the envelope reserve,
  the tool emits a descriptive error instead of silently breaking the cap (same
  guard on the CLI: stderr, exit 2).
- An empty result window — an empty session, or paging past the end — never
  signals truncation in `bounds`.
- `list_sessions` surfaces read damage: a payload that cannot be read carries
  `read_error` on the entry. A missing file is not damage.
- `extract_by_type` gains `include_injected` (MCP) / `--include-injected` (CLI)
  to reconstruct sessions containing synthetic records; the record carries the
  `injected` flag either way.

## [0.6.1] - 2026-09-21

### Added

- `TOTAL_RECALL_{VIBE,CLAUDE,CODEX,OPENCODE}_ROOT` environment overrides, and
  `TOTAL_RECALL_SANDBOX=1` to make `make_adapter` fail closed rather than read a
  live store.
- `list_sessions` emits one canonical entry per vibe rollout payload, with other
  directory names that resolve to the same payload listed under `aliases`, so a
  caller paging the index never processes the same rollout twice. The canonical
  id is the one whose directory-name date prefix agrees with the session's
  start time.
- `extract_messages` / `extract_user_messages` return a bounded JSON envelope —
  default most-recent-100, 8 MiB ceiling, 256 KiB per-record clamp — with
  `next_offset` paging and an explicit truncation notice. **Breaking shape
  change** from a bare array.
- New `extract_by_type` MCP tool and `extract-by-type` CLI subcommand: a raw
  recovery dump as `type,timestamp,json` lines under a `#`-prefixed header
  carrying the bounds. For vibe the records are the byte-faithful source lines
  re-parsed from `messages.jsonl`.
- Documentation: the opencode store vacuum write-up (measured breakdown, safe
  prune/vacuum/swap procedure), its helper script, and the harness skills for
  the dream-pass search and the vacuum.

## [0.5.0] - 2026-09-20

### Added

- `do_android_dream_of_electric_sheep`: tantivy per-session shadow-index
  full-text search over `content` **and** `thinking`, merging the top hits per
  session into one score-ordered ranking. `list_sessions` and `profile_session`
  gain a `has_tantivy_index` presence flag.

### Changed

- Rationalisation pass: simplified `fragment_for`, moved `is_iso8601`, deduped
  `resolve_session`, slimmed tokio features. `.tantivy` is now gitignored.

### Documentation

- `AGENTS.md` Andon prime directive; Trunk Development Lite house rules.

## [0.4.2] - 2026-09-16

### Fixed

- Batch parallelism is now asserted from served-request overlap rather than a
  wall-clock fraction, which was flaky on CI. Results-order and count checks are
  kept.

## [0.4.1] - 2026-09-15

### Added

- `--version` wired into the CLI.

## [0.4.0] - 2026-09-15

### Added

- `she_said_he_said_action`: term-matched dialogue and tool mining, with
  matching pushed down into SQLite as a custom scalar function on a read-only
  connection. Emits HE SAID (user text), SHE SAID (assistant text) and THEY DID
  (tool calls) per session as a markdown report, most-recent session first.
- `list_sessions` gains `hours_back` and `directory` bounds, rewritten as a
  single GROUP BY query with no correlated subqueries.
- Mercury ingestion guardrails: a 1M-token per-call input cap, concurrency 4,
  429/Retry-After backoff, and `compact_batch`. Measured against a PAYG key on
  2026-09-15 with real rollout payloads.

### Changed

- Dead research scripts, reports and outputs deleted, README updated.

## [0.3.0] - 2026-09-14

### Added

- `total_recall` MCP tool and `recall` CLI subcommand: two parallel Mercury 2.5
  calls (state summary + user goals, tasks and steers), a recent-rollouts table
  with the current session marked `SUMMARISED`, plan/todo files from
  `.tmp/delegation/` and `~/.vibe/plans/`, assembled metadata-first /
  substance-middle / instructions-last for autoregressive LLMs. Supports
  `--provider mistral` for A/B comparison.
- `MercuryProvider::with_api()` and `new_mistral()` for provider switching;
  `reasoning_effort` is now conditional on Mercury.

### Changed

- Crate renamed to `total-recall` (from `inception-mercury-compaction`).

## [0.2.1] - 2026-09-12

The first tagged release, and the end of the Python prototype.

### Added

- Universal context compaction: detect JSONL (Vibe, Codex, Claude) or SQLite
  (OpenCode, Cursor), find the last compaction point, prune large tool outputs,
  preserve recent messages, and summarize with Inception Mercury 2.5.
- Secret redaction in the prompt path — `redact_secrets()` stripping `sk_*`
  keys, `INCEPTION_API_KEY=*`, `MISTRAL_API_KEY=*` and Bearer tokens before text
  left for the vendor. This was part of the Python implementation
  (`compact.py`), which was removed as dead code in 0.4.0; the redaction did not
  carry into the Rust implementation.
- Pairwise summarization evaluations: Mercury against Mistral Small and against
  Haiku, chunked-vs-full comparisons with timings, and promptfoo prompt-variant
  experiments.
- Release workflow building Linux and macOS; Windows removed from the matrix,
  and the GitHub ARM runner used instead of Blacksmith.

[0.11.1]: https://github.com/prompt-cult/total-recall/compare/v0.11.0...v0.11.1
[0.11.0]: https://github.com/prompt-cult/total-recall/compare/v0.10.0...v0.11.0
[0.10.0]: https://github.com/prompt-cult/total-recall/compare/v0.9.2...v0.10.0
[0.9.2]: https://github.com/prompt-cult/total-recall/compare/v0.9.1...v0.9.2
[0.9.1]: https://github.com/prompt-cult/total-recall/compare/v0.9.0...v0.9.1
[0.9.0]: https://github.com/prompt-cult/total-recall/compare/v0.8.0...v0.9.0
[0.8.0]: https://github.com/prompt-cult/total-recall/compare/v0.7.0...v0.8.0
[0.7.0]: https://github.com/prompt-cult/total-recall/compare/v0.6.2...v0.7.0
[0.6.2]: https://github.com/prompt-cult/total-recall/compare/v0.6.1...v0.6.2
[0.6.1]: https://github.com/prompt-cult/total-recall/compare/v0.5.0...v0.6.1
[0.5.0]: https://github.com/prompt-cult/total-recall/compare/v0.4.2...v0.5.0
[0.4.2]: https://github.com/prompt-cult/total-recall/compare/v0.4.1...v0.4.2
[0.4.1]: https://github.com/prompt-cult/total-recall/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/prompt-cult/total-recall/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/prompt-cult/total-recall/compare/v0.2.1...v0.3.0
[0.2.1]: https://github.com/prompt-cult/total-recall/releases/tag/v0.2.1
