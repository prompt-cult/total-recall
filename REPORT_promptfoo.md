# Promptfoo: Mercury Prompt Variant Experiments

## Summary Table

| Variant | Haiku Wins | Mercury Wins | Ties | Errors | Haiku Avg | Mercury Avg | Avg Time (s) |
|---------|-----------|-------------|------|--------|-----------|-------------|--------------|
| baseline | 11 | 9 | 1 | 3 | 72.4 | 71.5 | 0.0 |
| structured | 2 | 22 | 0 | 0 | 79.1 | 87.0 | 0.0 |
| no_tools | 13 | 7 | 0 | 4 | 70.4 | 66.9 | 0.0 |
| probe | 7 | 17 | 0 | 0 | 78.8 | 81.5 | 0.0 |
| reasoning_medium | 6 | 0 | 0 | 0 | 91.0 | 53.8 | 1.84 |
| no_tools_probe | 4 | 17 | 0 | 3 | 65.9 | 71.7 | 1.58 |

## Variant Descriptions

### baseline
- reasoning_effort: low
- system: You are a helpful coding assistant that summarizes conversations....
- user prompt (first 200 chars): Summarize this conversation focusing on:
- What was accomplished
- Current work in progress
- Files involved
- Next steps
- Key constraints or decisions

Be concise (3-5 bullet points max).

--- Conve...

### structured
- reasoning_effort: low
- system: You are a helpful coding assistant that summarizes conversations....
- user prompt (first 200 chars): Create a structured summary of this coding conversation. Use these exact sections:

## Accomplished
List what was completed.

## Current Work
What is being worked on now.

## Files Involved
List all f...

### no_tools
- reasoning_effort: low
- system: You are a helpful coding assistant that summarizes conversations....
- user prompt (first 200 chars): Summarize this conversation between a user and a coding assistant. Focus only on what the user asked for and what the assistant accomplished. Ignore tool execution details.

Include:
- What was accomp...

### probe
- reasoning_effort: low
- system: You are a helpful coding assistant that summarizes conversations....
- user prompt (first 200 chars): Summarize this conversation. Your summary will be used to continue the work, so preserve:

1. FACTUAL RECALL: Specific facts mentioned (file paths, error messages, API endpoints, model names, version ...

### reasoning_medium
- reasoning_effort: medium
- system: You are a helpful coding assistant that summarizes conversations....
- user prompt (first 200 chars): Summarize this conversation focusing on:
- What was accomplished
- Current work in progress
- Files involved
- Next steps
- Key constraints or decisions

Be concise (3-5 bullet points max).

--- Conve...

### no_tools_probe
- reasoning_effort: low
- system: You are a helpful coding assistant that summarizes conversations....
- user prompt (first 200 chars): Summarize this conversation between a user and a coding assistant. Focus on what the user asked and what was accomplished. Ignore tool execution noise.

Your summary will be used to continue the work,...

## Fresh Rerun (item06, hardened pipeline)

Re-ran the `structured` and `probe` variants fresh with the same prompts, rollout fixtures,
judge panel and orderings, but with the hardened parse + retry from item04
(`evaluate_chunked.parse_json_response`), glm-5.3-flash judge raised to `max_tokens=16000` /
300s timeout, and HTTP + parse retries with backoff. Fresh artifacts:
`outputs/promptfoo_fresh/` (per-verdict JSON + fresh Mercury summaries + `run_meta.json`),
aggregate in `results_promptfoo_fresh.json`. Nothing from the original run was overwritten.

| Variant | Original run | Fresh rerun |
|---------|--------------|-------------|
| structured | Mercury 22 - Haiku 2, avg 87.0 vs 79.1 | Mercury 21 - Haiku 3, avg 85.5 vs 78.9 |
| probe | Mercury 17 - Haiku 7, avg 81.5 vs 78.8 | Mercury 20 - Haiku 4, avg 84.3 vs 78.9 |

48/48 fresh judge verdicts parsed cleanly (0 errors). The `structured` win reproduced within
noise; `probe` came out slightly stronger than saved. Differences from the saved numbers are
within single-verdict flips, consistent with judge nondeterminism at temperature 0.1 across a
served-model version change on Zen between the runs.

The other variants in the table above were NOT re-run; their rows are the original Sep 10
results. Note their parse-error counts (baseline 3, no_tools 4, no_tools_probe 3) are
attributable to the old glm judge configuration (`max_tokens=2000` truncating behind
reasoning tokens) and would likely shrink under the hardened pipeline; no action taken.

Decision applied: the `structured` prompt stays as the shipped default (`src/prompt.rs`,
`build_structured_prompt`, used by both the Rust CLI and the MCP server; identical to
`structured_prompt.txt` and the system prompt in `mercury.py`). `cargo test` green.
