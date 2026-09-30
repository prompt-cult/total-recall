# Configuration

## Storage-root overrides

Each adapter resolves its storage root from `$HOME` by default — the live
store. To point the CLI and the MCP server at fixtures or a scratch copy
instead, without ever touching live sessions, set the harness's root override.

| Harness | Env var | Default root |
|---------|---------|--------------|
| `vibe` | `TOTAL_RECALL_VIBE_ROOT` | `~/.vibe/logs/session/` |
| `claude` | `TOTAL_RECALL_CLAUDE_ROOT` | `~/.claude/projects/` |
| `codex` | `TOTAL_RECALL_CODEX_ROOT` | `~/.codex/sessions/` |
| `opencode` | `TOTAL_RECALL_OPENCODE_ROOT` | `~/.local/share/opencode/opencode.db` (a directory also works; `opencode.db` inside it is used) |

Precedence: an explicit root (tests) > the env override (non-empty) > the
`$HOME`-derived default. An empty value counts as unset. An override that
resolves to nothing usable is an error, never a silent fall-back to `$HOME`.

The shadow index root follows the session root, so an override moves the
tantivy indexes and the profile cache with it.

## The sandbox guard

`TOTAL_RECALL_SANDBOX=1` refuses to build any adapter whose root did not come
from its override. With the sandbox armed and no override set, adapter
construction fails closed with a descriptive error instead of reading the live
store — a mechanical guarantee that sandboxed dev and test runs never read live
sessions:

```
TOTAL_RECALL_SANDBOX is set but TOTAL_RECALL_VIBE_ROOT is not: refusing to read
the live store. Point TOTAL_RECALL_VIBE_ROOT at a fixture or scratch copy.
```

The guard is a constructor check, so it covers every tool: a sandboxed run
cannot read the live store by any route.

## API keys

| Variable | Vendor | Read by | Needed for |
|----------|--------|---------|------------|
| `INCEPTION_API_KEY` | Inception Mercury 2.5 (default) | `src/mercury.rs` | `compact_session` / `compact`, `total_recall` / `recall` |
| `MISTRAL_API_KEY` | Mistral (`--provider mistral`) | `src/mercury.rs` | the same, on the Mistral vendor |

A key reaches the binary one of two ways:

- **Environment / MCP environment block.** An MCP server finds a key only in
  its own environment, so the key belongs in the registration snippet's
  environment map (or `[mcp_servers.env]`). Exporting it in the shell that
  launches the client works too — the server inherits it. This is the
  recommended path: a spawned server's working directory is chosen by the MCP
  client, not by you, so a `.env` file may simply not be found.
- **`.env` in the working directory**, for the build-from-source CLI path: copy
  `.env.template` to `.env` and fill in the key. Only a default build reads it.

## Build features

The LLM vendor is a **compile-time cargo feature**. A vendor is a third-party
service that can be acquired, renamed or disappear entirely, so no build of
this tool requires one: the default build has both vendors, and a vendor-free
build is a first-class artifact.

```toml
[features]
default = ["mercury", "mistral"]
mercury = ["dep:dotenvy"]   # Inception Mercury 2.5 (the default provider)
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
| `list_sessions`, `profile_session`, `extract_messages`, `extract_user_messages`, `extract_by_type`, `she_said_he_said_action`, `index_sessions`, `do_android_dream_of_electric_sheep`, `harness` | works | works |
| `compact_session` (MCP), `compact` (CLI) | registered; returns an error naming the missing feature | calls Mercury, or `--provider mistral` |
| `total_recall` (MCP), `recall` (CLI) | registered; returns an error naming the missing feature | calls Mercury, or `--provider mistral` |

The LLM-backed **MCP tool names stay in `tools/list` in every build** — clients
bind by name — and the call itself returns the error:

```
LLM provider `mercury` is not compiled into this build: it was built without the
`mercury` cargo feature. Rebuild with `cargo build --release --features mercury`,
or use the vendor-free build (`--no-default-features`) — it makes no LLM calls and
needs no API key, and every log-mining tool still works. Vendors compiled into
this build: none.
```

## Provider selection

`--provider` selects between the vendors compiled into the build (`mercury`,
`mistral`); a vendor that is not compiled in fails the same way, and an
unrecognised name is refused rather than silently falling back to Mercury:

```
unknown LLM provider `gpt`: pass it to --provider. Known vendors: mercury,
mistral; compiled into this build: mercury, mistral.
```

Vendor identity (endpoint, model, key variable) is the gated part. The
OpenAI-compatible chat transport itself compiles in every build but is
unreachable without a vendor constructor.

`--provider` applies to the CLI's `compact` and `recall` subcommands. The
`mcp` subcommand constructs the default provider; see
[MCP setup](mcp-setup.md#which-vendor-the-server-calls).

## `OPENCODE_DB`

Not read by the binary. `scripts/opencode-db-vacuum.sh` reads it to locate the
store it operates on, defaulting to
`$HOME/.local/share/opencode/opencode.db`. See the
[store-vacuum runbook](operations/opencode-store-vacuum.md).

## What is written where

Nothing is written inside a session store. total-recall creates files only in
the harness's shadow root:

| Harness | Shadow root | Contents |
|---------|-------------|----------|
| `opencode` | `~/.local/share/opencode/.tantivy/` | `<session_id>/` index folders, `tr_<session>_meta.json` profile caches |
| `vibe` | `~/.vibe/logs/.tantivy/` | same |
| `codex` | `~/.codex/.tantivy/` | same |
| `claude` | `~/.claude/.tantivy/` | same |

Both are disposable: delete the folder at any time, or override the session
root to move it. A shadow root inside a git repository MUST NOT be committed.

## Pricing

Mercury 2.5, launch (80% off) versus normal:

| | Launch | Normal |
|---|---|---|
| Input | $0.04/M tokens | $0.20/M tokens |
| Output | $0.15/M tokens | $0.75/M tokens |
| Speed | 1,107 tokens/sec | same |
| Context | 260K tokens | same |

100M free tokens for new accounts.
