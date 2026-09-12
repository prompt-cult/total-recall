# Mercury Chunked vs Haiku Full: Summarization Comparison

## Methodology

- **Haiku**: Summarizes the full rollout in one call (Anthropic Messages API via Zen)
- **Mercury**: Splits rollout in half, summarizes each half separately, concatenates
- **Judges**: glm-5.3-flash, gpt-5.4-mini, kimi-k3 (all via Zen, max_tokens=2000)
- **Orderings**: A-B and B-A to detect ordering bias

## Timings

| Rollout | Haiku (s) | Mercury Half 1 (s) | Mercury Half 2 (s) | Mercury Total (s) | Speedup |
|---------|-----------|--------------------|--------------------|-------------------|---------|
| vibe | 4.64 | 1.82 | 0.94 | 2.76 | 1.7x |
| opencode | 6.27 | 1.39 | 1.5 | 2.89 | 2.2x |
| codex | 3.98 | 1.11 | 1.18 | 2.29 | 1.7x |
| claude | 3.8 | 1.62 | 1.17 | 2.79 | 1.4x |

## Results

| Metric | Haiku (full) | Mercury (concat) |
|--------|-------------|-----------------|
| Wins | 20 | 4 |
| Ties | 0 | |
| Errors | 0 | |
| Avg Score | 85.5 | 72.0 |

## Individual Evaluations

### vibe | AB | glm-5.3-flash
- Winner: **A**
- Score A: 84 | Score B: 66
- Confidence: 74
- Reasoning: Summary A is internally consistent, concise, and clearly captures the final state: Zen API outage limiting models to DeepSeek/GLM/Kimi/MiniMax, evaluate.py and REPORT.md updated with working models, a

### vibe | AB | gpt-5.4-mini
- Winner: **A**
- Score A: 91 | Score B: 62
- Confidence: 95
- Reasoning: A is more accurate and internally consistent with the stated conversation: it captures the Zen API outage, the switch to working DeepSeek/GLM/Kimi models, the updated files, and the need to rerun eval

### vibe | AB | kimi-k3
- Winner: **A**
- Score A: 80 | Score B: 66
- Confidence: 62
- Reasoning: Summary A is coherent, well-structured, and clearly conveys the current state: Zen API outage diagnosed, working models identified, evaluate.py and REPORT.md updated, and re-run pending. Summary B app

### vibe | BA | glm-5.3-flash
- Winner: **B**
- Score A: 70 | Score B: 86
- Confidence: 65
- Reasoning: Summary A contains rich detail (Mistral-based results of 9/24 wins, 76.0 avg, ordering bias, more files listed) but is internally contradictory: one section declares the task complete with no next ste

### vibe | BA | gpt-5.4-mini
- Winner: **B**
- Score A: 62 | Score B: 88
- Confidence: 90
- Reasoning: B is more accurate and concise, capturing the key issue that the Zen API only works with certain models and that the evaluation needs to be rerun. A includes useful details but mixes in conflicting or

### vibe | BA | kimi-k3
- Winner: **B**
- Score A: 58 | Score B: 84
- Confidence: 78
- Reasoning: Summary A contains two concatenated, contradictory summaries: one claims the evaluation is complete (Mercury won 9/24, committed/pushed, no next steps), while the other says work is in progress troubl

### opencode | AB | glm-5.3-flash
- Winner: **B**
- Score A: 78 | Score B: 85
- Confidence: 62
- Reasoning: Summary B captures more concrete substance of the conversation, including the reconstructed methodology specifics (5 models, 10 pairwise comparisons, 3 judges) and the diversity-requirements check, wh

### opencode | AB | gpt-5.4-mini
- Winner: **A**
- Score A: 89 | Score B: 78
- Confidence: 82
- Reasoning: A is more complete and clearly structured, capturing accomplishments, work in progress, files, next steps, and important constraints. B is also accurate but is slightly less complete and adds some lik

### opencode | AB | kimi-k3
- Winner: **A**
- Score A: 85 | Score B: 62
- Confidence: 78
- Reasoning: Summary A is well-organized with a single coherent structure (Accomplished, In Progress, Files, Next Steps, Constraints), includes specific verifiable details (session ID, date, file paths, pairwise_g

### opencode | BA | glm-5.3-flash
- Winner: **B**
- Score A: 78 | Score B: 88
- Confidence: 72
- Reasoning: Summary B is better organized with clear sections and includes richer specifics: the exact session ID and date, the pairwise_grade.py tool, 2000-character truncation boundaries, and the important cons

### opencode | BA | gpt-5.4-mini
- Winner: **B**
- Score A: 58 | Score B: 86
- Confidence: 82
- Reasoning: B is more accurate and complete: it captures the key recovery of the methodology, the single relevant session, the pasted transcript source, and the need to distinguish genuine user statements. A incl

### opencode | BA | kimi-k3
- Winner: **B**
- Score A: 58 | Score B: 88
- Confidence: 88
- Reasoning: Summary A appears to be two partial summaries concatenated together, with redundant 'Accomplished/Files/Next Steps/Constraints' sections that overlap in content, hurting clarity and conciseness. Summa

### codex | AB | glm-5.3-flash
- Winner: **B**
- Score A: 82 | Score B: 88
- Confidence: 65
- Reasoning: Both summaries accurately capture the two deliverables (Snake and Space Invaders games), file paths, PEP 723/uv shebang decisions, and readiness to run. Summary A is more concise and includes the suba

### codex | AB | gpt-5.4-mini
- Winner: **B**
- Score A: 78 | Score B: 92
- Confidence: 88
- Reasoning: B is more complete and clear: it captures both the Snake and Space Invaders work, files involved, key execution details, and next steps. A is concise but misses some specific gameplay/run details and 

### codex | AB | kimi-k3
- Winner: **A**
- Score A: 85 | Score B: 78
- Confidence: 65
- Reasoning: Both summaries accurately capture the two games created, the files involved, the PEP 723/uv shebang decisions, and readiness to run. Summary B includes slightly more granular detail (game mechanics, c

### codex | BA | glm-5.3-flash
- Winner: **A**
- Score A: 90 | Score B: 84
- Confidence: 62
- Reasoning: Summary A captures more specific details (collision detection, score tracking, arrow key/spacebar controls, SpaceInvadersGame class) and contains no factual errors. Summary B is better organized and m

### codex | BA | gpt-5.4-mini
- Winner: **B**
- Score A: 72 | Score B: 84
- Confidence: 86
- Reasoning: Both summaries capture the main accomplishments and files, but B is clearer and more concise. A adds some likely accurate detail, but it is more repetitive and slightly less polished. B better organiz

### codex | BA | kimi-k3
- Winner: **B**
- Score A: 74 | Score B: 85
- Confidence: 78
- Reasoning: Both summaries accurately capture the key facts: two games (Snake, Space Invaders) created at /tmp/ with PEP 723 metadata, uv shebang, pygame dependency, and subagent involvement. Summary A provides s

### claude | AB | glm-5.3-flash
- Winner: **A**
- Score A: 88 | Score B: 74
- Confidence: 78
- Reasoning: Summary A is well-organized, concise, and clearly captures the key facts: the tag name, the three extracted files, the purpose (hints for another project), and the decision to use git tags since spike

### claude | AB | gpt-5.4-mini
- Winner: **A**
- Score A: 92 | Score B: 72
- Confidence: 90
- Reasoning: A is more accurate and coherent: it captures the extracted spike17 files, the use of the archived tag, and the purpose of reviewing them as hints for another project. B includes some correct details, 

### claude | AB | kimi-k3
- Winner: **A**
- Score A: 86 | Score B: 72
- Confidence: 78
- Reasoning: Both summaries capture the same core facts: extraction of 3 files (README.md, whisper-3-large.py, 0000_host.mp3) from tag 20260702_120900_main_spike17 into .tmp/, the purpose (hints for another projec

### claude | BA | glm-5.3-flash
- Winner: **B**
- Score A: 76 | Score B: 86
- Confidence: 70
- Reasoning: Summary B is well-organized, concise, and clearly captures the core work (extracting spike17 files from the git tag into .tmp/, the three files involved, and next steps). Summary A contains more granu

### claude | BA | gpt-5.4-mini
- Winner: **B**
- Score A: 72 | Score B: 88
- Confidence: 87
- Reasoning: Both summaries capture the main actions, but B is more concise and clearer. It accurately covers the extracted files, current work, and next steps without the extra repetition and some ambiguous wordi

### claude | BA | kimi-k3
- Winner: **B**
- Score A: 66 | Score B: 87
- Confidence: 78
- Reasoning: Both summaries capture the core facts accurately (spike17 tag extraction, 3 files into .tmp/, reference use for another project). However, Summary A appears to contain two overlapping summaries concat

## Reconciliation

Two experiments in this repo answer different questions, and their headline numbers differ:

- `REPORT.md` (this file, from `evaluate_chunked.py`): Mercury **chunked-concat** (rollout split
  in half, each half summarized, halves concatenated) with the bullet-style prompt, judged vs
  Haiku summarizing the full rollout. Verdict: **Haiku 20 - Mercury 4** (0 ties, 0 errors),
  avg 85.5 vs 72.0 over 24 verdicts (4 rollouts x 2 orderings x 3 judges).
- `REPORT_promptfoo.md` (from `promptfoo_experiment.py`): Mercury **full-rollout single call**
  with the tuned `structured` prompt vs the same Haiku baseline. Verdict: **Mercury 22 - Haiku 2**,
  avg 87.0 vs 79.1; `probe` variant 17:7, avg 81.5 vs 78.8.

### Fresh rerun (item06, this reconciliation)

To rule out stale or truncated-judge artefacts, both tuned variants were re-run fresh
(`outputs/promptfoo_fresh/`, `results_promptfoo_fresh.json`): same prompts, same 4 rollout
fixtures, same 3-judge panel and A-B/B-A orderings, but with the hardened JSON parser and
single-retry from item04 (`evaluate_chunked.parse_json_response`), glm-5.3-flash judge at
`max_tokens=16000` / 300s timeout (it spends completion tokens on reasoning; the original
promptfoo run used 2000, which explains its baseline/no_tools parse-error verdicts), and HTTP +
parse retries with backoff. All 48 fresh judge calls returned parseable verdicts; 0 errors.

| Variant | Saved (Sep 10) | Fresh (item06) |
|---------|----------------|----------------|
| structured | Mercury 22 - Haiku 2, avg 87.0 vs 79.1 | Mercury 21 - Haiku 3, avg 85.5 vs 78.9 |
| probe | Mercury 17 - Haiku 7, avg 81.5 vs 78.8 | Mercury 20 - Haiku 4, avg 84.3 vs 78.9 |

The `structured` result **reproduced**. Per the pre-registered decision rule, the structured
prompt is confirmed as the shipped default: `src/prompt.rs` (`STRUCTURED_PROMPT` +
`SYSTEM_PROMPT`) already contains exactly this prompt (verified byte-identical to the
promptfoo `structured` config, and matching `structured_prompt.txt` / `mercury.py`); the Rust
tool and MCP server use it via `build_structured_prompt`. No code change was required.

### What each experiment supports, and what the contradiction actually is

- `REPORT_promptfoo.md` supports: given the full rollout in one call, Mercury-2.5 with the
  structured prompt produces summaries the judge panel prefers over Haiku's.
- `REPORT.md` supports: splitting a rollout in half and concatenating two independent summaries
  degrades quality (duplication, contradictory halves — see the judges' reasoning throughout)
  enough to lose to Haiku even though the same model wins when given the whole conversation.
- The headline contradiction is therefore **chunking strategy + prompt**, not model quality:
  same model, different input handling. The chunked-concat pipeline in `evaluate_chunked.py`
  still uses the bullet prompt; feeding it the structured prompt is untested and is the obvious
  next experiment.
- Caveats common to both: 4 rollout fixtures (24 clustered verdicts, not 24 independent
  conversations), and judges score summaries without access to the source conversation, so
  they reward structure and detail; they cannot verify factual accuracy. N=24 per variant with
  judge disagreement means 21:3 vs 22:2 is within run-to-run noise, but both fresh and saved
  runs agree the direction is clearly Mercury-favoured for the structured prompt.
