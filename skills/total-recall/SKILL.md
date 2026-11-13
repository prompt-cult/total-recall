---
name: total-recall
description: Total-recall MCP tool for fast compaction and log mining of session rollouts as a long-term memory store. Use this skill to compact sessions with Mercury 3, list/profile/extract rollouts across harnesses (opencode, vibe), and full-text search sessions with the tantivy-backed do_android_dream_of_electric_sheep action.
---

# Total Recall

The `total-recall` binary (0.5.0+) is an MCP tool and CLI for mining agent
session rollouts. Installed at `~/.local/bin/total-recall`. Run as an MCP
server with `total-recall mcp --harness <name>` or used directly as a CLI.

Harnesses: `opencode` (SQLite at `~/.local/share/opencode/opencode.db`),
`vibe` (JSONL store at `~/.vibe/logs/session/`; also named
`mistral-vibe`), `codex` (date-hierarchical rollout tree at
`~/.codex/sessions/<YYYY>/<MM>/<DD>/`).

## Multi-harness installs

The tool ships two ways:

- **CLI on PATH** (`~/.local/bin/total-recall`) — always the latest release,
  used ad hoc. It takes `--harness <name>` (or `HARNESS`), so it can read any
  harness's store on demand, including a harness you are not sitting in. This
  is the escape hatch for a harness whose own MCP is pinned.
- **MCP per harness** — pinned. Copy the binary the harness's MCP currently
  runs into the harness's own config location, named with its version
  (`total-recall-<version>` beside `config.toml`, say), and point the MCP
  registration at the copy. A global refresh then moves every other harness,
  not this one: the pin only changes when you consciously bump it — recopy the
  new version, repoint, test that harness.

If you run across several harnesses (OpenCode, Vibe/Mistral, Pi, Codex), do
not let them all share one central binary: a release that breaks one harness
breaks all of them at once, untested. The CLI stays latest for ad-hoc use;
each harness's MCP rides its own pinned copy. To tell whether a pinned harness
is behind, run the latest CLI from inside that harness and compare `--version`.

## The actions

| MCP tool | CLI | What it does |
|---|---|---|
| `total-recall_chat_with_mercury` | `chat` | THE one-call key check: chat with Mercury 3, get the response, the speed and the tokens (`elapsed_s`, the token counts, `finish_reason`) as `type=reply` records; missing or empty key fails at the front door naming the setup places |
| `total-recall_summary_cache` | `summary-cache` | read-only introspection of a session's summary cache as a record stream: meta carries the unsummarised tail, then one `type=block` record per cached block. Never calls the LLM |
| `total-recall_summarise_from` | `summarise-from` | Mercury 3 structured summary from an explicit 1-based message id; the guard requires the first unsummarised id (one past the last cached block's `to_id`, or 1) and errors naming it otherwise; returns `type=section` records per composed part + the cache footer |
| `total-recall_cache_delete_latest` | `cache-delete-latest` | roll the summary cache back one block (newest ledger row + part file deleted) and return the same record stream as summary_cache; no blocks = error |
| `total-recall_prior_period_summary` | `prior-period-summary` | orientation summary of a prior period as `type=section` records (recent_rollouts, plan_files, todo_log, commits, state_summary, goals_summary, compaction_footer, instructions, orientation_warning); the meta carries the timing map; the period is the work since the session's last compaction boundary |
| `total-recall_profile_session` | `profile` | record stream: meta carries file size, line count, role counts, index flag; `type=event` records per interesting event (cached by default; `--no-cache` to recompute) |
| `total-recall_extract_user_messages` | `user-messages` | record stream: meta carries the paging inputs, `type=message` records verbatim, injected skipped |
| `total-recall_extract_messages` | `extract` | all messages as a record stream (meta + `type=message` records) |
| `do_android_dream_of_electric_sheep` | `do-android-dream-of-electric-sheep` | **tantivy full-text search** — meta + `type=hit` records ranked by score, phrase-capable, covers assistant thinking |
| `he_said_she_said_they_did` | — | one full per-session report as records: `he_said`/`she_said`/`they_did` + inline `compaction_boundary`; the meta carries the period, compaction points and summary-cache blocks |
| `index_sessions` | `index` | build/refresh the per-session tantivy shadow indexes; `type=indexed` records per session |
| `prior_period_summary` | `prior-period-summary` | orientation summary of a prior period |
| `harness` | — | report which harness this server is bound to |
| `list_sessions` | `list` | record stream: meta carries `window_count`, `type=row` records most-recent-first |
| `extract_by_type` | `extract-by-type` | raw extract as `type=entry` records (entry_type/ts/record) filtered by type |
| `extract_qa_pairs` | `extract-qa-pairs` | question/answer pairs as `type=qa` records |
| `todo_history` | `todo-history` | todo-list edits as `type=todo_edit` records |
| `line_histogram` | `line-histogram` | line-size distribution or extract lines (the text path) |
| `help` | — | short orientation — the CLI's generated help page (clap) plus the house line: intended to be run as MCP in a harness, every CLI action is available over the MCP interface, MCP is the preferred mode; points to `skill` for the full guide (the text path, no parameters) |
| `skill` | `skill` | the full tool surface and usage guide — this file, through the text flood-control path (capped at `max_bytes`, cut at a line boundary, overflow to a text file named by the EOF marker with its line histogram) |

## The dream pass (tantivy search)

`do_android_dream_of_electric_sheep` is the ranked full-text search over
rollout content. It indexes message content **and assistant
thinking/reasoning**, so it surfaces what the agent was reasoning about.

`he_said_she_said_they_did` complements it as the reading pass: one full
per-session report of HE SAID (verbatim user text), SHE SAID (assistant text
only — no thinking, no tool results), and THEY DID (every action incl.
MCP/plugin/todo calls, arguments only), with compaction boundaries inline,
the period covered, the compaction-point ids and the summary-cache blocks.

Indexes are per session, in a shadow folder adjacent to the store:
`<store>/.tantivy/<session-id>/`. They are disposable and never committed.

### Workflow

1. `list_sessions` reports `has_tantivy_index` per session — true means
   searchable, false means run the index first. Rows also carry a
   `summary_cache` marker (`{through_id, through_ts}`) when the session has
   cached summary blocks.
2. Build or refresh indexes:

```bash
total-recall --harness opencode index --hours 48
```

MCP: `index_sessions` with `hours_back` (a window in hours, 1 or more;
omitted or 0 = the 24-hour default; `all: true` is the whole store) and
optional `directory` substring filter and explicit `sessions` list of partial
ids.

3. Search:

```bash
total-recall --harness opencode do-android-dream-of-electric-sheep \
  --query "tantivy index" --hours 48
```

MCP: `do_android_dream_of_electric_sheep` with `query` (tantivy query
syntax: terms, `OR`, phrase in quotes), `sessions` (partial ids, empty =
all within `hours_back`, default 48, omitted or 0 = that default), optional
`directory`.

Hits return as `type=hit` records ordered by relevance score — session id,
score, ts, role, `thinking: true` when the match is in reasoning, and a
fragment around the match. Sessions without an index are named in the meta
record's `not_indexed` (run `index_sessions` first).

## JSONL overflow files

Every record-shaped tool returns a **bare compact JSON array**: element 0 is
the meta record `{"type":"meta","tool":"…",…}`, every later element a data
record with a `type` discriminator. When a response would exceed
`max_bytes`, the array is cut at a record boundary and the fitting prefix is
followed by an `--- [EOF-TRUNCATED] ---` marker naming the overflow file —
a **pure JSONL** file (one compact record per line, `\n`-terminated, no
commas, meta first) that carries ALL records, mode 600 under
`$TMPDIR/total-recall/`, pruned after 24 hours. The file is the point:
`rg` finds, `jq` extracts, `line_histogram` profiles.

Recipes:

```bash
# strip the marker from a capped response — the array prefix alone:
sed '/^--- \[EOF-TRUNCATED\]/,$d' response.txt

# find records: rg over the JSONL (every line is one JSON object):
rg 'pattern' "$TMPDIR"/total-recall/list_sessions_*.jsonl

# extract fields (every record carries type; meta is record 0):
jq -c 'select(.type=="row") | {session_id, title}' file.jsonl
jq -c 'select(.type=="hit")  | {session_id, score, fragment}' file.jsonl
jq -c 'select(.type=="meta")' file.jsonl   # the meta record alone

# profile before prompting: the chonky record is visible without reading it:
total-recall line-histogram file.jsonl
```

Sweep the recent overflow files (the fdfind one-liner):

```bash
fdfind -H -t f -e jsonl --changed-within 1d . "$TMPDIR/total-recall" \
  | while read -r f; do echo "=== $f ==="; total-recall line-histogram "$f"; done
```

## The key-check recipe

Before blaming a missing key for an LLM-backed tool's failure, make the
one-call check:

```bash
total-recall chat "hello"
```

Or the MCP form, `chat_with_mercury` with `{"message": "hello"}`. The reply
record carries the response, `elapsed_s`, the token counts and
`finish_reason` — key, response, speed and tokens in one call. Missing or
empty key: the error names the three setup places (the MCP environment
block, `export INCEPTION_API_KEY=…` in the shell, a `.env` at or above the
working directory) and no request is attempted.

References: [Line histogram — the file profiler you didn't know you
needed](https://dev.to/simbo1905/line-histogram-the-file-profiler-you-didnt-know-you-needed-38oi)
and [the gist](https://gist.github.com/simbo1905/0454936144ee8dbc55bdc96ef532555e).

