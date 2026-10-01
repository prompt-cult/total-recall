# Security Policy

## Supported versions

| Version | Supported |
|---------|-----------|
| 0.7.x | yes |
| < 0.7 | no |

Security fixes land on `main` and are released in the next patch tag; see
[CHANGELOG.md](CHANGELOG.md). There is no LTS branch.

## API keys

Two API keys are read, both only by the LLM-backed tools
(`compact_session` / `compact`, `total_recall` / `recall`):

| Variable | Vendor | Read by |
|----------|--------|---------|
| `INCEPTION_API_KEY` | Inception Mercury 2.5 (default) | `src/mercury.rs` |
| `MISTRAL_API_KEY` | Mistral (`--provider mistral`) | `src/mercury.rs` |

A key reaches the binary one of two ways:

- **Environment / MCP environment block.** An MCP server finds a key only in
  its own environment, so the key belongs in the registration snippet's
  `environment` map (or `[mcp_servers.env]`). Exporting it in the shell that
  launches the client works too — the server inherits it. This is the
  recommended path: a spawned server's working directory is chosen by the MCP
  client, not by you, so a `.env` file may simply not be found.
- **A `.env` file**, loaded by `dotenvy` at the point a vendor is constructed.
  Only the key name matters; the file is never written by this tool.

`.env` is gitignored (`.gitignore` matches both `*.env` and `.env`), and only
`.env.template` — which holds placeholder values — is tracked. Never commit a
key. A key in a committed file is a compromised key: rotate it at the vendor
immediately, then remove it from history.

A **vendor-free build** (`cargo build --release --no-default-features`) reads
no key at all and does not even load a `.env`: `dotenvy` sits behind the
`mercury` and `mistral` features. Every log-mining tool works in that build.

## What leaves your machine

**No telemetry, ever.** There is no analytics, no crash reporting, no usage
ping, no update check. The only outbound requests the binary can make are the
two LLM chat-completion endpoints, and only when an LLM-backed tool is called:

- `https://api.inceptionlabs.ai/v1/chat/completions`
- `https://api.mistral.ai/v1/chat/completions`

Everything else is local. If you never call `compact` or `recall`, nothing is
sent anywhere.

### Session content is redacted before it is sent

When you call an LLM-backed tool, the session's messages — your prompts, the
assistant's replies, and tool calls — are sent to the vendor's API to be
summarized. Before they go, every credential shape in them is replaced with a
`[REDACTED:<kind>]` marker. This is unconditional: there is no switch, no
environment variable, and no vendor-by-vendor opt-out, on the CLI or over MCP.

The classes that are redacted:

| Class | What it covers | Marker |
|-------|----------------|--------|
| Vendor keys | `sk_` (Inception), `sk-` (OpenAI), `sk-ant-` (Anthropic), `tvly-` (Tavily), `ctx7sk-` (Context7), plus Slack, GitLab and Google shapes | `[REDACTED:vendor-key]` |
| GitHub tokens | `ghp_`, `gho_`, `ghu_`, `ghs_`, `ghr_`, `github_pat_` | `[REDACTED:github-token]` |
| Authorization | the value of a `Bearer`/`Basic`/`Digest` header | `[REDACTED:bearer]` |
| Assignments | `api_key`, `apikey`, `token`, `secret`, `password`, `passwd`, `passphrase`, `private_key`, `credentials`, `authorization` in `key = value`, `key: value`, `"key":"value"` and query-string forms | `[REDACTED:secret]` |
| Private keys | `-----BEGIN … PRIVATE KEY-----` blocks, including one whose `END` was cut off | `[REDACTED:pem]` |
| JWTs | three-segment `eyJ…` tokens | `[REDACTED:jwt]` |

A shape is treated as a secret on sight. Nothing asks whether a match is real
or live, because that judgement is the one that fails open. Redaction therefore
errs towards over-redacting: a hyphenated identifier or a word that reads like
a credential name can be replaced when it is not one. That costs a little
summary fidelity and is the correct trade against shipping a key.

Two further guardrails apply to the prompt itself, both documented in
[README](README.md#ingestion-guardrails):

- Tool results are snipped to 500 characters during prompt assembly.
- A per-call input cap rejects an over-large prompt with an error instead of
  truncating it.

The key name and the surrounding structure survive, so a summary still shows
*which* field was removed. Redaction runs before the 500-character tool-result
snip rather than after, so a key cannot be cut in half and have its surviving
prefix shipped; the log-mining tools
(`list_sessions`, `profile_session`, `extract_messages`,
`extract_user_messages`, `extract_by_type`, `she_said_he_said_action`,
`index_sessions`, `do_android_dream_of_electric_sheep`) make no outbound call
at all and never transmit content.

Two limits worth stating plainly. A credential in prose with no separator —
"the password is hunter-two" — is not an assignment and is not matched; a
shape matcher cannot tell it from an ordinary sentence. And a credential in a
format none of the classes above describes will pass through. If you are
pasting something you would not want a third party to read, the log-mining
tools and the vendor-free build (`--no-default-features`) make no outbound call
whatsoever.

## Reading your session stores

The session stores are the sensitive part: they are your prompts, your source
code, and your tool output.

**The stores are read-only.** SQLite is opened `SQLITE_OPEN_READ_ONLY`
(`src/rollout/opencode.rs`) and term matching for
`she_said_he_said_action` runs on a read-only connection. The tool never
writes, migrates, or repairs a session store.

**The only files the binary writes** are inside the adapter's shadow root,
never inside the session store:

| Written | Path |
|---------|------|
| tantivy indexes | `<shadow root>/<session_id>/` |
| index marker | `<shadow root>/<session_id>/total-recall-meta.json` |
| profile cache (opt-in) | `<shadow root>/tr_<session_id>_meta.json` |

| Harness | Shadow root |
|---------|-------------|
| opencode | `~/.local/share/opencode/.tantivy/` |
| vibe | `~/.vibe/logs/.tantivy/` |
| codex | `~/.codex/.tantivy/` |
| claude | `~/.claude/.tantivy/` |

These are disposable derived data. Deleting them costs only a rebuild. Never
commit them.

The one script that writes to a live store is
[`scripts/opencode-db-vacuum.sh`](scripts/opencode-db-vacuum.sh), an
operator-run maintenance tool for a store that has grown to tens of
gigabytes. It is not part of the binary, it is never invoked automatically,
and it takes an explicit subcommand (`prune-events`, `swap`). Read
[docs/opencode-store-vacuum.md](docs/opencode-store-vacuum.md) before running
it; back up the store first.

## Storage-root overrides and the sandbox guard

By default each adapter resolves its store from `$HOME` — the live store. To
point it at fixtures or a scratch copy instead, set the harness's override:

| Harness | Env var |
|---------|---------|
| vibe | `TOTAL_RECALL_VIBE_ROOT` |
| claude | `TOTAL_RECALL_CLAUDE_ROOT` |
| codex | `TOTAL_RECALL_CODEX_ROOT` |
| opencode | `TOTAL_RECALL_OPENCODE_ROOT` |

Precedence: an explicit `with_root` (tests) > a non-empty env override > the
`$HOME` default. An empty value counts as unset, and an override that resolves
to nothing usable is an error — never a silent fall-back to `$HOME`.

`TOTAL_RECALL_SANDBOX=1` arms a guard that **fails closed**: `make_adapter`
refuses to build any adapter whose root did not come from its override. Armed
with no override set, the tool cannot read a live store at all. Use it for any
development or test run that touches this code.

## Unreadable data is an error, not an empty result

A payload that exists but cannot be read — permissions, I/O error, corrupt
record — is never reported as an empty session. Every adapter read path returns
`Result<T, String>`: the failure surfaces as a tool error naming the path (or
CLI exit 2), `index_sessions` propagates it rather than building an empty
index, and a malformed JSONL line is skipped with a `WARN` carrying its line
number. Silently-shrinking results are treated as bugs.

## Reporting a vulnerability

Report privately through GitHub's private vulnerability reporting on this
repository: **Security** → **Report a vulnerability** at
[prompt-cult/total-recall](https://github.com/prompt-cult/total-recall). That
form reaches the maintainer alone — a public issue is the wrong channel for a
vulnerability report, because publishing the detail first removes the user's
ability to patch.

Please include: the version, the harness, the exact command or MCP tool call,
what you observed, and what you expected. A proof of concept is welcome.

What to expect: acknowledgement, then a fix and a release, then a credit in
[CHANGELOG.md](CHANGELOG.md) unless you ask not to be named.
