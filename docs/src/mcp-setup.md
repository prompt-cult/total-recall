# MCP setup

The binary runs as an MCP stdio server:

```bash
total-recall mcp
```

## One harness per server instance

Each server is bound to exactly ONE harness, set by the installing config via
the `HARNESS` environment variable, or with `--harness <name>` on the `mcp`
subcommand — the flag wins if both are given:

| Harness | Store |
|---------|-------|
| `vibe` | `~/.vibe/logs/session/` |
| `codex` | `~/.codex/sessions/` |
| `claude` | `~/.claude/projects/` |
| `opencode` | `~/.local/share/opencode/opencode.db` |

No tool takes a harness parameter; the `harness` tool reports the bound value.
Register one server per store you want to mine. If the harness is unset or
unknown the server refuses to start: it prints the reason to stdout and stderr
and exits 2.

## Keep the server key as `total-recall`

The MCP tool names derive from the server key — `total-recall_total_recall`,
`total-recall_list_sessions`, … — so a renamed server shows up in the agent's
tool surface under the wrong identity.

## The environment block

The registration snippet's environment block carries two entries:

- `HARNESS` — which store this server is bound to.
- `INCEPTION_API_KEY` — the Mercury key, for the LLM-backed tools. A spawned
  server only finds a `.env` file relative to its working directory, which the
  MCP client controls, not you. Alternatively export `INCEPTION_API_KEY` in the
  shell that launches the client; the server inherits it.

The log-mining tools (`list_sessions`, `profile_session`, the three extraction
tools, `she_said_he_said_action`, `index_sessions`,
`do_android_dream_of_electric_sheep`) need no key. Only `compact_session` and
`total_recall` read it.

### Mistral Vibe

`~/.vibe/config.toml`:

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

### OpenCode

`~/.config/opencode/opencode.jsonc`:

```json
"mcp": {
  "total-recall": {
    "type": "local",
    "command": ["/path/to/total-recall", "mcp"],
    "environment": {"HARNESS": "opencode", "INCEPTION_API_KEY": "sk_..."}
  }
}
```

## Which vendor the server calls

The MCP server constructs the default provider. `--provider` is a CLI flag and
is not read by the `mcp` subcommand: a server that needs Mistral is not a
supported configuration, and a vendor-free build answers the two LLM-backed
tools with an error naming the feature to rebuild with — see
[Configuration](configuration.md#build-features).

## Confirming the binding

The `harness` tool is the cheapest check that a registration is right: it
returns the bound harness as JSON. If a tool answers "No sessions found", the
`HARNESS` value is the first thing to check, then the store root — a root
override in the environment is honoured here exactly as it is on the CLI, see
[Configuration](configuration.md#storage-root-overrides).
