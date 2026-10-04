# Tools

Every MCP tool has a CLI equivalent over the same adapter. Parameter
surfaces are not duplicated here: the generated schemas are the authority —
`tools/list` for the MCP tools (derived from the source descriptions) and
`total-recall --help` for the CLI (derived from the `clap` definitions).
Two generic contracts cover every tool, including ones added later:

- the scope contract — see [The scope contract](#the-scope-contract): the
  natural call is the cheap call; breadth is explicit, never a default, and
  every request outside the cheap forms is rejected with those forms named,
  never clamped, never silently run
- every response that can overflow the caller's context is flood-capped —
  see [Flood control](#flood-control)

## The scope contract

Work is scoped before it starts, and the bound lives where the data lives:
a window or a directory is pushed into the store query, so a call costs
what it asks for, not what the store holds.

Two regimes, one parameter:

- **session tools** — `profile_session`, `extract_messages`,
  `extract_user_messages`, `extract_by_type`, `compact_session`,
  `total_recall`: `session_id` (partial match; empty = the most recent
  session). One session is the smallest useful scope, so it is the default.
- **window tools** — `list_sessions`, `she_said_he_said_action`,
  `index_sessions`, `do_android_dream_of_electric_sheep`: `session_id`
  (partial match) selects sessions explicitly; when empty, the tool works
  its window — `hours_back` (a window in hours, `1` or more; each tool
  carries its own default) and optionally `directory` (substring).
  `hours_back` is never "no bound": `0` means "omitted" and takes the
  tool's default window, so a client that serialises an unset window as
  zero gets the default window and never the whole store.

Breadth beyond a window is explicit: the window tools take `all: true` for
the whole store — every session of every project the store holds — as a
deliberate opt-in, and `all` together with a real `hours_back` is rejected
as the same contradiction. Every rejection names the cheap forms — pass
`session_id`, or `directory`, or `hours_back: 1` or more, or `all: true` on
purpose — because the error is the documentation. Rejection, not clamping: a
silent clamp would answer a smaller question than the one that was asked.
The same house rule governs every other input a tool can be handed: `limit`
above 1000, `max_bytes` above the 8 MiB ceiling, a 0-based `line`/`start`, an
unknown parameter, and an empty search query.

Every params struct rejects unknown fields by name: a typo (`hour_back`,
`directorys`) is a silently unscoped call otherwise, so the error names the
unrecognised field and the fields the tool does accept. Every served schema
states the bounds its handler enforces (`minimum`, `maximum`), so a
schema-honouring client cannot send a value the handler will reject.

Listings are bounded the way reports are: the listing renders as many of the
most recent rows as `max_bytes` holds — at most the 200 rows the store query
bounds — and states how many rows the window holds but did not print. A
default `list_sessions` therefore returns rows; when a single row is larger
than the whole budget the response is handed to [flood
control](#flood-control) whole, which is the same escape hatch every other
report has.

The CLI keeps its own doctrine — stdout is a stream, `0` means unbounded,
and the command you typed is the scope you asked for. The MCP tools are
the ones an agent calls on the caller's behalf, so they are the ones that
reject the unscoped call.

## Global CLI flags

Accepted before the subcommand:

| Flag | Effect |
|------|--------|
| `--harness <name>` | `vibe` \| `codex` \| `claude` \| `opencode`. Required for every subcommand except `mcp` |
| `--session <id>` | partial session id; empty means the most recent session |
| `--json` | pretty JSON, for `list` and `profile` |
| `--markdown` | markdown table for `list`; readable blocks for `extract`, numbered list for `user-messages` |
| `--full` | read the whole rollout instead of from the last compaction point |
| `--verbose` | trace-level logging on stderr |
| `--provider <name>` | `mercury` (default) or `mistral`, for the LLM-backed subcommands only |
| `--limit` / `--offset` / `--max-bytes` / `--max-record-bytes` | extraction bounds; see [Bounded output](#bounded-output) |
| `--include-injected` | include synthetic (injected) user records in `extract-by-type` |
| `--cache` | serve `profile` from the opt-in on-disk cache |

Output format per subcommand, with no flag given: `list` and `profile` print a
human-readable summary, `extract` prints one JSON object per line,
`user-messages` prints a JSON array, and `extract-by-type` always prints
`type,timestamp,json` records. `he-said-she-said`,
`do-android-dream-of-electric-sheep` and `index` print their own report format
regardless of flags.

CLI extraction is unbounded by default — stdout is a stream, and 0 means
unbounded. The MCP tools are always bounded. A CLI truncation notice goes to
stderr so stdout stays machine-parseable.

## Read tools

### `harness`

No parameters. Returns the harness this server is bound to, as JSON. The
cheapest confirmation that a registration is right. No CLI equivalent — the CLI
takes `--harness` explicitly.

### `list_sessions`

MCP: `hours_back` (a window in hours, `1` or more; `0` or omitted = the
240-hour default), `directory` (substring), `all: true` (the whole store),
`max_bytes` (the response budget, default 16,384). `window_count` is how many
rows the window holds, `held_back` how many of them the response did not
print, and `max_bytes` the budget that decided it.

CLI: `total-recall --harness <h> list [--json|--markdown] [--hours N]` —
`--hours 0` (the default) is the unbounded full listing stream; `--hours N`
with `N >= 1` is the scoped listing (at most the 200 most recent rows of the
window, held-back count on stderr).

Each entry: `session_id`, `title`, `start_time`, `end_time`, `file_size`,
`line_count`, `user_count`, `assistant_count`, `tool_count`,
`has_compaction`, `directory`, `parent_session_id`, `child_sessions`,
`has_tantivy_index`, `aliases`, and `read_error` when present.

`aliases` lists other session directory names that resolve to the same
underlying rollout payload — the vibe store can hold two directories for one
rollout, for example a resumed session that kept its content under a new
directory name. One canonical entry is emitted per payload, and the rest are
listed under `aliases`, so a caller paging the index never processes the same
rollout twice. `aliases` is empty for a unique session.

`read_error` is set when the payload could not be read: the entry is still
listed, with counts from whatever could be read, so damage is visible in the
index rather than the payload silently appearing empty.

The listing is bounded the way the scope contract says: most recent rows
first, at most the 200 the store query bounds and as many of those as
`max_bytes` holds, with the held-back count stated. A default
`list_sessions` returns rows, never the flood cap's overflow marker; when a
single row is larger than the whole budget the response is handed to flood
control whole, which writes it out and returns the marker — the escape hatch
every other report has. The bound lives where the data lives — the OpenCode
implementation pushes the window, the directory substring and the row cap
into SQL, so
aggregates are computed only for the listed rows (through the store's
`message(session_id, …)` and `part(session_id)` indexes) and the window count
is a bare `COUNT(*)`; the file-based harnesses test mtime and directory
before reading a payload. A scoped listing costs what it asks for, not what
the store holds.

### `profile_session`


CLI: `total-recall --harness <h> --session <id> profile [--cache]`.

Returns `session_id`, `file_size`, `line_count`, `first_ts`, `last_ts`,
`role_counts`, `has_tantivy_index` and `interesting_events`. An interesting
event carries `line_number`, `gap_lines`, `event_type` and `summary`;
`event_type` is one of `Compaction`, `TodoCreate`, `TodoUpdate`, `TodoDelete`,
`GitCommit`, `GitPush`, `GitTag`, `UserMessage`.

#### The profile cache

Opt-in, staleness-checked. With `cache` (or `--cache`), the profile is served
from a flat JSON file inside the adapter's shadow root:
`<shadow root>/tr_<session-id>_meta.json`, sibling of the per-session index
folders. The folder is disposable, can be deleted at any time, and MUST NOT be
committed.

Staleness is checked against one cheap time-updated source — for opencode a
single `SELECT time_updated` row, never the full `list_sessions` aggregates;
for the file-based harnesses the source file's mtime. The cache is fresh when
`time_updated_ms <= cache_mtime_ms + 15_000` (15 s tolerance). Fresh → served
from the cache; stale, missing, or corrupt → recomputed and the cache is
rewritten. With the flag off, no cache file is created at all.

A cached payload is validated through an
[RFC 8927 JSON Type Definition](https://www.rfc-editor.org/rfc/rfc8927) schema,
`schemas/profile-cache.jtd`, compiled by `jtd-codegen` into the standalone
validator in `src/profile_cache_types.rs`. A payload that fails the gate — or
whose `event_type` strings are not known `EventType` variant names — is
treated as corrupt and recomputed.

## Bounded output

`extract_messages`, `extract_user_messages` and `extract_by_type` return a
JSON envelope, never a bare array, so a large session can never exceed what an
MCP client can hold:

```json
{ "session_id": "…", "harness": "vibe", "full": false,
  "bounds": { "total_records": 7525, "returned_records": 100, "offset": 7425,
              "limit": 100, "next_offset": null, "truncated": true,
              "truncation_reason": "record_limit", "max_bytes": 8388608,
              "bytes": 153220, "clamped_record_indices": [],
              "notice": "TRUNCATED: returned records 7425..7525 of 7525 …" },
  "messages": [ … ] }
```


Omit `offset` for the most recent `limit`; pass `offset` (from `0`) and follow
`bounds.next_offset` to page the whole session in bounded chunks.
`truncated` and `notice` always state when output was cut and why
(`record_limit` / `byte_cap` / `record_clamp`, comma-joined). An empty result
window — an empty session, or paging past the end — never signals truncation.
The payload is emitted as compact JSON and bounding is computed on that same
compact serialization, so the cap holds exactly. When even one record cannot
fit under the budget, the tool emits a descriptive error instead of silently
breaking the cap: raise `max_bytes`, or set `max_record_bytes` to enable the
clamp.

### `extract_messages`


CLI: `total-recall --harness <h> --session <id> extract [--full] [--limit N]
[--offset N] [--max-bytes N] [--max-record-bytes N]`.

### `extract_user_messages`

`session_id`, plus `limit`, `offset`, `max_bytes`, `max_record_bytes`. There is
no `full`: user messages are always derived from the whole rollout.
Injected (synthetic) user records are skipped; the envelope is identical to
`extract_messages` with `user_messages` in place of `messages`.

CLI: `total-recall --harness <h> --session <id> user-messages`.

### `extract_by_type`

A pure read of the raw store, for recovering data from large, partially
corrupt, or poisoned sessions. Emits one record per line with no summarization
or aggregation.


```
# {"session_id":"…","harness":"vibe","types":["user","tool"],"bounds":{…}}
user,2026-09-15T09:59:55Z,{"role":"user","content":"…","injected":false}
tool,2026-09-15T10:00:02Z,{"role":"tool","content":"…"}
```

Line 1 is a `#`-prefixed JSON header carrying the same `bounds` envelope,
including `next_offset` and a truncation notice. Each record line is
`type,timestamp,json`: `timestamp` is ISO8601, a unix epoch, or `0` when
neither; `json` is the source record serialized compactly, so embedded
newlines are escaped and the one-record-per-line invariant always holds. On
truncation a final `# TRUNCATED: …` line is appended. The stable prefix lets
callers filter and re-merge with standard Unix tools:

```bash
awk -F, '$1=="user"'        # filter by type
jq -R 'fromjson'            # re-parse the JSON column
sort -t, -k2                # order by timestamp
```

Injected records are skipped by default; the record itself carries the
`injected` flag either way. For vibe the records are the byte-faithful source
lines re-parsed from `messages.jsonl`; other harnesses derive entries from the
normalized message stream.

CLI: `total-recall --harness <h> --session <id> extract-by-type --type user
--type thinking [--include-injected]`.

## Matched dialogue and actions

### `she_said_he_said_action`


Per session it extracts:

- **HE SAID** — user text parts matching any term
- **SHE SAID** — assistant text parts matching any term
- **THEY DID** — tool calls whose name or arguments match any term

Matching is pushed down into SQLite as a custom scalar function on a
read-only connection: the database is never written, matching runs inside the
query engine, and matching rows stream out in one pass. Synthetic parts (skill
injections, compaction markers) are skipped. Output is a markdown report,
sessions ordered most-recent first. An unmatched session_id is
reported in the header, not fatal. Currently implemented for OpenCode; other
harnesses return a clear unsupported error.

CLI: `total-recall --harness opencode he-said-she-said --words git,branch,tag
[--sessions <id>…] [--hours 48] [--directory <substr>]`.

## Indexing and full-text search

### `index_sessions`


Builds or refreshes a [tantivy](https://github.com/quickwit-oss/tantivy)
full-text index per session, from the normalized message stream of any harness.
One document per message that carries text: `content` and `thinking` are both
indexed; `session_id`, `role`, `timestamp` and the message sequence number are
stored for display. For OpenCode, `reasoning` parts become the message's
`thinking` text (synthetic reasoning skipped); other harnesses currently have
no thinking content.

The index lives in a shadow folder next to the session store, never inside it:

| Harness | Shadow root |
|---------|-------------|
| `opencode` | `~/.local/share/opencode/.tantivy/` |
| `vibe` | `~/.vibe/logs/.tantivy/` |
| `codex` | `~/.codex/.tantivy/` |
| `claude` | `~/.claude/.tantivy/` |

Each session gets `<shadow root>/<session_id>/`, plus a
`total-recall-meta.json` marker carrying `session_id`, `doc_count` and
`built_at`. Re-indexing a session replaces its index from scratch — the
folder is disposable, can be deleted at any time, and MUST NOT be committed.

MCP: `hours_back` defaults to 24 — the natural call indexes the last day,
not the whole store; `all: true` is the explicit whole-store opt-in.

CLI: `total-recall --harness <h> index [--sessions <id>…] [--hours 24]
[--directory <substr>]`. `list` and `profile` carry the index presence flag
alongside their usual output.

### `do_android_dream_of_electric_sheep`


Searches the per-session indexes with a `QueryParser` over the `content` and
`thinking` fields, merging the top 10 hits per session into one ranking
ordered by score. The session_id resolves to the most recent match;
an unmatched id is reported in the header, not fatal. Sessions without an index
are listed as not indexed and skipped; a corrupt index is reported in the
header, not fatal. Run `index_sessions` first.

Hit lines carry the matched text and are marked `thinking` when the term
matched only a message's thinking content:

```
ses_f5a4ea5e | 0.5395 | 2026-09-19T10:02:00Z | ASSISTANT (thinking) | contemplating the electric sheep dream
```

CLI: `total-recall --harness opencode do-android-dream-of-electric-sheep
--query 'shadow AND index' [--sessions <id>…] [--hours 48] [--directory <substr>]`.

## LLM-backed tools

### `compact_session`


1. Reads the session (whole, or from the last compaction point)
2. Builds a structured prompt
3. Streams it to Mercury 2.5
4. Returns a structured summary: Accomplished, Current Work, Files, Next Steps, Key Decisions

CLI: `total-recall --harness <h> --session <id> [--full] compact [--provider mistral]`.

### `total_recall`


1. Makes two parallel Mercury 2.5 calls: a current-state summary, and the
   user's goals / tasks / steers
2. Lists recent rollouts within `hours_back`, with the current session marked
   SUMMARISED
3. Lists plan/todo files from `.tmp/delegation/` and `~/.vibe/plans/`
4. Assembles the output in deliberate ordering for autoregressive LLMs:
   metadata first, substance middle, instructions last

CLI: `total-recall --harness <h> --session <id> recall [--provider mistral]`.

## Flood control

Every response that can overflow the caller's context is capped at
`max_bytes` (default 16,384) — the mechanism is generic and applies to any
tool that can produce a large response, not to a fixed list. An overflowing
response is never lost and never floods the caller's context:

- the full response is written to a private file under the user's temp
  directory, `$TMPDIR/total-recall/<tool>_<scope>_<unix-time>.md`, mode 600
- the returned text is cut at a line boundary at or under `max_bytes` —
  JSON responses are never torn: an overflowing JSON response returns the
  marker alone
- the return ends with a line-oriented EOF marker naming the tool, the
  returned and total byte counts, the total line count, the full response's
  path, and the prune policy
- the marker carries the file's line histogram (the vendored
  `line_histogram.awk`), so the shape of what overflowed is visible without
  reading it
- every capped call first removes overflow files older than 24 hours from
  that directory, so the temp file needs no manual cleanup

Raise the window with `max_bytes` when a bigger response is wanted inline.

### Bounded prompts

Flood control bounds what comes *out*. What goes *in* to an LLM is bounded the
same way: every prompt a tool builds from a session is held to a byte budget,
and the newest context is kept while the oldest is dropped. The drop is never
silent — a marker line inside the prompt names how many items and how many
bytes were dropped, and the tool's timing comment carries the prompt byte sizes
and dropped counts. A session too large for one call costs a bounded prompt
rather than a request the vendor cannot ingest in time.

### `line_histogram`

The companion for overflow files and any large dump on disk. Profiles a
file by line-size distribution (histogram mode, ten buckets — a 2 MB file
yields a 2 KB histogram) or extracts a line range (`mode: extract` with
`line`, or `start` and `end`). Runs the vendored `line_histogram.awk` with a
direct `awk -f` spawn — the shebang is never relied on. The line selectors are
validated as a set: `mode: extract` needs `line`, or `start` with `end`;
`end` alone, `start` past `end`, and a selector without `mode: extract` are
rejected by name rather than answered with a histogram nobody asked for.

## Ingestion guardrails

Mercury calls are guarded by measured, documented limits (probed against a
Pay-As-You-Go key on 2026-09-15 with real rollout payloads, 5k/10k/20k tokens,
concurrency ramped 1→64). The free tier carries the same caps — 1,000
requests/min, 1M input tokens/min, 100k output tokens/min — and exceeding any of
them returns HTTP 429.

- **The binding limit is ~1M input tokens/minute.** Requests/min (1,000) and
  output tokens/min (100,000) never bind for compaction workloads.
- **Per-call input cap: 1M tokens** (~4 chars/token). A 100MiB rollout is never
  slung in one call; an over-cap prompt is rejected with an error, never
  truncated. Mercury 2.5's documented context window is 260K tokens, so
  practical calls stay well below the cap — tool results are snipped to 500
  chars in prompt assembly, and a prompt built from a whole session is held to
  the byte budget above, so a session too large to ingest in one call costs a
  bounded prompt rather than a rejected request.
- **Concurrency: 4 in-flight requests** with ~10k-token prompts. Measured
  sweet spot: ~22k input tok/s with zero rejections, p50 latency 1.9s. Beyond
  concurrency 8 the 429 wall arrives with no throughput gain.
- **429 handling**: at most 5 retries, then a descriptive error naming the
  status and the exhausted budget — never a silent drop. `Retry-After` (seconds
  form) is honoured in full and capped at 10s per attempt, so `Retry-After: 0`
  comes back immediately; with no parseable header the wait doubles from 1s
  (1s, 2s, 4s, 8s, capped at 10s). Every 429 is logged via `tracing`.
- **5xx handling**: 3 retries at ~1s spacing, then an error naming the status.
  The two budgets are independent: 429s never consume 5xx retries or vice versa.

## Compaction prompt design

The compaction prompts and strategies are informed by
[badlogic's context compaction research](https://gist.github.com/badlogic/cd2ef65b0697c4dbe2d13fbecb0a0a5f)
comparing Claude Code, Codex CLI, OpenCode, and Amp:

- Claude Code triggers at ~95% context capacity, uses a summary prompt
- Codex CLI preserves recent user messages (last ~20k tokens) alongside the summary
- OpenCode has a separate "prune" mechanism for tool outputs beyond 40k tokens
- Amp uses manual "handoff" instead of auto-compaction

This tool takes the best of each: it preserves recent messages, prunes large
tool outputs, and uses Mercury 2.5 for fast summarization.
