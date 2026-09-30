# Troubleshooting

## The MCP server refuses to start, exit 2

The harness is unset or unknown. Each server is bound to exactly one harness
by `HARNESS` in the registration snippet's environment block, or `--harness
<name>` on the `mcp` subcommand:

```
refusing to start MCP server: harness not defined. Set HARNESS=<vibe|codex|claude|opencode>
in the MCP config environment or pass --harness <name>.
```

```
refusing to start MCP server: unknown harness 'vibee'. Valid values:
vibe|codex|claude|opencode. Set HARNESS=<vibe|codex|claude|opencode> in the MCP
config environment or pass --harness <name>.
```

The server prints the reason to stdout and stderr and exits 2, because a
server that starts with an unknown harness would bind every tool to a
nonsense store.

## 401 "Incorrect API key" from `compact_session` / `total_recall`

The MCP server's environment block is missing `INCEPTION_API_KEY`. List,
profile, extract, index and search all work without a key — no LLM call — and
the two LLM-backed tools fail with the vendor's 401.

Add the key to the snippet's environment block, per
[MCP setup](mcp-setup.md#the-environment-block), or export it in the shell
that launches the client so the server inherits it. A `.env` file is not a
reliable route for a spawned server: it is only found relative to a working
directory the MCP client chooses.

The same 401 on a CLI invocation means the key is neither exported nor in a
`.env` in the current directory.

## "LLM provider `mercury` is not compiled into this build"

The binary was built with `--no-default-features`, or without that vendor's
feature. Either rebuild:

```bash
cargo build --release --features mercury
```

or keep the vendor-free build and use the log-mining tools, which need no key.
The MCP tool name stays in `tools/list` either way — clients bind by name, so
a missing vendor is reported on the call, not by a disappearing tool. See
[Configuration](configuration.md#build-features).

## `unknown LLM provider '<name>'`

The name is not a vendor. Valid values are `mercury` and `mistral`; an
unrecognised name is refused rather than silently falling back to Mercury.

## The sandbox guard refuses to build an adapter

```
TOTAL_RECALL_SANDBOX is set but TOTAL_RECALL_OPENCODE_ROOT is not: refusing to
read the live store. Point TOTAL_RECALL_OPENCODE_ROOT at a fixture or scratch
copy.
```

`TOTAL_RECALL_SANDBOX=1` is armed and this harness has no root override. Set
the harness's override — see
[Configuration](configuration.md#storage-root-overrides) — or unset the
sandbox. The guard never falls back to the live store.

## An override resolves to nothing usable

An override that does not exist, or is empty, is an error naming the path — not
a silent fall-back to `$HOME`. Check for a typo in the variable name and in the
path, and remember an empty value counts as unset.

## "No sessions found"

The store resolved to an empty or non-existent directory. Check, in order: the
`HARNESS` value, then the store root the tool resolved for it (see
[Configuration](configuration.md#storage-root-overrides)), then that the
harness has actually written sessions there.

## The CLI exits 2 with a path in the message

A rollout payload could not be resolved or read. Read paths return
`Result<T, String>`: unreadable data is an error, never an empty session. The
usual causes are permissions on the store, or the session id not resolving.
A JSONL line that fails to parse is not fatal — it is skipped with a `WARN`
carrying its line number, visible under `--verbose`.

`list_sessions` is the deliberate exception: a session whose payload cannot be
read is still listed, with counts from whatever could be read, and carries a
`read_error` field.

## A tool reports "No sessions found" but the store has sessions

A partial `session_id` that matches nothing resolves to no session, and the
tools answer with that error rather than guessing. Confirm the id against
`list_sessions`; partial ids are matched by substring, and the most recent
match wins.

## `do_android_dream_of_electric_sheep` reports "not indexed"

The session has no shadow index yet. Run `index_sessions` (or
`total-recall --harness <h> index`) first — the search reports unindexed
sessions in its header and skips them, rather than returning nothing as if the
terms were absent. A corrupt index is reported in the header, also not fatal.

## `she_said_he_said_action` fails on a non-OpenCode harness

The tool is implemented for OpenCode only; other harnesses return a clear
unsupported error. Matching runs inside SQLite as a custom scalar function on
a read-only connection, which is an OpenCode-store property.

## A tool name looks wrong in the agent's surface

The MCP server key was renamed. The tool names derive from it —
`total-recall_list_sessions`, … — so keep the key as `total-recall` in both
registration snippets.

## Extraction came back truncated

Expected: the MCP tools are bounded by design. Read `bounds.truncation_reason`
and `bounds.notice` — `record_limit`, `byte_cap` or `record_clamp` — and page
with `offset` from `0`, following `bounds.next_offset`. See
[Tools](tools.md#bounded-output).

CLI extraction is unbounded unless you pass `--limit` / `--max-bytes`; when you
do, the truncation notice goes to stderr so stdout stays machine-parseable.

## "even one record ... exceeds max_bytes"

One record is larger than the whole budget. Raise `max_bytes`, or set
`max_record_bytes` to enable the per-record clamp. The tool errors rather than
silently breaking the cap.

## The store is huge and `list` is slow

On the OpenCode SQLite store the index is a single `GROUP BY` query, so this is
a disk-read question, not a CPU one. A store that grew without bound is the
subject of the [store-vacuum runbook](operations/opencode-store-vacuum.md).
