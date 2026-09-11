# inception-mercury-compaction

Universal context compaction tool for CLI coding agents. Uses [Inception Mercury 2.5](https://www.inceptionlabs.ai) — a diffusion LLM running at 1,100 tokens/sec — to compact long agent sessions.

## Why

Every CLI coding agent (Claude Code, Codex CLI, OpenCode, Mistral Vibe) hits context limits on long sessions. Their built-in compaction is slow and uses expensive frontier models. Mercury 2.5 does the same job at 1,100 tok/s for $0.04/M input and $0.15/M output (launch pricing).

Augment Code [moved compaction to Mercury and cut latency 82%](https://www.inceptionlabs.ai/blog/introducing-mercury-2-5) (150s to 27s) and costs 90%. This tool does the same for your CLI agent sessions.

## What it does

1. Detects the session format (JSONL or SQLite) — works across all tools
2. Finds the last compaction point in the session
3. Streams messages since that point to Mercury 2.5
4. Writes a compaction summary that can be fed back into the agent

## Supported tools

| Tool | Storage | Chat history repos / gists |
|------|---------|---------------------------|
| Mistral Vibe | JSONL (`~/.vibe/logs/session/`) | [gist: mistral-vibe-chat-history](https://gist.github.com/simbo1905/b79ba81f637e9e235d55e4853e3dc299) |
| OpenCode | SQLite (`~/.local/share/opencode/`) | [simbo1905/opencode-chat-history](https://github.com/simbo1905/opencode-chat-history) |
| Codex CLI | JSONL + SQLite | [simbo1905/codex-chat-history](https://github.com/simbo1905/codex-chat-history) |
| Claude Code | JSONL | [simbo1905/claude-chat-history](https://github.com/simbo1905/claude-chat-history) |
| Cursor | SQLite | [simbo1905/cursor-chat-history](https://github.com/simbo1905/cursor-chat-history) |

## Compaction prompt research

The compaction prompts and strategies in this tool are informed by [badlogic's context compaction research](https://gist.github.com/badlogic/cd2ef65b0697c4dbe2d13fbecb0a0a5f) comparing Claude Code, Codex CLI, OpenCode, and Amp.

Key findings from that research:
- Claude Code triggers at ~95% context capacity, uses a summary prompt
- Codex CLI preserves recent user messages (last ~20k tokens) alongside the summary
- OpenCode has a separate "prune" mechanism for tool outputs beyond 40k tokens
- Amp uses manual "handoff" instead of auto-compaction

This tool takes the best of each: it preserves recent messages, prunes large tool outputs, and uses Mercury 2.5 for fast summarization.

## Usage

```bash
# Compact the current Mistral Vibe session
./compact.py --tool vibe

# Compact a specific session by ID
./compact.py --tool vibe --session 7b00dfb9

# Compact the latest OpenCode session
./compact.py --tool opencode

# Dry run — show what would be compacted without calling the API
./compact.py --tool vibe --dry-run

# Use a different Mercury model
./compact.py --tool vibe --model mercury-2
```

## Rust CLI (this crate)

The Rust binary is the primary implementation. `compact.py` remains the
seed/reference script.

```bash
cargo build --release
B=./target/release/inception-mercury-compaction

# List rollouts for a harness (vibe | codex | claude)
$B --harness vibe list

# Profile a rollout (counts, compaction markers, interesting events)
$B --harness vibe --session 4836855e profile

# Extract what the user said verbatim
$B --harness vibe --session 4836855e user-messages --markdown

# Compact from the last compaction point (default) or the full rollout
$B --harness vibe --session 4836855e compact
$B --harness vibe --session 4836855e --full compact
```

Requires `INCEPTION_API_KEY` in `.env` (see `.env.template`).

## MCP server

The binary runs as an MCP stdio server exposing `list_sessions`,
`profile_session`, `extract_messages`, `extract_user_messages`, and
`compact_session`:

```bash
$B mcp
```

Register it in Mistral Vibe (`~/.vibe/config.toml`):

```toml
[[mcp_servers]]
name = "compaction"
transport = "stdio"
command = "/path/to/inception-mercury-compaction"
args = ["mcp"]
```

Register it in OpenCode (`~/.config/opencode/opencode.jsonc`):

```json
"mcp": {
  "compaction": {
    "type": "local",
    "command": ["/path/to/inception-mercury-compaction", "mcp"]
  }
}
```

## Setup

1. Get an Inception API key from [https://platform.inceptionlabs.ai](https://platform.inceptionlabs.ai)
2. Copy `.env.template` to `.env` and fill in your key
3. Run `./compact.py --tool vibe` (or the Rust CLI above)

## Requirements

- Python >= 3.11 (via [uv](https://docs.astral.sh/uv/))
- `INCEPTION_API_KEY` in environment or `.env`
- No other dependencies — the script uses stdlib only

## Mercury 2.5 pricing

| | Launch (80% off) | Normal |
|---|---|---|
| Input | $0.04/M tokens | $0.20/M tokens |
| Output | $0.15/M tokens | $0.75/M tokens |
| Speed | 1,107 tokens/sec | same |
| Context | 260K tokens | same |

100M free tokens for new accounts.
