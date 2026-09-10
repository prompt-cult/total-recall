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
