# total-recall

Total-recall MCP tool for log mining of session rollouts as a long-term memory store. Uses [Inception Mercury 3](https://www.inceptionlabs.ai) — a diffusion LLM running at 1,068 tokens/sec — to summarise agent sessions and mine them for goals, tasks, and steers.

## Why

Every CLI coding agent (Claude Code, Codex CLI, OpenCode, Mistral Vibe) hits context limits on long sessions, and frontier models are expensive. Mercury 3 summarises at 1,068 tok/s for $0.10/M input and $0.35/M output (prices as-at the Mercury 3 launch, October 2026).

Augment Code [moved compaction to Mercury and cut latency 82%](https://www.inceptionlabs.ai/blog/introducing-mercury-2-5) (150s to 27s) and costs 90%. This tool brings that same engine — now Mercury 3 — to mining your CLI agent sessions.

## What it does

### `list_sessions` — rollout index

The rollouts of the listing's window with id, title, times, directory,
message and tool counts, byte size, parent/children. Optional bounds:
`hours_back` (a window in hours, 1 or more; 0 or omitted = the 240-hour
default) and `directory` substring filter; `all: true` lists the whole
store. `max_bytes` (default 16,384) is the response budget: the listing
renders as many of the most recent rows of the window as the budget holds
and states how many rows the window holds but did not
print. The bound lives where the data lives: the OpenCode SQLite
implementation pushes the window, the directory substring and the row cap
into SQL — aggregates are computed only for the listed rows, through the
store's `message(session_id, …)` and `part(session_id)` indexes, and the
window count is a bare `COUNT(*)` — and the file-based harnesses test mtime
and directory before reading a payload. A scoped listing costs what it asks
for, not what the store holds.

Each entry also carries `aliases`: other session directory names that resolve
to the same underlying rollout payload. The vibe store can hold two directories
for one rollout (for example a resumed session that kept its content under a
new directory name); `list_sessions` emits one canonical entry per payload —
the id whose directory-name date prefix agrees with the session's start time —
and lists the rest under `aliases`, so a caller paging the index never
processes the same rollout twice. `aliases` is empty for a unique session.
The two id arrays a row carries are bounded: the 10 most recent child ids and
the 10 first alias names travel with the row, `child_session_count` and
`alias_count` state the true totals beside them and the row's `notice` says
what it held back, so no session's subagent fan-out can spend the response
budget on its own — raise `max_bytes` for a wider slice.
When a rollout payload cannot be read (permissions, I/O error), the entry is
still listed — with counts from whatever could be read — and carries a
`read_error` message so damage is visible in the index instead of the
payload silently appearing empty.

Every read path carries the same contract: a payload that cannot be resolved
or read returns a descriptive error naming the path, and the tool surfaces it
as a tool error (or exits 2 on the CLI). "Unreadable" is never reported as
"found nothing". A payload that exists and is genuinely empty is not damage
and still reads as an empty session. `index_sessions` propagates the same
error rather than building an empty index. A JSONL line that fails to parse
is skipped with a `WARN` carrying its line number, so a short read is
diagnosable.

### Bounded extraction — `extract_messages` / `extract_user_messages`

Both return a JSON envelope, never a bare array, so a large session can never
exceed what an MCP client can hold:

```json
{ "session_id": "…", "harness": "vibe", "full": false,
  "bounds": { "total_records": 7525, "returned_records": 100, "offset": 7425,
              "limit": 100, "next_offset": null, "truncated": true,
              "truncation_reason": "record_limit", "max_bytes": 8388608,
              "bytes": 153220, "clamped_record_indices": [],
              "notice": "TRUNCATED: returned records 7425..7525 of 7525 …" },
  "messages": [ … ] }
```

`extract_user_messages` is identical with `user_messages` in place of
`messages`. Defaults: `limit` 100 (max 1000), `max_bytes` 8 MiB (a hard
ceiling, ~2× headroom under typical client limits), `max_record_bytes`
256 KiB per-record clamp. Omit `offset` for the most recent `limit` (a tail
window); pass `offset` (from `0`) and follow `bounds.next_offset` to page the
whole session in bounded chunks. `truncated` and `notice` always state when
output was cut and why (`record_limit` / `byte_cap` / `record_clamp`); an
empty result window (an empty session, or paging past the end) never signals
truncation. The payload is emitted as compact JSON and bounding is computed
on the same compact serialization that is emitted, so the contract holds
exactly. When even one record cannot fit under the budget (fix: raise
`max_bytes` or enable the clamp) the tool emits a descriptive error instead
of silently breaking the cap.

### `extract_by_type` — raw recovery dump

A pure read of the raw rollout store for recovering data from large, partially
corrupt, or poisoned sessions. Selects entries by type — `user`, `assistant`,
`tool`, `thinking`, individually, in combination, or `"all"` — and emits one
record per line with no summarization or aggregation:

```
# {"session_id":"…","harness":"vibe","types":["user","tool"],"bounds":{…}}
user,2026-09-15T09:59:55Z,{"role":"user","content":"…","injected":false}
tool,2026-09-15T10:00:02Z,{"role":"tool","content":"…"}
```

Line 1 is a `#`-prefixed JSON header carrying the same `bounds` envelope
(including `next_offset` and a truncation notice). Each record line is
`type,timestamp,json`: `timestamp` is ISO8601 (or a unix epoch, or `0` when
absent); `json` is the source record serialized compactly, so embedded
newlines are escaped and the one-record-per-line invariant always holds. On
truncation a final `# TRUNCATED: …` line is appended. The stable prefix lets
callers filter and re-merge with standard Unix tools (`awk -F, '$1=="user"'`,
`jq -R 'fromjson'`, `sort -t, -k2`). Output is bounded by the same `limit` /
`offset` / `max_bytes` / `max_record_bytes` parameters as bounded extraction
(with the same exact contract and error behavior). Injected (synthetic) user
records are skipped by default; pass `include_injected: true` (MCP) or
`--include-injected` (CLI) to include them — the record itself carries the
`injected` flag either way. When even one record cannot fit under the budget,
the tool emits a descriptive error (raise `max_bytes` or enable the clamp).
For vibe the records are the byte-faithful source lines re-parsed from
`messages.jsonl`; other harnesses derive entries from the normalized message
stream.

### `extract_qa_pairs` — question/answer pairs

The question/answer pairs from sessions where the harness asked through a
question tool — the question text, its header and options where the format
carries them, paired with the answers the user picked or typed,
chronologically:

```
=== ses_ee8e8f72 (opencode)
--- QA 1 (2026-10-12T08:15:04Z)
HEADER: Branch
OPTIONS:
  - main — the trunk, release when green
  - feature — cut a branch instead
ANSWERS:
  - main
```

For opencode the pairs are read store-side from the `question` tool parts
(`state.input.questions`), the picks under `state.metadata.answers` and the
tool result output as fallback. For vibe, `ask_user_question` arguments'
questions are paired with the following `tool_result` output's answers by
`tool_call_id`. A question that was never answered, or whose prompt was
cancelled, yields no pair. Harnesses without question tools report no
sections, never an error.

MCP tool. Session selection follows the window tools: `session_id` (partial
match) selects one session; empty works the `hours_back` window (default 48,
`0` = omitted) with an optional `directory` substring, and `all: true` is
the explicit whole-store opt-in. In window mode sessions are ordered
most-recent-first, one section each for the sessions that have pairs, the
scanned count stated in the header. Each session's section carries at most
the most recent 100 pairs with the held-back count stated, and the report is
flood-capped at `max_bytes` like every other one.

### `she_said_he_said_action` — matched dialogue and actions

Given case-insensitive terms and a session list (partial IDs) — or, when the
list is empty, all rollouts updated within `hours_back` hours (default 48)
optionally filtered by `directory` substring — extracts per session:

- **HE SAID** — user text parts matching any term
- **SHE SAID** — assistant text parts matching any term
- **THEY DID** — tool calls whose name or arguments match any term

Matching is pushed down into SQLite as a custom scalar function on a
read-only connection: the database is never written, matching runs inside
the query engine, and matching rows stream out in one pass. Synthetic parts
(skill injections, compaction markers) are skipped. Output is a markdown
report, sessions ordered most-recent first. Currently implemented for
OpenCode; other harnesses return a clear unsupported error.

### `index_sessions` / `index` — tantivy shadow indexes

Builds or refreshes a full-text index per session using
[tantivy](https://github.com/quickwit-oss/tantivy), from the normalized
message stream of any harness. One document per message that carries text:
`content` and `thinking` are both indexed; `session_id`, `role`,
`timestamp`, and the message sequence number are stored for display. For
OpenCode, `reasoning` parts become the message's `thinking` text (synthetic
reasoning skipped); other harnesses currently have no thinking content.

The index lives in a shadow folder next to the session store, never inside
it:

| Harness | Shadow root |
|---------|-------------|
| opencode | `~/.local/share/opencode/.tantivy/` |
| vibe | `~/.vibe/logs/.tantivy/` |
| codex | `~/.codex/.tantivy/` |
| claude | `~/.claude/.tantivy/` |

Each session gets `<shadow root>/<session_id>/`, plus a
`total-recall-meta.json` marker (`session_id`, `doc_count`, `built_at`).
Re-indexing a session replaces its index from scratch — the folder is
disposable, can be deleted at any time, and MUST NOT be committed.

MCP: `hours_back` defaults to 24 — the natural call indexes the last day,
not the whole store; `all: true` is the explicit whole-store opt-in.

CLI:

```bash
# Index specific sessions (partial ids) or everything newer than --hours
$B --harness opencode index --sessions ses_f5a4ea5e
$B --harness opencode index --hours 24

# Print the index presence flag alongside the usual session/profile output
$B --harness opencode list
$B --harness opencode --session ses_f5a4ea5e profile
```

### `do_android_dream_of_electric_sheep` — full-text search

Searches the per-session tantivy indexes with tantivy query syntax
(`QueryParser` over the `content` and `thinking` fields), merging the top 10
hits per session into one ranking ordered by score. Session selection is
identical to `she_said_he_said_action`: the `session_id` parameter takes one
partial id, resolving to the most recent match (an unmatched id is reported
in the header, not fatal); empty means the window — all sessions updated
within `hours_back` (default 48), or `all: true` for the whole store,
optionally filtered by `directory` substring. Sessions without an
index are listed as not indexed (run index first) and skipped; a corrupt
index is reported in the header, not fatal.

Hit lines carry the matched text and are marked `thinking` when the term
matched only a message's thinking content:

```
ses_f5a4ea5e | 0.5395 | 2026-09-19T10:02:00Z | ASSISTANT (thinking) | contemplating the electric sheep dream
```

CLI:

```bash
$B --harness opencode do-android-dream-of-electric-sheep --query 'tantivy' --hours 48
$B --harness opencode do-android-dream-of-electric-sheep --query 'shadow AND index' --sessions ses_f5a4ea5e
```

MCP tools over the same adapters; the generated surfaces are the authority
for parameters — `tools/list` for the MCP tools, `total-recall --help` for
the CLI. `list_sessions` and `profile_session` carry a `has_tantivy_index`
flag, set by the server, not by the adapters.

### `profile_session` cache — on by default, staleness-checked

`profile --no-cache` (CLI global flag) and the MCP `profile_session` tool
(`cache: false`) turn the cache off for a call; by default the profile is
served from an on-disk cache inside
the adapter's shadow root: `<shadow root>/tr_<session-id>_meta.json`. One
flat JSON file per session, sibling of the per-session index folders, same
disposable rules as the shadow indexes: never inside the session store,
MUST NOT be committed, safe to delete at any time.

Staleness is checked against one cheap time-updated source — for opencode a
single `SELECT time_updated` row (never the full `list_sessions` aggregates);
for the file-based harnesses the source file's mtime. The cache is fresh when
`time_updated_ms <= cache_mtime_ms + 15_000` (15 s tolerance). Fresh → served
from the cache; stale, missing, or corrupt → recomputed as today and the
cache is rewritten. With `--no-cache` (or `cache: false` on MCP), behaviour is
byte-for-byte the recompute path and no cache file is created.

### The summary cache — blocks

The summary cache is the same shadow-root discipline with a different freshness
key: not wall-clock staleness but the session's **message-id coverage**. A
ledger per session, `<shadow root>/tr_<session-id>_summary.csv`, records one
row per summarised **block** — 11 columns,
`from_id,from_ts,to_id,to_ts,user,assistant,tool,thinking,chars,tokens_est,part_file`
— where the ids are 1-based message sequence numbers in the session stream and
the part texts live beside it in `tr_<session-id>_summary_parts/`. There is no
backward compatibility with the v0.15.0 3-column ledger (alpha): a malformed or
short row invalidates the ledger — it is treated as empty and the file is
dropped; orphaned part files may be clobbered later.

The cache is an explicit, inspectable data structure with three tools around
it (each also a CLI subcommand: `summary-cache`, `summarise-from`,
`cache-delete-latest`):

- **`summary_cache`** — read-only introspection: the cached blocks with their
  id ranges, type counts, chars and token estimates, plus the **tail** (the
  first unsummarised id and what a summarise from there would cost), or
  `tail: null` when everything is covered. Never calls the LLM, never writes.
- **`summarise_from`** (`session_id` + `from_id`) — summarise from an explicit
  id. The guard: `from_id` must be the first unsummarised id (one past the
  last block's `to_id`, or `1` with no blocks) — anything else is an error
  naming the expected id. On success: the composed parts
  (`### part N: from_ts → to_ts (ids from_id..to_id)`) plus the
  `summary cache:` footer, and the new block stored.
- **`cache_delete_latest`** — roll the cache back one block: the newest ledger
  row and its part file are deleted and the new cache status is returned. No
  blocks → an error, not a no-op.

`list_sessions` rows carry a `summary_cache` marker
(`{through_id, through_ts}`, absent when nothing is cached), so a listing
alone shows what each session's cache covers through.

The cached JSON is validated on read through a
[RFC 8927 JSON Type Definition](https://www.rfc-editor.org/rfc/rfc8927)
schema, `schemas/profile-cache.jtd`: `jtd-codegen` compiles it into the
standalone validator in `src/profile_cache_types.rs`, and any cached payload
that fails that gate (or whose `event_type` strings are not known
`EventType` variant names) is treated as corrupt and recomputed. Regenerate
the validator after changing the schema:

```bash
cargo install --locked --git https://github.com/prompt-cult/json-type-definition-RFC-8927 --rev 767806d42f3ff4654503a6207fe8a18d75fe8211 jtd-codegen
jtd-codegen --target rust schemas/profile-cache.jtd > src/profile_cache_types.rs
```

(The generated file also carries the typed envelope and the
`TryFrom` conversion into `SessionProfile`; re-apply that adaptation after
regenerating.)

## The scope contract

The natural call is the cheap call. Every tool defaults to the smallest
useful scope, work is bounded where the data lives (a window or a directory
is pushed into the store query, so a call costs what it asks for, not what
the store holds), and breadth is an explicit opt-in — never a default, never
a silent clamp.

- **Session tools** (`profile_session`, `extract_messages`,
  `extract_user_messages`, `extract_by_type`,
  `prior_period_summary`, `todo_history`): `session_id` partial match; empty = the
  most recent session.
- **Window tools** (`list_sessions`, `she_said_he_said_action`,
  `extract_qa_pairs`, `index_sessions`,
  `do_android_dream_of_electric_sheep`): `session_id`
  selects explicitly; when empty the tool works its window — `hours_back`
  (a window in hours, `1` or more) and optionally `directory`. `0` means
  "omitted", so it takes the tool's default window — never the whole store.
  `all: true` (the explicit whole-store opt-in) combined with a real
  `hours_back` is rejected as the contradiction it is. Every rejection names
  the cheap forms — the error is the documentation. The same rule governs
  every numeric input: `limit` above 1000, a `max_bytes` above the 8 MiB
  ceiling, a 0-based `line`/`start`, and an empty search query.
- Unknown parameters are rejected by name — a typo is a silently unscoped
  call otherwise — and every served schema states the bounds the handlers
  enforce (`minimum`, `maximum`).
- Listings render as many of the most recent rows as the response budget
  holds (at most the 200 the store query bounds) and state how many rows the
  window holds but did not print.
- The CLI keeps its own doctrine: stdout is a stream, `0` means unbounded,
  and the command typed is the scope asked for. The MCP tools — the ones an
  agent calls on the caller's behalf — reject the unscoped call instead.

## Ingestion guardrails

Mercury calls are guarded by measured, documented limits (probed against a
Pay-As-You-Go key on 2026-09-15 with real rollout payloads, 5k/10k/20k
tokens, concurrency ramped 1→64). The free tier carries the same caps —
1,000 requests/min, 1M input tokens/min, 100k output tokens/min — and
exceeding any of them returns HTTP 429:

- **The binding limit is ~1M input tokens/minute** (the documented input
  cap). Requests/min (1,000) and output tokens/min (100,000) never bind for
  compaction workloads.
- **Per-call input cap: 450,000 tokens — 90% of Mercury 3's 500K context
  window** (half of the model's 1M window, deliberately, so summarisation
  keeps headroom and sends more of the session intact rather than pruning
  harder; ~4 chars/token), leaving the rest for the completion. A 100MiB
  rollout is never slung in one call; an over-cap prompt is rejected with an
  error, never truncated. Practical calls stay far below the cap — tool
  results are snipped to 500 chars in prompt assembly, and a prompt built
  from a whole session is held to a byte budget, so an over-cap prompt is
  never assembled in the first place.
- **Concurrency: 4 in-flight requests** with ~10k-token prompts. Measured
  sweet spot: ~22k input tok/s with zero rejections, p50 latency 1.9s
  (latency is flat across payload sizes and concurrency). Beyond
  concurrency 8 the 429 wall arrives with no throughput gain.
- **429 handling**: at most 5 retries, then a descriptive error naming the
  status and the exhausted budget — never a silent drop. `Retry-After`
  (seconds form) is honoured in full and capped at 10s per attempt, so
  `Retry-After: 0` comes back immediately; with no parseable header the wait
  doubles from 1s (1s, 2s, 4s, 8s, capped at 10s). Every 429 is logged via
  `tracing`.
- **5xx handling**: 3 retries at ~1s spacing, then an error naming the
  status. The two budgets are independent: 429s never consume 5xx retries
  or vice versa.

These contracts are asserted against a local mock server in
`tests/mercury_guardrails.rs`; no test ever calls the real API.

### `prior-period-summary` — a summary of a prior period

An orientation summary of a prior period of work. The period is the work
since the session's last compaction boundary — the work after that boundary.
A session with no compaction points has no earlier period, so the
summary covers the whole session up to now. On opencode the compaction
boundary is the typed compaction part, which prose quoting the marker text
cannot forge.

1. Makes two parallel Mercury 3 calls: current state summary + user goals/tasks/steers
2. Lists recent rollouts (24h filter, configurable) with the current session marked SUMMARISED
3. Lists plan/todo files from `.tmp/delegation/` and `~/.vibe/plans/`
4. Folds in the period's todo log (opencode) — items opened and closed with
   timings, the state-change history, and a churn note when items open
   faster than they close, with the human's recent steers read into it —
   and the commits made in the period
5. Appends a compaction-points FYI footer (the last few compaction points
   with message/line counts since each) and a fixed orientation warning:
   the summary does not attempt to show what was in-flight or the next
   steps — those are known from the todo list, the user's most recent
   inputs, and the commit history — so take stock and get your bearings
   before picking up the work
6. Assembles output in deliberate ordering for autoregressive LLMs: metadata first, ground truth next, substance middle, action last

CLI: `total-recall --harness <h> --session <id> prior-period-summary [--provider mistral]`.

## Supported tools

| Tool | Storage | Chat history repos / gists | Rust CLI |
|------|---------|---------------------------|----------|
| Mistral Vibe | JSONL (`~/.vibe/logs/session/`) | [gist: mistral-vibe-chat-history](https://gist.github.com/simbo1905/b79ba81f637e9e235d55e4853e3dc299) | Yes |
| OpenCode | SQLite (`~/.local/share/opencode/`) | [simbo1905/opencode-chat-history](https://github.com/simbo1905/opencode-chat-history) | Yes (`--harness opencode`) |
| Codex CLI | JSONL + SQLite | [simbo1905/codex-chat-history](https://github.com/simbo1905/codex-chat-history) | Yes |
| Claude Code | JSONL | [simbo1905/claude-chat-history](https://github.com/simbo1905/claude-chat-history) | Yes |

## Compaction prompt research

The compaction prompts and strategies in this tool are informed by [badlogic's context compaction research](https://gist.github.com/badlogic/cd2ef65b0697c4dbe2d13fbecb0a0a5f) comparing Claude Code, Codex CLI, OpenCode, and Amp.

Key findings from that research:
- Claude Code triggers at ~95% context capacity, uses a summary prompt
- Codex CLI preserves recent user messages (last ~20k tokens) alongside the summary
- OpenCode has a separate "prune" mechanism for tool outputs beyond 40k tokens
- Amp uses manual "handoff" instead of auto-compaction

This tool takes the best of each: it preserves recent messages, prunes large tool outputs, and uses Mercury 3 for fast summarization.

## Build features

The LLM vendor is a **compile-time cargo feature**. A vendor is a third-party
service that can be acquired, renamed or disappear entirely, so no build of
this tool requires one: the default build has both vendors, and a vendor-free
build is a first-class artifact.

```toml
[features]
default = ["mercury", "mistral"]
mercury = ["dep:dotenvy"]   # Inception Mercury 3 (the default provider)
mistral = ["dep:dotenvy"]   # Mistral, selected with --provider mistral
```

```bash
cargo build --release                        # both vendors (default)
cargo build --release --no-default-features   # vendor-free
```

A **vendor-free build makes no LLM calls and needs no API key** — it never
reads `INCEPTION_API_KEY` or `MISTRAL_API_KEY`, and does not even load a `.env`
file.

| Tool | Vendor-free build | Default build |
|------|-------------------|---------------|
| `list_sessions`, `profile_session`, `extract_messages`, `extract_user_messages`, `extract_by_type`, `extract_qa_pairs`, `she_said_he_said_action`, `index_sessions`, `do_android_dream_of_electric_sheep`, `todo_history`, `summary_cache`, `cache_delete_latest` | works | works |
| `summarise_from` (MCP), `summarise-from` (CLI) | registered; the guard and a covered tail serve the cache with no LLM call, a real summarise returns an error naming the missing feature | calls Mercury only when there is something new to summarise, or `--provider mistral` |
| `prior_period_summary` (MCP), `prior-period-summary` (CLI) | registered; returns an error naming the missing feature | calls Mercury, or `--provider mistral` |

The LLM-backed **MCP tool names stay in `tools/list` in every build** — clients
bind by name — and the call itself returns the error:

```
LLM provider `mercury` is not compiled into this build: it was built without the
`mercury` cargo feature. Rebuild with `cargo build --release --features mercury`,
or use the vendor-free build (`--no-default-features`) — it makes no LLM calls and
needs no API key, and every log-mining tool still works. Vendors compiled into
this build: none.
```

`--provider` selects between the vendors compiled into the build
(`mercury`, `mistral`); a vendor that is not compiled in fails the same way, and
an unrecognised name is refused rather than silently falling back to Mercury.
Vendor identity (endpoint, model, key variable) is the gated part; the
OpenAI-compatible chat transport itself compiles in every build but is
unreachable without a vendor constructor.

## Usage

```bash
cargo build --release
B=./target/release/total-recall

# Version (matches the Cargo package / release tag)
$B --version

# List rollouts for a harness (--harness is required; vibe | codex | claude | opencode)
$B --harness vibe list

# Profile: counts, compaction markers, interesting events (cache is on by default)
$B --harness vibe --session 4836855e profile
$B --harness vibe --session 4836855e --no-cache profile

# Extract what the user said verbatim
$B --harness vibe --session 4836855e user-messages --markdown

# Raw recovery dump by type (one `type,timestamp,json` record per line)
$B --harness vibe --session 4836855e extract-by-type --type user --type thinking
# CLI extraction is unbounded by default; bound it explicitly when paging large
# sessions (--limit/--offset/--max-bytes/--max-record-bytes, notice on stderr)
$B --harness vibe --session 4836855e extract --limit 100 --offset 0

# Todo-list edits as a JSONL event stream (one {ts,action,todo} object per line)
$B --harness opencode --session ses_ee8e8f72 todo-history

# The summary cache as an explicit data structure: inspect it, summarise an
# explicit range, roll it back one block
$B --harness vibe --session 4836855e summary-cache
$B --harness vibe --session 4836855e summarise-from --from-id 5
$B --harness vibe --session 4836855e cache-delete-latest

# Total recall: state summary + user goals/steers + rollouts table + plan files
$B --harness vibe --session 4836855e recall

# She-said/he-said/they-did: matched dialogue and tool actions
$B --harness opencode he-said-she-said --words git,branch,tag,worktree --hours 48 --directory uvrr-core
$B --harness opencode he-said-she-said --words git --sessions ses_f5a4ea5e ses_f5b87b1c

# Build/refresh per-session tantivy shadow indexes, then full-text search them
$B --harness opencode index --sessions ses_f5a4ea5e
$B --harness opencode do-android-dream-of-electric-sheep --query 'compaction' --sessions ses_f5a4ea5e

# Total recall with Mistral instead of Mercury (for A/B comparison)
$B --harness vibe --session 4836855e --provider mistral recall
```

Requires `INCEPTION_API_KEY` in `.env` (see `.env.template`).
For `--provider mistral`, set `MISTRAL_API_KEY` instead. A vendor-free build
(`--no-default-features`) needs neither — see [Build features](#build-features).

## Environment: storage-root overrides and the sandbox guard

Each adapter resolves its storage root from `$HOME` by default (the live
store). To point the CLI and MCP server at fixtures or a scratch copy instead
— without ever touching live sessions — set the harness's root override.
Precedence: an explicit `with_root` (tests) > the env override (non-empty) >
the `$HOME`-derived default. An empty value counts as unset; an override that
resolves to nothing usable is an error, never a silent fall-back to `$HOME`.

| Harness | Env var | Default root |
|---------|---------|--------------|
| vibe | `TOTAL_RECALL_VIBE_ROOT` | `~/.vibe/logs/session/` |
| claude | `TOTAL_RECALL_CLAUDE_ROOT` | `~/.claude/projects/` |
| codex | `TOTAL_RECALL_CODEX_ROOT` | `~/.codex/sessions/` |
| opencode | `TOTAL_RECALL_OPENCODE_ROOT` | `~/.local/share/opencode/opencode.db` (a directory also works; `opencode.db` inside it is used) |

Both vibe store layouts are read: pre-3.x releases keep the log in flat
`session_*/messages.jsonl` directories; 3.0 and later write each session to a
unified store under `unified/<uuid>/` (a `CURRENT` pointer, `meta.json`, a
hash-chained `journal/*.jsonl` replay and content-addressed chunks). The
adapter reads both side by side — unified sessions by journal replay, legacy
sessions by messages.jsonl — so an upgrade to vibe 3.x loses nothing. Both
stores are opened read-only.

Both opencode store layouts are read: pre-2.2 releases keep the log in
`session`/`message`/`part`; 2.2 and later write it to
`session_v2`/`session_message` (parts inlined in each row's data JSON,
`seq`-ordered) and migrate every old session over. total-recall detects the
layout per store, reads v2 where the pair exists — the frozen legacy copy would
hide the sessions 2.2 wrote after the bump — and falls back to the legacy
tables otherwise. The store is always opened read-only.

Set `TOTAL_RECALL_SANDBOX=1` to refuse to build any adapter whose root did not
come from its override. With the sandbox armed and no override set,
`make_adapter` fails closed with a descriptive error instead of reading the
live store — a mechanical guarantee that sandboxed dev/test runs never read
live sessions.

## MCP server

The binary runs as an MCP stdio server exposing `harness`, `list_sessions`
(with optional `hours_back`/`directory` bounds and a `has_tantivy_index`
flag), `profile_session` (the on-by-default cache; `cache: false` to recompute), `extract_messages`,
`extract_user_messages`, `extract_by_type`, `extract_qa_pairs`,
`summary_cache`, `summarise_from`,
`cache_delete_latest`, `she_said_he_said_action`, `index_sessions`,
`do_android_dream_of_electric_sheep`, `prior_period_summary`, `todo_history`,
and `line_histogram`:

Every tool is session-scoped through a `session_id` parameter (partial match,
empty = most recent) except the window tools, which take a `hours_back`
window instead (a window in hours, 1 or more; `list_sessions` defaults to
240 hours = 10 days; `all: true` opts into the whole store). The generated
schemas are the parameter authority: `tools/list` for the MCP tools,
`--help` for the CLI.

**Flood control.** Every response that can overflow the caller's context is
capped at `max_bytes` (default 16,384) — the mechanism is generic, not a
fixed tool list. An overflowing response is written whole to a private file
under the user's temp directory (`$TMPDIR/total-recall/`, mode 600); the
returned text is cut at a line boundary (JSON responses are never torn —
the marker returns alone) and ends with a line-oriented EOF marker naming
the tool, the byte and line counts, the full file's path, and the 24-hour
prune policy. The marker carries the file's line histogram (the vendored
`line_histogram.awk`), and the `line_histogram` tool pages any file by line
range — so nothing is lost and nothing floods a context window.

**Bounded prompts.** The two LLM prompts the recall path sends are byte-bounded
on the way in, on the same principle: the newest context is kept, the oldest
dropped, and the drop reported — in a marker line inside the prompt and as the
prompt byte sizes and dropped counts in the timing comment the response already
carries. A session too large to summarize in one call therefore costs a bounded
prompt instead of a request that cannot be ingested.

```bash
$B mcp
```

Each server instance is bound to exactly ONE harness, set by the installing
config via the `HARNESS` environment variable (or `--harness <name>` on the
`mcp` subcommand — the flag wins if both are given). Tools take no harness
parameter; the `harness` tool reports the bound value. If the harness is
unset or unknown the server refuses to start: it prints the reason to stdout
and stderr and exits 2.

Keep the server key/name as `total-recall`: the MCP tool names derive from
it (`total-recall_prior_period_summary`, …), so a renamed server shows up in the
agent's tool surface under the wrong identity.

Register it in Mistral Vibe (`~/.vibe/config.toml`):

```toml
[[mcp_servers]]
name = "total-recall"
transport = "stdio"
command = "/path/to/total-recall"
args = ["mcp"]

[mcp_servers.env]
HARNESS = "vibe"
INCEPTION_API_KEY = "sk_..."
```

Register it in OpenCode (`~/.config/opencode/opencode.jsonc`):

```json
"mcp": {
  "total-recall": {
    "type": "local",
    "command": ["/path/to/total-recall", "mcp"],
    "environment": {"HARNESS": "opencode", "INCEPTION_API_KEY": "sk_..."}
  }
}
```

The snippets carry `INCEPTION_API_KEY` because the spawned server only finds
a `.env` file relative to its working directory — which the MCP client
controls, not you. Alternatively export `INCEPTION_API_KEY` in the shell that
launches the client; the server inherits it.

## Operations

- [opencode store vacuum](docs/opencode-store-vacuum.md) — measured breakdown
  of the opencode SQLite session store and the safe prune/vacuum/swap
  procedure.
- [`scripts/opencode-db-vacuum.sh`](scripts/opencode-db-vacuum.sh) — the
  helper script implementing that procedure (`check`, `counts`, `backup`,
  `vacuum-into`, `prune-events`, `swap`, `verify`). The store path comes from
  the `OPENCODE_DB` env var.

## Troubleshooting

- **"LLM provider `mercury` is not compiled into this build"** — the binary
  was built with `--no-default-features` (or without that vendor's feature).
  Rebuild with `cargo build --release --features mercury`, or keep the
  vendor-free build and use the log-mining tools, which need no key.

- **401 "Incorrect API key" from `prior_period_summary`** — the MCP
  server's environment block is missing `INCEPTION_API_KEY`. List/profile
  work without it (no LLM call); Mercury calls fail. Add the key to the
  snippet's environment (see MCP server above) or export it in the launching
  shell.

## Setup

1. Get an Inception API key from [https://platform.inceptionlabs.ai](https://platform.inceptionlabs.ai)
2. Copy `.env.template` to `.env` and fill in your key (build-from-source CLI path; MCP servers get the key from their registration snippet's environment instead)
3. `cargo build --release` and use the CLI above

Steps 1–2 are only needed for the LLM-backed tools (`prior-period-summary`).
Every log-mining tool works from a
vendor-free build with no key at all.

<!-- BEGIN: distribution (ticket 452) -->
## Install a release binary

Prebuilt archives ship on every tagged release. Asset names are
`total-recall_<version>_<target-triple>.tar.gz` — for example
`total-recall_0.7.0_x86_64-unknown-linux-gnu.tar.gz` — and each archive holds
one executable, `total-recall`, at its root.

### mise

```bash
mise use github:prompt-cult/total-recall
```

mise selects the archive for your OS and CPU by reading the triple out of the
asset name, so the same line installs the right binary on macOS arm64, macOS
x86_64, Linux x86_64 and Linux arm64. It also fetches the release's
`SHA256SUMS` and verifies the download against it.

> The older `mise use ubi:prompt-cult/total-recall` spelling still resolves,
> but mise has deprecated the ubi backend in favour of the `github:` backend.

### aqua

aqua resolves assets through a registry entry rather than by inspecting release
names, so it needs a `registry.yaml` mapping this project's assets. The asset
layout above is what such an entry targets:

```yaml
packages:
  - type: github_release
    repo_owner: prompt-cult
    repo_name: total-recall
    asset: 'total-recall_{{trimV .Version}}_{{.Arch}}-{{.OS}}.tar.gz'
    format: tar.gz
    replacements:
      amd64: x86_64
      arm64: aarch64
      darwin: apple-darwin
      linux: unknown-linux-gnu
```

`aqua g -i prompt-cult/total-recall` works once that entry is in a registry
aqua is configured to read.

### Plain download

```bash
VERSION=0.7.0
curl -fsSLO "https://github.com/prompt-cult/total-recall/releases/download/v${VERSION}/total-recall_${VERSION}_x86_64-unknown-linux-gnu.tar.gz"
curl -fsSLO "https://github.com/prompt-cult/total-recall/releases/download/v${VERSION}/SHA256SUMS"
sha256sum --check --ignore-missing SHA256SUMS
tar -xzf "total-recall_${VERSION}_x86_64-unknown-linux-gnu.tar.gz"
install -m 0755 total-recall /usr/local/bin/
```

Substitute the triple for your platform: `aarch64-apple-darwin`,
`x86_64-apple-darwin`, `x86_64-unknown-linux-gnu`, or
`aarch64-unknown-linux-gnu`.

### Verifying a download

Every release carries a `SHA256SUMS` over all four archives, one line each,
`<64 hex digits>` then two spaces then the bare asset filename. `sha256sum
--check SHA256SUMS` verifies them all; `--ignore-missing` verifies only the
archives you downloaded. mise reads the same file automatically. The archives
are built reproducibly, so a given source tree always yields the same digest.

Building from source instead: `cargo build --release`.
<!-- END: distribution (ticket 452) -->

## Requirements

- Rust toolchain (stable)
- `INCEPTION_API_KEY` in environment or `.env` (default build only)
- Optional: `MISTRAL_API_KEY` for `--provider mistral`

## Mercury 3 pricing

Prices as-at the Mercury 3 launch, October 2026.

| | Launch (50% off) | Normal |
|---|---|---|
| Input | $0.10/M tokens | $0.20/M tokens |
| Cached input | $0.01/M tokens | $0.02/M tokens |
| Output | $0.35/M tokens | $0.70/M tokens |
| Speed | 1,068 tokens/sec | same |
| Context | 1M tokens | same |

100M free tokens for new accounts.

## Documentation

The documentation site is at <https://prompt-cult.github.io/total-recall/>,
built from [`book.toml`](book.toml) and [`docs/src/`](docs/src) by
[`.github/workflows/pages.yml`](.github/workflows/pages.yml).

| Document | What it covers |
|----------|----------------|
| [docs site](https://prompt-cult.github.io/total-recall/) | installation, MCP registration, per-tool parameters, configuration, troubleshooting |
| [README](README.md) | this file — tools, build features, usage |
| [SECURITY.md](SECURITY.md) | key handling, what leaves your machine, read-only store access, reporting a vulnerability |
| [CONTRIBUTING.md](CONTRIBUTING.md) | the working contract is [`AGENTS.md`](AGENTS.md); the test suite is the review |
| [CHANGELOG.md](CHANGELOG.md) | every release, derived from the tag history |
| [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md) | Contributor Covenant 3.0 |
| [LICENSE-MIT](LICENSE-MIT) · [LICENSE-APACHE](LICENSE-APACHE) | dual licensed `MIT OR Apache-2.0` |
| [docs/opencode-store-vacuum.md](docs/opencode-store-vacuum.md) | store-vacuum runbook (see [Operations](#operations)) |
