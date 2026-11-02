---
name: total-recall
description: Total-recall MCP tool for fast compaction and log mining of session rollouts as a long-term memory store. Use this skill to compact sessions with Mercury 3, list/profile/extract rollouts across harnesses (opencode, vibe, claude, codex), and full-text search sessions with the tantivy-backed do_android_dream_of_electric_sheep action.
---

# Total Recall

The `total-recall` binary (0.5.0+) is an MCP tool and CLI for mining agent
session rollouts. Installed at `~/.local/bin/total-recall`. Run as an MCP
server with `total-recall mcp --harness <name>` or used directly as a CLI.

Harnesses: `opencode` (SQLite at `~/.local/share/opencode/opencode.db`),
`vibe`, `claude`, `codex` (JSONL stores).

## The actions

| MCP tool | CLI | What it does |
|---|---|---|
| `total-recall_summary_cache` | `summary-cache` | read-only introspection of a session's summary cache: the cached blocks (id ranges, type counts, chars, token estimates) plus the unsummarised tail — what a summarise would cost. Never calls the LLM |
| `total-recall_summarise_from` | `summarise-from` | Mercury 3 structured summary from an explicit 1-based message id; the guard requires the first unsummarised id (one past the last cached block's `to_id`, or 1) and errors naming it otherwise; returns the composed parts + cache footer |
| `total-recall_cache_delete_latest` | `cache-delete-latest` | roll the summary cache back one block (newest ledger row + part file deleted) and return the new cache status; no blocks = error |
| `total-recall_prior_period_summary` | `prior-period-summary` | orientation summary of a prior period: state summary + user goals + todo log + commits + recent rollouts + plan files + orientation warning; the period is the work since the session's last compaction boundary |
| `total-recall_profile_session` | `profile` | file size, line count, role counts, interesting events (cached by default; `--no-cache` to recompute) |
| `total-recall_extract_user_messages` | `user-messages` | verbatim user messages |
| `total-recall_extract_messages` | `extract` | all messages as JSON |
| `do_android_dream_of_electric_sheep` | `do-android-dream-of-electric-sheep` | **tantivy full-text search** — ranked, phrase-capable, covers assistant thinking |
| `he_said_she_said_they_did` | — | one full per-session report: HE SAID (verbatim user text), SHE SAID (assistant text only), THEY DID (every action incl. MCP/plugin/todo calls) with compaction boundaries inline, period covered, compaction points and summary-cache blocks |
| `index_sessions` | `index` | build/refresh the per-session tantivy shadow indexes |
| `prior_period_summary` | `prior-period-summary` | orientation summary of a prior period |
| `harness` | — | report which harness this server is bound to |
| `list_sessions` | `list` | list session rollouts for the bound harness |
| `extract_by_type` | `extract-by-type` | raw extract of entries filtered by type |
| `extract_qa_pairs` | `extract-qa-pairs` | question/answer pairs from sessions |
| `todo_history` | `todo-history` | todo-list edits as JSONL event stream |
| `line_histogram` | `line-histogram` | line-size distribution or extract lines |
| `help` | — | return this skill text |

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

The report lists hits ordered by relevance score: session id, score,
timestamp, role (marked `ROLE (thinking)` when the match is in reasoning),
and a fragment around the match. Sessions without an index are reported in
the header as "not indexed (run index first)".

