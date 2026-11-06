# Tools

Every MCP tool has a CLI equivalent over the same adapter, except where a
section states otherwise — `harness` (the CLI takes `--harness` explicitly)
and `extract_qa_pairs` (MCP only) are the exceptions. Parameter
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
  `extract_user_messages`, `extract_by_type`,
  `prior_period_summary`, `todo_history`, `summary_cache`,
  `summarise_from`, `cache_delete_latest`, `he_said_she_said_they_did`:
  `session_id` (partial match; empty = the
  most recent session). One session is the smallest useful scope, so it is
  the default.
- **window tools** — `list_sessions`,
  `extract_qa_pairs`, `index_sessions`,
  `do_android_dream_of_electric_sheep`: `session_id`
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
| `--harness <name>` | `vibe` \| `opencode`. Required for every subcommand except `mcp` |
| `--session <id>` | partial session id; empty means the most recent session |
| `--json` | pretty JSON, for `list` and `profile` |
| `--markdown` | markdown table for `list`; readable blocks for `extract`, numbered list for `user-messages` |
| `--full` | read the whole rollout instead of from the last compaction point |
| `--verbose` | trace-level logging on stderr |
| `--provider <name>` | `mercury` (default) or `mistral`, for the LLM-backed subcommands only |
| `--limit` / `--offset` / `--max-bytes` / `--max-record-bytes` | extraction bounds; see [Bounded output](#bounded-output) |
| `--include-injected` | include synthetic (injected) user records in `extract-by-type` |
| `--no-cache` | skip the on-disk cache for `profile` |

Output format per subcommand, with no flag given: `list` and `profile` print a
human-readable summary, `extract` prints one JSON object per line,
`user-messages` prints a JSON array, `extract-by-type` always prints
`type,timestamp,json` records, and `todo-history` always prints one JSONL
todo-edit event per line. `do-android-dream-of-electric-sheep` and `index`
print their own report format regardless of flags.

CLI extraction is unbounded by default — stdout is a stream, and 0 means
unbounded. The MCP tools are always bounded. A CLI truncation notice goes to
stderr so stdout stays machine-parseable.

## Read tools

### `harness`

No parameters. Returns the harness this server is bound to as a JSONL record
stream: a bare compact JSON array whose element 0 is the meta record
`{"type":"meta","tool":"harness","harness":"…"}`, with no data records — the
binding is the whole payload. The cheapest confirmation that a registration is
right. No CLI equivalent — the CLI takes `--harness` explicitly.

### `help`

No session parameters. `max_bytes` (default 16,384). Returns the release's
`skills/total-recall/SKILL.md` through the standard capped-text path.
Resolution, in order: the `TOTAL_RECALL_SKILL_PATH` env var if set and
non-empty (the escape hatch); the skills directory —
`~/.config/opencode/skills/total-recall/SKILL.md`, the file OpenCode itself
loads as the skill; a walk up from the server binary (dev builds run from a
repo `target/` tree). A missing skill degrades gracefully: the error names
the skills-directory path and the env var.

### `list_sessions`

MCP: `hours_back` (a window in hours, `1` or more; `0` or omitted = the
240-hour default), `directory` (substring), `all: true` (the whole store),
`max_bytes` (the response budget, default 16,384). A record stream: a bare
compact JSON array whose element 0 is the meta record
`{"type":"meta","tool":"list_sessions","window_count":N,"max_bytes":N}` —
`window_count` is how many rows the window holds, `max_bytes` the budget
that decided the cut — followed by one `type=row` record per listed
session, most recent first:

```json
[{"type":"meta","tool":"list_sessions","window_count":250,"max_bytes":16384},
 {"type":"row","session_id":"ses_…","title":"…","start_time":"…", …}]
```

When the budget cuts the stream, the fitting-prefix array is followed by the
[EOF marker](#flood-control), whose `records_returned` / `records_total`
state the cut: the rows the window holds but the response did not print are
`window_count - records_returned`. The overflow file carries every row, one
per line.

CLI: `total-recall --harness <h> list [--json|--markdown] [--hours N]` —
`--hours 0` (the default) is the unbounded full listing stream; `--hours N`
with `N >= 1` is the scoped listing (at most the 200 most recent rows of the
window, held-back count on stderr).

Each row record: `session_id`, `title`, `start_time`, `end_time`,
`file_size`, `line_count`, `user_count`, `assistant_count`, `tool_count`,
`has_compaction`, `directory`, `parent_session_id`, `child_sessions`,
`child_session_count`, `has_tantivy_index`, `aliases`, `alias_count`,
`notice` and `read_error` when present.

A row whose session has cached summary blocks also carries a
`summary_cache` marker, `{through_id, through_ts}` — the message id and
timestamp the cache covers through, the same field the [summary cache
tools](#the-summary-cache-blocks) work with. The marker is absent when
nothing is cached, so a listing alone tells the caller whether a
`summarise_from` would start from id 1 or from the block edge.

`child_sessions` and `aliases` are bounded per row to the 10 most recent child
ids and the 10 first alias names, with `child_session_count` / `alias_count`
carrying the true count beside them and the row's `notice` naming what it held
back — a store that accumulates subagent children cannot spend the response
budget on one row, so the row count a default call gets is a function of
`max_bytes`; raise `max_bytes` for a wider slice.

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
`max_bytes` holds, cut at a record boundary. A default `list_sessions`
returns rows, never the flood cap's overflow marker; when a single row is
larger than the whole budget the response is the meta record alone followed
by the marker, and the overflow file carries every row as JSONL. The bound
lives where the data lives — the OpenCode
implementation pushes the window, the directory substring and the row cap
into SQL, so
aggregates are computed only for the listed rows (through the store's
`message(session_id, …)` and `part(session_id)` indexes) and the window count
is a bare `COUNT(*)`; the file-based harnesses test mtime and directory
before reading a payload. A scoped listing costs what it asks for, not what
the store holds.

### `profile_session`

CLI: `total-recall --harness <h> --session <id> profile [--no-cache]`.

A record stream: a bare compact JSON array whose element 0 is the meta
record carrying the scalar profile fields, followed by one `type=event`
record per interesting event:

```json
[{"type":"meta","tool":"profile_session","session_id":"ses_…","harness":"…",
  "file_size":613026,"line_count":47,"first_ts":"…","last_ts":"…",
  "role_counts":{"user":1,"assistant":13},"has_tantivy_index":false},
 {"type":"event","line_number":4,"event_type":"UserMessage",
  "summary":"…","gap_lines":4}]
```

An interesting event carries `line_number`, `gap_lines`, `event_type` and
`summary`; `event_type` is one of `Compaction`, `TodoCreate`, `TodoUpdate`,
`TodoDelete`, `GitCommit`, `GitPush`, `GitTag`, `UserMessage`.

#### The profile cache

On by default, staleness-checked; `cache: false` (MCP) or `--no-cache` (CLI)
forces a recompute without touching the cache. The profile is served from a
flat JSON file inside the adapter's shadow root:
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

`extract_messages`, `extract_user_messages` and `extract_by_type` are record
streams like every other record-shaped tool: a bare compact JSON array whose
element 0 is the meta record carrying the paging inputs, followed by one
record per extracted entry. The window (`offset`/`limit`) and the per-record
clamp (`max_record_bytes`) are applied before the stream; the byte fit is
flood control's — over `max_bytes` the fitting-prefix array is followed by the
[EOF marker](#flood-control), whose `records_returned` / `records_total`
state the cut, and the overflow file carries every record in the window, one
per line. The old "even one record exceeds max_bytes" hard error is dead: a
record that cannot fit the response returns via the file, never as an error.

```json
[{"type":"meta","tool":"extract_messages","session_id":"…","harness":"vibe",
  "full":false,
  "bounds":{"total_records":7525,"offset":7425,"limit":100,
            "max_bytes":8388608,"clamped_record_indices":[]}},
 {"type":"message","role":"user","content":"…","injected":false, …}]
```

Omit `offset` for the most recent `limit`; page by passing `offset` (from
`0`) and stepping by the records the response returned — the array's record
count under the cap, the marker's `records_returned` over it. When that
count is short of the window's `limit`, the cut reasons were `record_limit`
(the window itself), `byte_cap` (the budget), or `record_clamp` (a record
was clamped — its index is in `bounds.clamped_record_indices`). An empty
result window — an empty session, or paging past the end — returns the meta
record alone.

### `extract_messages`


CLI: `total-recall --harness <h> --session <id> extract [--full] [--limit N]
[--offset N] [--max-bytes N] [--max-record-bytes N]`.

### `extract_user_messages`

`session_id`, plus `limit`, `offset`, `max_bytes`, `max_record_bytes`. There is
no `full`: user messages are always derived from the whole rollout.
Injected (synthetic) user records are skipped; the stream is identical to
`extract_messages` with the meta record naming
`tool: extract_user_messages`.

CLI: `total-recall --harness <h> --session <id> user-messages`.

### `extract_by_type`

A pure read of the raw store, for recovering data from large, partially
corrupt, or poisoned sessions: a record stream with no summarization or
aggregation. The response is a bare compact JSON array whose element 0 is
the meta record
`{"type":"meta","tool":"extract_by_type","session_id":"…","harness":"…",
"full":bool,"types":[…],"bounds":{…}}` (the same paging inputs as
`extract_messages`), followed by one `type=entry` record per entry:

```json
[{"type":"meta","tool":"extract_by_type","session_id":"…","harness":"vibe",
  "full":true,"types":["user","tool"],
  "bounds":{"total_records":2,"offset":0,"limit":100,"max_bytes":8388608,
            "clamped_record_indices":[]}},
 {"type":"entry","entry_type":"user","ts":"2026-09-15T09:59:55Z",
  "record":{"role":"user","content":"…","injected":false}}]
```

`ts` is ISO8601, a unix epoch, or `0` when neither; `record` is the source
record serialized compactly, clamped by `max_record_bytes` (the clamped
window-relative indices are in `bounds.clamped_record_indices`). Injected
records are skipped by default; the record itself carries the `injected`
flag either way. For vibe the records are the byte-faithful source lines
re-parsed from `messages.jsonl`; other harnesses derive entries from the
normalized message stream. Over `max_bytes` the fitting-prefix array is
followed by the EOF marker; the overflow file carries every entry in the
window, one per line — `rg`/`jq` work it directly:

```bash
jq -c 'select(.entry_type=="user")' <file>.jsonl   # filter by type
jq -c '.record' <file>.jsonl                        # the raw records
sort -t'"' -k4 <file>.jsonl                         # order by timestamp
```

CLI: `total-recall --harness <h> --session <id> extract-by-type --type user
--type thinking [--include-injected]`.

### `extract_qa_pairs`

The question/answer pairs from sessions where the harness asked through a
question tool as a record stream: a bare compact JSON array whose element 0
is the meta record
`{"type":"meta","tool":"extract_qa_pairs","scope":"…","sessions_scanned":N,
"sessions_with_pairs":N,"per_session_cap":100}`, followed by one `type=qa`
record per pair, chronologically:

```json
[{"type":"meta","tool":"extract_qa_pairs","scope":"ses_ee8e8f72",
  "sessions_scanned":1,"sessions_with_pairs":1,"per_session_cap":100},
 {"type":"qa","session_id":"ses_ee8e8f72","harness":"opencode","qa":1,
  "ts":"2026-10-12T08:15:04Z","header":"Branch",
  "question":"Which branch?",
  "options":[{"label":"main","description":"the trunk, release when green"}],
  "answers":["main"]}]
```

For opencode the pairs are read store-side from the `question` tool parts
(`state.input.questions`), the picks under `state.metadata.answers` and the
tool result output as fallback. For vibe, `ask_user_question` arguments'
questions are paired with the following `tool_result` output's answers by
`tool_call_id`. A question that was never answered, or whose prompt was
cancelled, yields no pair. Harnesses without question tools return the meta
record with zero pair records, never an error.

Session selection follows the window tools: `session_id` (partial match)
selects one session; empty works the `hours_back` window (default 48, `0` =
omitted) with an optional `directory` substring, and `all: true` is the
explicit whole-store opt-in. In window mode sessions are ordered
most-recent-first; sessions without pairs contribute no records — the meta
carries the scanned count. Each session contributes at most the most recent
100 pairs, the held-back count stated in the session's last record
(`held_back`). Over `max_bytes` (default 16384, 8 MiB ceiling) the
fitting-prefix array is followed by the EOF marker and the overflow file
carries every pair, one per line. MCP only — no CLI subcommand.

### `todo_history`

Recovers a rollout's todo-list edits: every `todowrite` flush replayed as
the edits between consecutive list states, one compact JSON object per
line, in ts order:

```
{"type":"meta","tool":"todo_history","session_id":"ses_ee8e8f72","harness":"opencode","full":true,"events":3,"max_bytes":16384}
{"type":"todo_edit","ts":"2026-10-08T04:14:18Z","action":"added","todo":"item00: ship the fix","status":"pending"}
{"type":"todo_edit","ts":"2026-10-08T04:20:02Z","action":"in_progress","todo":"item00: ship the fix","status":"in_progress"}
{"type":"todo_edit","ts":"2026-10-08T04:31:47Z","action":"removed","todo":"item01: later"}
```

`action` is `added` (first sight of an item), `updated` (reworded, same
status), the item's new status on a status change (`pending`,
`in_progress`, `completed`, `cancelled`, …), or `removed` (the flush
dropped it — `status` is omitted on a removal). Item identity is the
`itemNN:` slug prefix this repo's sessions carry, else the full content
string. Folding the events reproduces the flushes. `full` defaults to true
— the whole rollout's todo history; `false` reads from the last compaction
point. On the MCP stream every record carries a `type` discriminator: the
array opens with the meta record
`{"type":"meta","tool":"todo_history","session_id":"…","harness":"…","full":bool,"events":N,"max_bytes":N}`,
followed by one `{"type":"todo_edit", …}` record per edit. Over `max_bytes`
(default 16384, 8 MiB ceiling) the
fitting-prefix array continues into the EOF marker naming the overflow
`.jsonl` file, which holds ALL records as pure JSONL — every line parses
with `jq`. Under the cap nothing is retained. The CLI streams the bare
JSONL events, unbounded. Opencode only: every other harness refuses with
an explicit error, never an empty list.

CLI: `total-recall --harness opencode --session <id> todo-history [--no-full]`.

### `he_said_she_said_they_did`

One full, per-session report answering "what did we say and do" — the
surface for "why/how/when/else" and follow-ups. Read-only: no term
matching, no LLM call, nothing written. `session_id` (partial match; empty
= the most recent session) selects the session; `max_bytes` (default
16,384, 8 MiB ceiling) flood-caps the report with the EOF marker and
overflow file.

The report has three spoken sections plus a context section, all derived
from the session as a whole:

- **HE SAID** — the user's messages, verbatim. Injected (synthetic) user
  parts are skipped, as they are in `extract_user_messages`.
- **SHE SAID** — the assistant's text parts only. Explicitly NOT model
  thinking/reasoning and explicitly NOT tool results.
- **THEY DID** — every action: built-in tool calls, MCP tool calls, plugin
  actions, todo actions — anything that arrived as a tool-like call. Each
  action is one line of call name + arguments; tool results are excluded.
  Compaction boundaries appear inline in the stream at the point they
  occurred, so the reader knows the dialogue was itself cut down.
- **Context** — the period the report covers (first to last message
  timestamp), where the compaction points sit (1-based message ids), and
  where the existing summaries are: total-recall's own summary-cache
  blocks (their id ranges) and the high watermark, i.e. the first message
  id a `summarise_from` can start at.

Example (shape; content is whatever the session holds):

```
# he_said_she_said_they_did — ses_alpha0001 (opencode)
PERIOD: 2026-10-26T09:04:00Z .. 2026-10-26T09:17:00Z (14 messages)

## HE SAID
1. [2026-10-26T09:04:00Z] Please run git status for me

## SHE SAID
2. [2026-10-26T09:05:00Z] Here is the current branch state…

## THEY DID
3. [2026-10-26T09:05:30Z] bash: git status
4. [2026-10-26T09:06:00Z] todowrite(...)
--- COMPACTION BOUNDARY (message 5 of 14) [2026-10-26T09:08:00Z]

## CONTEXT
- period covers all 14 messages: nothing has been compacted away within the store itself
- compaction points: 1, at message 5
- summaries: no summary-cache blocks — summarise_from can start at id 1
```

MCP only — no CLI subcommand.

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

Each session gets `<shadow root>/<session_id>/`, plus a
`total-recall-meta.json` marker carrying `session_id`, `doc_count` and
`built_at`. Re-indexing a session replaces its index from scratch — the
folder is disposable, can be deleted at any time, and MUST NOT be committed.

MCP: the response is a JSONL record stream — a bare compact JSON array whose
element 0 is the meta record
`{"type":"meta","tool":"index_sessions","scope":"…","selected":N}` (`scope` is
the requested session id, or `"all"` when unscoped), followed by one
`{"type":"indexed","session_id":"…","doc_count":N}` record per indexed
session. Unmatched session ids and a failed index return a tool error; they
never enter the stream. `hours_back` defaults to 24 — the natural call indexes
the last day, not the whole store; `all: true` is the explicit whole-store
opt-in.

CLI: `total-recall --harness <h> index [--sessions <id>…] [--hours 24]
[--directory <substr>]`. `list` and `profile` carry the index presence flag
alongside their usual output.

### `do_android_dream_of_electric_sheep`


Searches the per-session indexes with a `QueryParser` over the `content` and
`thinking` fields, merging the top 10 hits per session into one ranking
ordered by score, returned as a record stream: a bare compact JSON array
whose element 0 is the meta record
`{"type":"meta","tool":"do_android_dream_of_electric_sheep","query":"…",
"scope":"…","sessions_searched":N,"sessions_with_hits":N,
"not_indexed":["…"],"unmatched":["…"],"corrupt":["…"]}`, followed by one
`type=hit` record per hit, relevance-ordered:

```json
[{"type":"meta","tool":"do_android_dream_of_electric_sheep","query":"alpha",
  "scope":"window","sessions_searched":1,"sessions_with_hits":1,
  "not_indexed":[],"unmatched":[],"corrupt":[]},
 {"type":"hit","session_id":"ses_f5a4ea5e","score":0.5395,
  "ts":"2026-09-19T10:02:00Z","role":"assistant","thinking":true,
  "fragment":"contemplating the electric sheep dream"}]
```

The session_id resolves to the most recent match; an unmatched id is
reported in the meta record's `unmatched`, not fatal. Sessions without an
index are listed in `not_indexed` and skipped (run `index_sessions` first);
a corrupt index is reported in `corrupt`, not fatal. A hit carries
`thinking: true` when the term matched only a message's thinking content.

CLI: `total-recall --harness opencode do-android-dream-of-electric-sheep
--query 'shadow AND index' [--sessions <id>…] [--hours 48] [--directory <substr>]`.

## The summary cache (blocks)

The summary cache makes what is cached an explicit, inspectable data
structure: a per-session **range ledger** of blocks, each naming the exact
message-id range it summarises. The ledger is
`<shadow root>/tr_<session-id>_summary.csv`, one row per summarised part,
11 columns:

```
from_id,from_ts,to_id,to_ts,user,assistant,tool,thinking,chars,tokens_est,part_file
```

The ids are 1-based message sequence numbers in the session stream — stable
for append-only stores, computed by enumerating the messages at read time,
so they work on every harness. The type counts, `chars` and `tokens_est`
describe the block's message slice; the part text lives in
`tr_<session-id>_summary_parts/`. There is no backward compatibility with
the v0.15.0 3-column ledger (alpha): a malformed or short row invalidates
the ledger — it is treated as empty and the file is dropped, and any
orphaned part files may be clobbered later.

### `summary_cache`

Read-only introspection as a record stream: resolve the session, read the
ledger, and return a bare compact JSON array whose element 0 is the meta
record — carrying the session and the unsummarised tail — and each later
element one cached block:

```json
[{"type":"meta","tool":"summary_cache","session_id":"ses_…",
  "tail":{"from_id":5,"from_ts":"…","count":2,"user":1,"assistant":1,
          "tool":0,"thinking":0,"chars":400,"tokens_est":100}},
 {"type":"block","from_id":1,"from_ts":"…","to_id":4,"to_ts":"…","user":2,
  "assistant":2,"tool":0,"thinking":0,"chars":1200,"tokens_est":300,
  "part_file":"…/part_001_004.md"}]
```

`tail` is the first unsummarised id (the last block's `to_id + 1`, or 1 with
no blocks) plus the count, type counts, chars and token estimate over the
messages from there to the session's end — what a summarise would cost.
`tail` is `null` when every message is already covered. A session with no
blocks returns the meta record with `tail` and zero block records — never an
empty unexplained array. The tool never calls the LLM and never writes.

CLI: `total-recall --harness <h> --session <id> summary-cache`.

### `summarise_from`

Summarise from an explicit id: `session_id` (partial match) and `from_id`.
The guard: `from_id` must be the **first unsummarised id** — one past the
last cached block's `to_id`, or `1` with no blocks. Anything else is an
error in the house form naming the expected id, so a caller that has not
seen `summary_cache` cannot guess. On success the messages from `from_id`
to the session's end are summarised, stored as a new block (part file plus
ledger row), and the tool returns the composed parts — each named
`### part N: from_ts → to_ts (ids from_id..to_id)` — plus the
`summary cache:` footer with the new block counted in.

Rollback is explicit too: `cache_delete_latest` deletes the newest block
(ledger row and part file) and returns the new cache status; call it twice
to roll back two blocks, then `summarise_from` redoes the period whole.

```bash
total-recall --harness <h> --session <id> summarise-from --from-id 5 [--provider mistral]
```

### `cache_delete_latest`

Roll the cache back one block: the newest ledger row is removed and its
part file deleted, then the rolled-back cache status is returned as the
same record stream as `summary_cache` — a bare compact JSON array whose
element 0 is the meta record `{"type":"meta","tool":"cache_delete_latest",
…}` followed by one `type=block` record per surviving block. Call it
repeatedly to roll back several blocks. With no blocks it is a house-form
error, not a no-op — an error never enters the record stream.

```bash
total-recall --harness <h> --session <id> cache-delete-latest
```

## LLM-backed tools

### `prior_period_summary`

An orientation summary of a prior period of work. The period is the work
**since the session's last compaction boundary** — the work after that
boundary. A session with no compaction points has no earlier period, so the
summary covers the whole session up to now.

The output carries, in deliberate ordering for autoregressive LLMs (metadata
first, ground truth next, LLM summaries middle, action last):

1. Recent rollouts within `hours_back`, with the current session marked
   SUMMARISED
2. Plan/todo files from `.tmp/delegation/` and `~/.vibe/plans/`
3. The **todo log** of the period (opencode): items opened and closed with
   timings, the state-change history (opened → in_progress →
   completed/cancelled transitions), and a **churn note** when items open
   faster than they close — starting new things without finishing old ones
   may be causing churn — with the human's recent steers read into it. On a
   harness without readable todowrite flushes the section says so instead of
   failing or silently reading as "no todos".
4. The **commits made in the period**: the git commit tool calls with their
   timestamps.
5. The current-state summary and the user's goals / tasks / steers (two
   parallel Mercury 3 calls).
6. A **compaction-points FYI footer**: the timestamps of the last few
   compaction points with the message and line counts since each — FYI only;
   mine the logs for anything deeper.
7. The instructions, then a fixed **orientation warning** appended to every
   output: summarising in-flight work in more detail is a fool's errand and
   a moral hazard — the model would trust the summary and not double-check —
   so the warning makes re-orientation the model's job.

CLI: `total-recall --harness <h> --session <id> prior-period-summary [--provider mistral]`.

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
- **Per-call input cap: 450,000 tokens — 90% of Mercury 3's 500K context
  window** (half of the model's 1M window, deliberately, so summarisation
  keeps headroom and sends more of the session intact rather than pruning
  harder; ~4 chars/token), leaving the rest for the completion. A 100MiB
  rollout is never slung in one call; an over-cap prompt is rejected with an
  error, never truncated. Practical calls stay far below the cap — tool
  results are snipped to 500 chars in prompt assembly, and a prompt built
  from a whole session is held to the byte budget above, so a session too
  large to ingest in one call costs a bounded prompt rather than a rejected
  request.
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
[badlogic's context compaction research](https://gist.github.com/badlogic/cd2ef65b0697c4dbe2d13fbecb0a0a5f):

- OpenCode has a separate "prune" mechanism for tool outputs beyond 40k tokens
- Amp uses manual "handoff" instead of auto-compaction

This tool takes the best of each: it preserves recent messages, prunes large
tool outputs, and uses Mercury 3 for fast summarization.
