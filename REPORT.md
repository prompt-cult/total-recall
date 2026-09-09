# Mercury 2.5 vs Claude Haiku 4.5: Summarization Comparison

## Methodology

Based on the pairwise blind evaluation pattern from
[gist b60a4d9af6d7789e70220fb3901ec9ea](https://gist.github.com/simbo1905/b60a4d9af6d7789e70220fb3901ec9ea).

- **Summarization models**: Claude Haiku 4.5 (via Zen Anthropic Messages API) vs Inception Mercury 2.5
- **Rollouts**: Small sessions from Vibe, OpenCode, Codex CLI, Claude Code
- **Judges**: 3 Zen models (glm-5.3-flash, gpt-5.4-mini, kimi-k3) across all 3 endpoint types
- **Orderings**: Each pair evaluated in both A-B and B-A to detect ordering bias
- **Scoring**: Each judge returns winner (A/B/tie), score_A (0-100), score_B (0-100), confidence, reasoning

## Overall Results

| Metric | Haiku | Mercury |
|--------|-------|---------|
| Wins | 7 | 2 |
| Ties | 0 | |
| Errors | 15 | |
| Avg Score | 32.0 | 29.5 |
| Total Evaluations | 24 | |

## Per-Rollout Results

| Rollout | Haiku Wins | Mercury Wins | Ties | Haiku Avg | Mercury Avg |
|---------|-----------|-------------|------|-----------|-------------|
| claude | 2 | 0 | 0 | 29.5 | 27.166666666666668 |
| codex | 1 | 2 | 0 | 40.166666666666664 | 42.166666666666664 |
| opencode | 2 | 0 | 0 | 29.333333333333332 | 24.5 |
| vibe | 2 | 0 | 0 | 29.0 | 24.0 |

## Per-Judge Results

| Judge | Haiku Wins | Mercury Wins | Ties | Errors |
|-------|-----------|-------------|------|--------|
| glm-5.3-flash | 0 | 0 | 0 | 8 |
| gpt-5.4-mini | 6 | 2 | 0 | 0 |
| kimi-k3 | 1 | 0 | 0 | 7 |

## Order Bias Check

Comparing A-B vs B-A results to detect ordering bias:

| Order | Haiku Wins | Mercury Wins | Ties |
|-------|-----------|-------------|------|
| A-B (A=Haiku) | 3 | 1 | 0 |
| B-A (B=Haiku) | 4 | 1 | 0 |

If the results are consistent across orderings, there is no ordering bias.
If A-B favors A and B-A favors A (regardless of model), there is an ordering bias.

## Individual Evaluations

### vibe | AB | glm-5.3-flash
- Winner: **error**
- Score A: 0 | Score B: 0
- Confidence: 0
- Reasoning: Failed to parse: 

### vibe | AB | gpt-5.4-mini
- Winner: **A**
- Score A: 88 | Score B: 72
- Confidence: 86
- Reasoning: A is more complete and accurate: it captures the full 3-phase evaluation workflow, the blind scoring/judge panel, the exact files/results involved, the evaluation rerun plan, and the discovered ordering bias. B is concise but misses important details like the phased process, specific next steps beyond commit/push, and the bias finding, and it is less precise about the evaluation setup.

### vibe | AB | kimi-k3
- Winner: **error**
- Score A: 0 | Score B: 0
- Confidence: 0
- Reasoning: HTTP Error 400: Bad Request

### vibe | BA | glm-5.3-flash
- Winner: **error**
- Score A: 0 | Score B: 0
- Confidence: 0
- Reasoning: Failed to parse: 

### vibe | BA | gpt-5.4-mini
- Winner: **B**
- Score A: 72 | Score B: 86
- Confidence: 84
- Reasoning: B is more complete and clearer: it captures the evaluation phases, specific files, the report-generation bug, the Zen model constraint, and the next steps. A is concise but omits important details like the 3-phase workflow, the cached outputs, and the more specific update/re-run status. B has some extra specifics that may be slightly over-detailed, but overall it better reflects the conversation summary requirements.

### vibe | BA | kimi-k3
- Winner: **error**
- Score A: 0 | Score B: 0
- Confidence: 0
- Reasoning: Failed to parse: Let me analyze both summaries carefully.

Summary A:
- Accomplished: Updated evaluate.py to use OpenCode Zen models, fixed report generation bugs, ran Mercury vs DeepSeek evaluation with pairwise judg

### opencode | AB | glm-5.3-flash
- Winner: **error**
- Score A: 0 | Score B: 0
- Confidence: 0
- Reasoning: Failed to parse: 

### opencode | AB | gpt-5.4-mini
- Winner: **A**
- Score A: 88 | Score B: 75
- Confidence: 86
- Reasoning: A is more complete and specific: it includes the session ID, project context, tool/database references, and several important constraints/decisions. B is clearer and concise, but omits some concrete details and is less thorough. Both appear accurate, but A better captures the full work state and next steps.

### opencode | AB | kimi-k3
- Winner: **error**
- Score A: 0 | Score B: 0
- Confidence: 0
- Reasoning: Failed to parse: Let me analyze both summaries against the criteria.

Summary A:
- Very detailed with structured sections
- Includes specific session ID, project path, database path
- Mentions 50 user messages, specif

### opencode | BA | glm-5.3-flash
- Winner: **error**
- Score A: 0 | Score B: 0
- Confidence: 0
- Reasoning: Failed to parse: 

### opencode | BA | gpt-5.4-mini
- Winner: **B**
- Score A: 72 | Score B: 88
- Confidence: 86
- Reasoning: B is more complete and clearer: it captures the key methodology details, the relevant session, the project, database, constraints, and next steps. A is concise but misses important specifics such as the 50 messages, 5 models/10 comparisons/3 judges/60 judgments, and the distinction between Codex CLI and opencode.

### opencode | BA | kimi-k3
- Winner: **error**
- Score A: 0 | Score B: 0
- Confidence: 0
- Reasoning: Failed to parse: {
  "winner": "B",
  "score_A": 74,
  "score_B": 88,
  "confidence": 72,
  "reasoning": "Both summaries cover

### codex | AB | glm-5.3-flash
- Winner: **error**
- Score A: 0 | Score B: 0
- Confidence: 0
- Reasoning: Failed to parse: 

### codex | AB | gpt-5.4-mini
- Winner: **B**
- Score A: 78 | Score B: 88
- Confidence: 86
- Reasoning: Both summaries capture the main outcome, but B is clearer and more concise. A adds some useful details, but it incorrectly says both games are complete and ready to run in the work-in-progress section and is slightly wordier. B better balances accuracy, completeness, and brevity, though it still has a minor path issue with './tmp/snake.py' instead of '/tmp/snake.py'.

### codex | AB | kimi-k3
- Winner: **error**
- Score A: 0 | Score B: 0
- Confidence: 0
- Reasoning: Failed to parse: {
  "winner": "A",
  "score_A": 88,
  "score_B": 80,
  "confidence": 70,
  "reasoning": "Both summaries accurately capture the core work: two single-file Python games (Snake, Space Invaders) with uv s

### codex | BA | glm-5.3-flash
- Winner: **error**
- Score A: 0 | Score B: 0
- Confidence: 0
- Reasoning: Failed to parse: 

### codex | BA | gpt-5.4-mini
- Winner: **A**
- Score A: 83 | Score B: 74
- Confidence: 77
- Reasoning: A is more concise and captures the key outcome, files, and execution approach well. B includes useful details but is slightly less accurate/clean because it overstates readiness, adds specific mechanics that may not be fully supported, and is less concise.

### codex | BA | kimi-k3
- Winner: **B**
- Score A: 82 | Score B: 89
- Confidence: 70
- Reasoning: Both summaries accurately capture the core work: two single-file Python games (Snake, Space Invaders) with uv shebangs and PEP 723 metadata. Summary B is more complete, including specific technical details (exact shebang line, class method structure, game-over restart via spacebar, arrow key controls) and has clearer formatting with distinct sections. Summary A is accurate and concise but slightly more generic, mentioning 'consistent class methods' without specifics. A does note the alternative 'uv run' execution method, which B omits, but overall B provides richer, more actionable detail.

### claude | AB | glm-5.3-flash
- Winner: **error**
- Score A: 0 | Score B: 0
- Confidence: 0
- Reasoning: Failed to parse: 

### claude | AB | gpt-5.4-mini
- Winner: **A**
- Score A: 88 | Score B: 80
- Confidence: 79
- Reasoning: A is slightly more complete and clearer: it captures the archived-via-git-tags constraint, mentions both spike15 and spike17 not existing locally, and includes the relevant next step. B is concise and accurate, but misses the explicit spike15/tag-history nuance and is less structured.

### claude | AB | kimi-k3
- Winner: **error**
- Score A: 0 | Score B: 0
- Confidence: 0
- Reasoning: Failed to parse: Let me analyze both summaries against the evaluation criteria.

Summary A:
- Accomplished: Located and extracted Whisper Large V3 spike files from git tag into .tmp/
- Confirmed spike15/spike17 folder

### claude | BA | glm-5.3-flash
- Winner: **error**
- Score A: 0 | Score B: 0
- Confidence: 0
- Reasoning: Failed to parse: 

### claude | BA | gpt-5.4-mini
- Winner: **B**
- Score A: 83 | Score B: 89
- Confidence: 82
- Reasoning: Both summaries are accurate and cover the main extracted files and next-step intent. B is slightly stronger because it is clearer and more organized, and it better reflects the archived-via-git-tags constraint. A is also good, but is a bit less concise and includes a less clearly grounded note about preserving pre-existing .tmp files.

### claude | BA | kimi-k3
- Winner: **error**
- Score A: 0 | Score B: 0
- Confidence: 0
- Reasoning: Failed to parse: Let me analyze both summaries against the evaluation criteria.

Summary A:
- Accomplished: Extracted files from git tag into .tmp
- Files: README.md, whisper-3-large.py, 0000_host.mp3
- Current Status
