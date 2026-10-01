# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/prompt-cult/total-recall/compare/v0.7.0...HEAD
[0.7.0]: https://github.com/prompt-cult/total-recall/compare/v0.6.2...v0.7.0
[0.6.2]: https://github.com/prompt-cult/total-recall/compare/v0.6.1...v0.6.2
[0.6.1]: https://github.com/prompt-cult/total-recall/compare/v0.5.0...v0.6.1
[0.5.0]: https://github.com/prompt-cult/total-recall/compare/v0.4.2...v0.5.0
[0.4.2]: https://github.com/prompt-cult/total-recall/compare/v0.4.1...v0.4.2
[0.4.1]: https://github.com/prompt-cult/total-recall/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/prompt-cult/total-recall/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/prompt-cult/total-recall/compare/v0.2.1...v0.3.0
[0.2.1]: https://github.com/prompt-cult/total-recall/releases/tag/v0.2.1
