---
name: total-recall
description: Total-recall MCP tool for fast compaction and log mining of session rollouts as a long-term memory store. Use this skill to compact sessions with Mercury 2.5, list/profile/extract rollouts across harnesses (opencode, vibe, claude, codex), mine dialogue with she_said_he_said_action, and full-text search sessions with the tantivy-backed do_android_dream_of_electric_sheep action.
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
| `compaction_compact_session` | `compact` | Mercury 2.5 structured summary |
| `compaction_total_recall` | `recall` | state summary + user goals + recent rollouts + plan files |
| `compaction_profile_session` | `profile` | file size, line count, role counts, interesting events |
| `compaction_extract_user_messages` | `user-messages` | verbatim user messages |
| `compaction_extract_messages` | `extract` | all messages as JSON |
| `she_said_he_said_action` | `he-said-she-said` | exact-term dialogue/tool mining |
| `do_android_dream_of_electric_sheep` | `do-android-dream-of-electric-sheep` | **tantivy full-text search** — ranked, phrase-capable, covers assistant thinking |
| `index_sessions` | `index` | build/refresh the per-session tantivy shadow indexes |

## The dream pass (tantivy search)

`do_android_dream_of_electric_sheep` is the ranked full-text search over
rollout content. It indexes message content **and assistant
thinking/reasoning**, so it surfaces what the agent was reasoning about —
exactly the material word-list tools cannot reach. It complements
`she_said_he_said_action`, which only finds exact terms.

Indexes are per session, in a shadow folder adjacent to the store:
`<store>/.tantivy/<session-id>/`. They are disposable and never committed.

### Workflow

1. `list_sessions` reports `has_tantivy_index` per session — true means
   searchable, false means run the index first.
2. Build or refresh indexes:

```bash
total-recall --harness opencode index --hours 48
```

MCP: `index_sessions` with `hours_back` (0 = no bound) and optional
`directory` substring filter and explicit `sessions` list of partial ids.

3. Search:

```bash
total-recall --harness opencode do-android-dream-of-electric-sheep \
  --query "tantivy index" --hours 48
```

MCP: `do_android_dream_of_electric_sheep` with `query` (tantivy query
syntax: terms, `OR`, phrase in quotes), `sessions` (partial ids, empty =
all within `hours_back`, default 48, 0 = no bound), optional `directory`.

The report lists hits ordered by relevance score: session id, score,
timestamp, role (marked `ROLE (thinking)` when the match is in reasoning),
and a fragment around the match. Sessions without an index are reported in
the header as "not indexed (run index first)".

## Term mining (exact match)

```bash
total-recall --harness opencode he-said-she-said --words "mcp,tantivy" --hours 48
```

MCP: `she_said_he_said_action` with `words` (required, at least one),
optional `sessions`, `hours_back` (default 48, 0 = no bound), `directory`.
Returns HE SAID (user), SHE SAID (assistant), THEY DID (tool calls) per
session, most recent first.
