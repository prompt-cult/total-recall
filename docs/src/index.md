# total-recall

Fast compaction and log mining of agent session rollouts, as an MCP server
and a CLI. Reads the session store a CLI coding agent already writes on your
machine, and turns it into a queryable long-term memory: compaction summaries
through [Inception Mercury 2.5](https://www.inceptionlabs.ai), a diffusion LLM
running at 1,100 tokens/sec.

Two surfaces, one binary. `total-recall mcp` is an MCP stdio server, so an
agent calls the tools directly. `total-recall <subcommand>` is a CLI over the
same adapters, for scripting and for reading a store without an agent in the
loop.

## Why

Every CLI coding agent (Claude Code, Codex CLI, OpenCode, Mistral Vibe) hits
context limits on long sessions. Their built-in compaction is slow and uses
expensive frontier models. Mercury 2.5 does the same job at 1,100 tok/s for
$0.04/M input and $0.15/M output (launch pricing). Augment Code
[moved compaction to Mercury and cut latency 82%](https://www.inceptionlabs.ai/blog/introducing-mercury-2-5)
(150s to 27s) and costs 90%.

Log mining needs no LLM at all. Every index, profile, extract and search tool
works in a vendor-free build with no API key: see
[Configuration](configuration.md#build-features).

## Supported harnesses

| Harness | Store | Format |
|---------|-------|--------|
| `vibe` | `~/.vibe/logs/session/` | JSONL |
| `codex` | `~/.codex/sessions/` | JSONL + SQLite |
| `claude` | `~/.claude/projects/` | JSONL |
| `opencode` | `~/.local/share/opencode/opencode.db` | SQLite |

A session store holds the transcripts. The shadow index root is a disposable
folder of full-text indexes this tool builds beside the store, never inside it:

| Harness | Shadow index root |
|---------|-------------------|
| `opencode` | `~/.local/share/opencode/.tantivy/` |
| `vibe` | `~/.vibe/logs/.tantivy/` |
| `codex` | `~/.codex/.tantivy/` |
| `claude` | `~/.claude/.tantivy/` |

See [`index_sessions` / `do_android_dream_of_electric_sheep`](tools.md#indexing-and-full-text-search).

## What the tools do

| Tool | What it returns |
|------|-----------------|
| `harness` | the harness this server is bound to |
| `list_sessions` | every rollout: id, title, times, counts, byte size, parent/children, aliases, `has_tantivy_index` |
| `profile_session` | one rollout: size, line count, role counts, compaction markers, interesting events |
| `extract_messages` | messages since the last compaction point (or the whole rollout) as a bounded JSON envelope |
| `extract_user_messages` | the same envelope over verbatim user messages |
| `extract_by_type` | a raw `type,timestamp,json` recovery dump, one record per line |
| `she_said_he_said_action` | per session: HE SAID (user text), SHE SAID (assistant text), THEY DID (tool calls) matching your terms |
| `index_sessions` | builds or refreshes a tantivy full-text index per session |
| `do_android_dream_of_electric_sheep` | full-text search across those indexes, merged by score |
| `compact_session` | a structured summary of a session: Accomplished, Current Work, Files, Next Steps, Key Decisions |
| `total_recall` | state summary + user goals/tasks/steers + a recent-rollouts table + plan files |

Full parameter tables and the CLI equivalent of each tool: [Tools](tools.md).

## Reading a store is read-only

total-recall opens the store read-only — the OpenCode adapter connects with
`SQLITE_OPEN_READ_ONLY`. It never writes to a session store, and the only files
it creates live in the shadow root beside it.
`she_said_he_said_action` pushes its matching into the query engine as a custom
scalar function, so the rows stream out in one pass without the database ever
being written.

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
