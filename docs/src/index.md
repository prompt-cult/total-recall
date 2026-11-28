# total-recall

Total recall for your agents. Rollouts passed by reference. Session history
mined as long-term memory on every harness you run.

Fast log mining of agent session rollouts, as an MCP server or a CLI. Reads
the session store a coding agent already writes on your machine and turns it
into a queryable long-term memory.

Two surfaces, one binary. `total-recall mcp` is an MCP stdio server, so an
agent calls the tools directly. `total-recall <subcommand>` is a CLI over the
same adapters, for scripting and for reading a store without an agent in the
loop.

## Why

The NOOA paper ([arXiv:2607.20709](https://arxiv.org/abs/2607.20709))
advocates for pass-by-reference over the historic rollouts. We do not
serialize program state into the prompt. The model sees a bounded preview of
each event of the past, be it user text, agent thinking or tool calls. Large
events such as tool call results are linked to. Aggressive caching is used
to speed up queries. Every coding agent already writes the full rollout to
disk. Those harness-specific files are the long-term memory. total-recall
has pluggable backends that read those storage files to provide agents with
tooling to query into months or years of history. The window is for working.
The long-term harness store is for remembering.

Log mining needs no LLM at all. Every profile, extract and search tool works
in a vendor-free build with no API key: see
[Configuration](configuration.md#build-features). Compaction summaries are
opt-in through the fast [Inception Mercury 3](https://www.inceptionlabs.ai),
a diffusion LLM at 1,068 tok/s. Feel free to send a PR to expand the model
set or feature set, to give us agents that have total recall and bring
self-sovereign continuous LLM improvements to the masses!

## Supported harnesses

| Harness | Store | Format |
|---------|-------|--------|
| `vibe` | `~/.vibe/logs/session/` | JSONL / content-addressed JSON chunks |
| `opencode` | `~/.local/share/opencode/opencode.db` | SQLite |

`mistral-vibe` is accepted as a name for the `vibe` harness — the product's
own name for it.

A session store holds the transcripts. The shadow index root is a disposable
folder of full-text indexes this tool builds beside the store, never inside it:

| Harness | Shadow index root |
|---------|-------------------|
| `opencode` | `~/.local/share/opencode/.tantivy/` |
| `vibe` | `~/.vibe/logs/.tantivy/` |

See [`index_sessions` / `do_android_dream_of_electric_sheep`](tools.md#indexing-and-full-text-search).

## What the tools do

| Tool | What it returns |
|------|-----------------|
| `harness` | the harness this server is bound to |
| `help` | the release's `skills/total-recall/SKILL.md` — the tool surface and usage guide, served through the standard capped-text path |
| `list_sessions` | every rollout: id, title, times, counts, byte size, parent/children, aliases, `backend_kind`, `ctime`/`mtime`, `has_tantivy_index` |
| `profile_session` | one rollout: size, line count, role counts, compaction markers, interesting events |
| `extract_messages` | messages since the last compaction point (or the whole rollout) as a bounded JSON envelope |
| `extract_user_messages` | the same envelope over verbatim user messages |
| `extract_by_type` | a raw `type,timestamp,json` recovery dump, one record per line, filtered by type: `user|assistant|tool|thinking` plus the folded derived kinds `todo|compaction|questions` (`"all"` includes everything; a kind the harness cannot serve is skipped with a notice in the meta, never an error) |
| `index_sessions` | builds or refreshes a tantivy full-text index per session |
| `do_android_dream_of_electric_sheep` | full-text search across those indexes, merged by score |
| `summary_cache` | the per-session summary cache blocks: from/to ids and timestamps, message-type counts, size, part file — plus the unsummarised tail |
| `summarise_from` | a structured summary of a session from an explicit unsummarised id: Accomplished, Current Work, Files, Next Steps, Key Decisions; guard: the id must be the first unsummarised one |
| `cache_delete_latest` | roll the summary-cache high watermark back one block (rm ledger row + part file); returns the new cache status |
| `he_said_she_said_they_did` (CLI: `he-said-she-said-they-did`) | one full per-session report: HE SAID (verbatim user text), SHE SAID (assistant text only — no thinking, no tool results), THEY DID (every action incl. MCP/plugin/todo calls, arguments only) with compaction boundaries inline, plus the period covered, the compaction-point ids and the summary-cache blocks/high watermark |
| `prior_period_summary` | orientation summary of a prior period: state summary + user goals + todo log + commits + a recent-rollouts table + plan files + orientation warning |
Full parameter tables and the CLI equivalent of each tool (with the MCP-only
exceptions named): [Tools](tools.md).

## Reading a store is read-only

total-recall opens the store read-only — the OpenCode adapter connects with
`SQLITE_OPEN_READ_ONLY`. It never writes to a session store, and the only files
it creates live in the shadow root beside it.
## Error contract

A payload that cannot be resolved or read is an error, never an empty
session. Every adapter read path returns `Result<T, String>`: the MCP tool
answers with an error result, the CLI prints the reason and exits 2. Damage is
visible instead of a payload silently appearing empty. A payload that exists
and is genuinely empty is not damage and still reads as an empty session. A
JSONL line that fails to parse is skipped with a `WARN` carrying its line
number, so a short read is diagnosable.

`list_sessions` is the one exception by design: a session whose payload cannot
be read is still listed, with counts from whatever could be read, and carries
a `read_error` field so the damage is visible in the index. `index_sessions`
propagates the same error rather than building an empty index.

## Where to go next

- [Installation](installation.md) — release binary via mise, or build from source.
- [MCP setup](mcp-setup.md) — register the server with Mistral Vibe or OpenCode.
- [Configuration](configuration.md) — storage-root overrides, the sandbox guard, API keys, vendor features.
- [Troubleshooting](troubleshooting.md) — the failure modes worth knowing by heart.
- [Operations](operations/opencode-store-vacuum.md) — the opencode store-vacuum runbook.
