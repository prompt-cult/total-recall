# total-recall

Total-recall MCP tool for fast compaction and log mining of session rollouts as a long-term memory store. Uses [Inception Mercury 2.5](https://www.inceptionlabs.ai) — a diffusion LLM running at 1,100 tokens/sec — to compact long agent sessions and mine them for goals, tasks, and steers.

## Why

Every CLI coding agent (Claude Code, Codex CLI, OpenCode, Mistral Vibe) hits context limits on long sessions. Their built-in compaction is slow and uses expensive frontier models. Mercury 2.5 does the same job at 1,100 tok/s for $0.04/M input and $0.15/M output (launch pricing).

Augment Code [moved compaction to Mercury and cut latency 82%](https://www.inceptionlabs.ai/blog/introducing-mercury-2-5) (150s to 27s) and costs 90%. This tool does the same for your CLI agent sessions.

## What it does

### `compact` — fast compaction

1. Detects the session format (JSONL or SQLite) — works across all tools
2. Finds the last compaction point in the session
3. Streams messages since that point to Mercury 2.5
4. Returns a structured summary (Accomplished, Current Work, Files, Next Steps, Key Decisions)

### `list_sessions` — rollout index

All rollouts for the bound harness with id, title, times, directory, message
and tool counts, byte size, parent/children. Optional bounds: `hours_back`
(default 0 = all) and `directory` substring filter. The OpenCode SQLite
implementation is a single GROUP BY query (no correlated subqueries), so the
index over thousands of sessions reads at disk speed.

### `she_said_he_said_action` — matched dialogue and actions

Given case-insensitive terms and a session list (partial IDs) — or, when the
list is empty, all rollouts updated within `hours_back` hours (default 48)
optionally filtered by `directory` substring — extracts per session:

- **HE SAID** — user text parts matching any term
- **SHE SAID** — assistant text parts matching any term
- **THEY DID** — tool calls whose name or arguments match any term

Matching is pushed down into SQLite as a custom scalar function on a
read-only connection: the database is never written, matching runs inside
the query engine, and matching rows stream out in one pass. Synthetic parts
(skill injections, compaction markers) are skipped. Output is a markdown
report, sessions ordered most-recent first. Currently implemented for
OpenCode; other harnesses return a clear unsupported error.

## Ingestion guardrails

Mercury calls are guarded by measured, documented limits (probed against a
Pay-As-You-Go key on 2026-09-15 with real rollout payloads, 5k/10k/20k
tokens, concurrency ramped 1→64):

- **The binding limit is ~1M input tokens/minute** (the documented PAYG
  input cap). Requests/min (1,000) and output tokens/min (100,000) never
  bind for compaction workloads.
- **Per-call input cap: 1M tokens** (~4 chars/token). A 100MiB rollout is
  never slung in one call. Note Mercury 2.5's documented context window is
  260K tokens, so practical calls stay well below the cap — tool results
  are snipped to 500 chars in prompt assembly, keeping prompts small.
- **Concurrency: 4 in-flight requests** with ~10k-token prompts. Measured
  sweet spot: ~22k input tok/s with zero rejections, p50 latency 1.9s
  (latency is flat across payload sizes and concurrency). Beyond
  concurrency 8 the 429 wall arrives with no throughput gain.
- **429 + `Retry-After` exponential backoff, 5xx retry**: the documented
  correct behaviour; the server recovers immediately after backoff.

### `recall` — total recall

1. Makes two parallel Mercury 2.5 calls: current state summary + user goals/tasks/steers
2. Lists recent rollouts (24h filter, configurable) with the current session marked SUMMARISED
3. Lists plan/todo files from `.tmp/delegation/` and `~/.vibe/plans/`
4. Assembles output in deliberate ordering for autoregressive LLMs: metadata first, substance middle, instructions last

## Supported tools

| Tool | Storage | Chat history repos / gists | Rust CLI |
|------|---------|---------------------------|----------|
| Mistral Vibe | JSONL (`~/.vibe/logs/session/`) | [gist: mistral-vibe-chat-history](https://gist.github.com/simbo1905/b79ba81f637e9e235d55e4853e3dc299) | Yes |
| OpenCode | SQLite (`~/.local/share/opencode/`) | [simbo1905/opencode-chat-history](https://github.com/simbo1905/opencode-chat-history) | Yes (`--harness opencode`) |
| Codex CLI | JSONL + SQLite | [simbo1905/codex-chat-history](https://github.com/simbo1905/codex-chat-history) | Yes |
| Claude Code | JSONL | [simbo1905/claude-chat-history](https://github.com/simbo1905/claude-chat-history) | Yes |

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
cargo build --release
B=./target/release/total-recall

# List rollouts for a harness (--harness is required; vibe | codex | claude | opencode)
$B --harness vibe list

# Profile a rollout (counts, compaction markers, interesting events)
$B --harness vibe --session 4836855e profile

# Extract what the user said verbatim
$B --harness vibe --session 4836855e user-messages --markdown

# Compact from the last compaction point (default) or the full rollout
$B --harness vibe --session 4836855e compact
$B --harness vibe --session 4836855e --full compact

# Total recall: state summary + user goals/steers + rollouts table + plan files
$B --harness vibe --session 4836855e recall

# She-said/he-said/they-did: matched dialogue and tool actions
$B --harness opencode he-said-she-said --words git,branch,tag,worktree --hours 48 --directory uvrr-core
$B --harness opencode he-said-she-said --words git --sessions ses_f5a4ea5e ses_f5b87b1c

# Total recall with Mistral instead of Mercury (for A/B comparison)
$B --harness vibe --session 4836855e --provider mistral recall
```

Requires `INCEPTION_API_KEY` in `.env` (see `.env.template`).
For `--provider mistral`, set `MISTRAL_API_KEY` instead.

## MCP server

The binary runs as an MCP stdio server exposing `harness`, `list_sessions`
(with optional `hours_back`/`directory` bounds), `profile_session`,
`extract_messages`, `extract_user_messages`, `compact_session`,
`she_said_he_said_action`, and `total_recall`:

```bash
$B mcp
```

Each server instance is bound to exactly ONE harness, set by the installing
config via the `HARNESS` environment variable (or `--harness <name>` on the
`mcp` subcommand — the flag wins if both are given). Tools take no harness
parameter; the `harness` tool reports the bound value. If the harness is
unset or unknown the server refuses to start: it prints the reason to stdout
and stderr and exits 2.

Register it in Mistral Vibe (`~/.vibe/config.toml`):

```toml
[[mcp_servers]]
name = "compaction"
transport = "stdio"
command = "/path/to/total-recall"
args = ["mcp"]

[mcp_servers.env]
HARNESS = "vibe"
```

Register it in OpenCode (`~/.config/opencode/opencode.jsonc`):

```json
"mcp": {
  "compaction": {
    "type": "local",
    "command": ["/path/to/total-recall", "mcp"],
    "environment": {"HARNESS": "opencode"}
  }
}
```


## Setup

1. Get an Inception API key from [https://platform.inceptionlabs.ai](https://platform.inceptionlabs.ai)
2. Copy `.env.template` to `.env` and fill in your key
3. `cargo build --release` and use the CLI above

## Requirements

- Rust toolchain (stable)
- `INCEPTION_API_KEY` in environment or `.env`
- Optional: `MISTRAL_API_KEY` for `--provider mistral`

## Mercury 2.5 pricing

| | Launch (80% off) | Normal |
|---|---|---|
| Input | $0.04/M tokens | $0.20/M tokens |
| Output | $0.15/M tokens | $0.75/M tokens |
| Speed | 1,107 tokens/sec | same |
| Context | 260K tokens | same |

100M free tokens for new accounts.
