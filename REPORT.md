# Mercury 2.5 vs Claude Haiku 4.5: Summarization Comparison

## Methodology

Based on the pairwise blind evaluation pattern from
[gist b60a4d9af6d7789e70220fb3901ec9ea](https://gist.github.com/simbo1905/b60a4d9af6d7789e70220fb3901ec9ea).

- **Summarization models**: Mistral Small (via Mistral API) vs Inception Mercury 2.5
- **Rollouts**: Small sessions from Vibe, OpenCode, Codex CLI, Claude Code
- **Judges**: 3 Mistral models (mistral-large, ministral-8b, magistral-small)
- **Note**: OpenCode Go API was returning 500 errors, so Mistral used as fallback for both summarization comparison and judge panel
- **Orderings**: Each pair evaluated in both A-B and B-A to detect ordering bias
- **Scoring**: Each judge returns winner (A/B/tie), score_A (0-100), score_B (0-100), confidence, reasoning

## Overall Results

| Metric | Haiku | Mercury |
|--------|-------|---------|
| Wins | 8 | 9 |
| Ties | 3 | |
| Errors | 4 | |
| Avg Score | 74.7 | 76.0 |
| Total Evaluations | 24 | |

## Per-Rollout Results

| Rollout | Haiku Wins | Mercury Wins | Ties | Haiku Avg | Mercury Avg |
|---------|-----------|-------------|------|-----------|-------------|
| claude | 2 | 2 | 1 | 76.66666666666667 | 76.66666666666667 |
| codex | 2 | 3 | 0 | 73.33333333333333 | 76.66666666666667 |
| opencode | 2 | 2 | 1 | 73.33333333333333 | 74.16666666666667 |
| vibe | 2 | 2 | 1 | 75.5 | 76.66666666666667 |

## Per-Judge Results

| Judge | Haiku Wins | Mercury Wins | Ties | Errors |
|-------|-----------|-------------|------|--------|
| magistral-small | 4 | 4 | 0 | 0 |
| ministral-8b | 0 | 1 | 3 | 4 |
| mistral-large | 4 | 4 | 0 | 0 |

## Order Bias Check

Comparing A-B vs B-A results to detect ordering bias:

| Order | Haiku Wins | Mercury Wins | Ties |
|-------|-----------|-------------|------|
| A-B (A=Haiku) | 8 | 0 | 0 |
| B-A (B=Haiku) | 0 | 9 | 3 |

If the results are consistent across orderings, there is no ordering bias.
If A-B favors A and B-A favors A (regardless of model), there is an ordering bias.

## Individual Evaluations

### vibe | AB | mistral-large
- Winner: **A**
- Score A: 95 | Score B: 90
- Confidence: 90
- Reasoning: Both summaries are strong, but Summary A edges out slightly due to better clarity and structure. It explicitly lists the number of messages extracted for each source, which adds precision, and separates accomplishments, current work, files, next steps, and key constraints/decisions into distinct sections, making it easier to scan. Summary B is also accurate and complete but is slightly less concise and mixes some details (e.g., 'increased message extraction from 0 to 21' is less precise than stating the final count). Both summaries cover all key points, but Summary A's organization and specificity give it a slight advantage.

### vibe | AB | ministral-8b
- Winner: **error**
- Score A: 0 | Score B: 0
- Confidence: 0
- Reasoning: Failed to parse: ```json
{
  "winner": "tie",
  "score_A": 95,
  "score_B": 95,
  "confidence": 98,
  "reasoning": {
    "accuracy": {
      "A": 100,
      "B": 100,
      "note": "Both summaries accurately reflect t

### vibe | AB | magistral-small
- Winner: **A**
- Score A: 95 | Score B: 85
- Confidence: 90
- Reasoning: Summary A is more detailed and structured, providing specific numbers (e.g., 200 messages for Vibe, 21 for OpenCode) and clearer breakdowns of accomplished work, current WIP, and key constraints. Summary B is concise but lacks some granularity (e.g., exact message counts for Vibe and Claude) and mixes some points (e.g., 'increased message extraction' is less precise than 'successfully extracted'). Both are clear and complete, but A edges out due to its thoroughness and organization.

### vibe | BA | mistral-large
- Winner: **A**
- Score A: 95 | Score B: 85
- Confidence: 90
- Reasoning: Both summaries are accurate and clear, but Summary A is more complete and concise. Summary A provides specific details about the iterative fixes and the increase in message extraction (e.g., '0 to 21 for OpenCode' and '3 to 8 for Codex'), which adds valuable context. It also explicitly mentions the handling of nested data structures and the decision to adapt extraction logic for provider-specific schemas, which are critical details. Summary B, while accurate, omits some of these specifics and includes slightly less relevant details (e.g., exact message counts for all providers, which may not be as critical as the iterative improvements). Both summaries are well-structured, but Summary A edges out in completeness and conciseness.

### vibe | BA | ministral-8b
- Winner: **tie**
- Score A: 95 | Score B: 93
- Confidence: 95
- Reasoning: {'accuracy': {'A': 98, 'B': 95, 'note': 'Both summaries accurately reflect the key accomplishments (e.g., message extraction improvements for OpenCode and Codex). Summary B slightly overstates Claude’s extracted messages (7 vs. 8 in A) and omits the initial 0/3 baseline for OpenCode/Codex, though the intent is clear.'}, 'completeness': {'A': 98, 'B': 90, 'note': 'Summary A includes all critical details (e.g., nested data structures, session files, and provider-specific schema decisions). Summary B omits the *why* behind key decisions (e.g., adapting to schema differences) and lacks specifics on session file locations.'}, 'conciseness': {'A': 95, 'B': 90, 'note': "Summary A is more concise by grouping related points (e.g., 'Key Decisions') and avoiding redundancy. Summary B repeats the script path and includes less critical details (e.g., Claude’s message count)."}, 'clarity': {'A': 98, 'B': 95, 'note': "Summary A’s structure (bullet points, clear labels) improves readability. Summary B’s phrasing (e.g., 'Key Constraints/Decisions') is slightly less intuitive, though still understandable."}, 'overall': {'note': 'Both summaries are strong, but **A** edges out due to its precision, completeness of technical context, and tighter organization. **B** is slightly less detailed in explaining *how* decisions were made and includes minor inaccuracies (e.g., Claude’s message count).'}}

### vibe | BA | magistral-small
- Winner: **A**
- Score A: 95 | Score B: 85
- Confidence: 90
- Reasoning: Summary A is more accurate and complete, capturing the iterative process of fixing the script (e.g., increasing message extraction counts) and explicitly mentioning schema parsing logic adjustments. Summary B provides concrete message counts but misses some key details like the initial zero extraction counts and the provider-specific schema differences (e.g., SQLite vs. JSONL). Both summaries are clear and concise, but A's additional context gives it an edge.

### opencode | AB | mistral-large
- Winner: **A**
- Score A: 95 | Score B: 85
- Confidence: 90
- Reasoning: Summary A is more accurate and complete, capturing all key details such as the number of models, comparisons, judges, and the exact next steps (no further action needed). It is also concise and clear, directly stating the accomplishments and constraints without ambiguity. Summary B, while accurate, is less complete as it frames the work as ongoing ('Current Work') and does not explicitly state the final outcome (reconstruction of the methodology). It also lacks some specifics (e.g., 5 models, 10 pairwise comparisons, 60 judgments) and is slightly less concise due to its procedural tone.

### opencode | AB | ministral-8b
- Winner: **error**
- Score A: 0 | Score B: 0
- Confidence: 0
- Reasoning: Failed to parse: ```json
{
  "winner": "A",
  "score_A": 95,
  "score_B": 75,
  "confidence": 95,
  "reasoning": "
  **Accuracy & Completeness**:
  - **Summary A** correctly captures the **verbatim methodology** (5 mo

### opencode | AB | magistral-small
- Winner: **A**
- Score A: 95 | Score B: 85
- Confidence: 90
- Reasoning: Summary A is more accurate and complete, capturing all key details (verbatim methodology, file paths, constraints, and next steps) concisely. Summary B is clear but less concise, includes redundant phrasing (e.g., 'user messages about the TurboFieldfare project'), and omits the explicit count of models/judgments. Both summaries are accurate, but A's brevity and precision give it the edge.

### opencode | BA | mistral-large
- Winner: **A**
- Score A: 95 | Score B: 85
- Confidence: 90
- Reasoning: Summary A is more comprehensive and detailed in capturing the current work and next steps, which are critical for understanding the ongoing process. It also explicitly mentions the constraints and the nature of the task (read-only research). Summary B, while accurate and concise, omits the ongoing extraction process and jumps directly to the outcome, which may not fully reflect the conversation's progression. Both summaries are clear and easy to understand, but A provides a more complete picture of the workflow and next actions. However, B is more concise and directly states the key findings, which is valuable. The scores reflect A's slight edge in completeness and accuracy of the current state.

### opencode | BA | ministral-8b
- Winner: **tie**
- Score A: 85 | Score B: 85
- Confidence: 95
- Reasoning: {'accuracy': {'A': 90, 'B': 90, 'note': 'Both summaries accurately reflect the core details of the conversation, though **Summary B** incorrectly claims the methodology was *already recovered* (a claim not supported by the context—only extraction/analysis was completed). **Summary A** is more precise in stating the task is ongoing.'}, 'completeness': {'A': 95, 'B': 85, 'note': '**Summary A** includes all key points (files, constraints, next steps) explicitly. **Summary B** omits critical details like the *read-only* constraint and the *ongoing extraction* of verbatim quotes, instead overstating completion.'}, 'conciseness': {'A': 95, 'B': 80, 'note': "**Summary A** is concise and structured. **Summary B** is slightly verbose (e.g., redundant phrasing like 'recovered the user's verbatim...') and misleads by implying full recovery."}, 'clarity': {'A': 95, 'B': 85, 'note': "**Summary A** is unambiguous. **Summary B**’s phrasing ('recovered the user's exact specification') risks misinterpretation as a completed task rather than an analytical step."}, 'overall': {'note': 'Both summaries are strong but **Summary A** avoids inaccuracies and maintains precision. **Summary B**’s overstatement of progress and minor omissions (e.g., constraints) slightly undermine its reliability.'}}

### opencode | BA | magistral-small
- Winner: **A**
- Score A: 95 | Score B: 80
- Confidence: 90
- Reasoning: Summary A is more accurate and complete, capturing the current work (extracting and analyzing user messages) and constraints (read-only task) that Summary B omits. Summary B incorrectly states that the methodology was 'recovered verbatim' and omits the ongoing extraction work, making it less accurate. Both summaries are clear and concise, but A provides a more faithful representation of the conversation.

### codex | AB | mistral-large
- Winner: **A**
- Score A: 95 | Score B: 90
- Confidence: 90
- Reasoning: Both summaries are strong, but Summary A edges out slightly due to better clarity and structure. Summary A explicitly separates accomplishments, files involved, and next steps, making it easier to scan. It also correctly notes the subagent's role in replicating the structure for Space Invaders, which Summary B omits. Summary B is more concise but sacrifices some completeness (e.g., missing the subagent detail) and includes minor redundancies (e.g., 'Current Work' overlaps with 'Accomplished'). Both are accurate, but Summary A's organization and slight edge in completeness give it the win.

### codex | AB | ministral-8b
- Winner: **error**
- Score A: 0 | Score B: 0
- Confidence: 0
- Reasoning: Failed to parse: ```json
{
  "winner": "B",
  "score_A": 75,
  "score_B": 95,
  "confidence": 90,
  "reasoning": {
    "accuracy": {
      "A": 85,
      "B": 95,
      "_note": "B correctly includes additional detail

### codex | AB | magistral-small
- Winner: **A**
- Score A: 95 | Score B: 85
- Confidence: 90
- Reasoning: Summary A is more accurate (correctly notes subagent usage for Space Invaders), more concise (avoids redundant details like FPS), and clearer in structure. Summary B includes unnecessary details (FPS, 'classic mechanics') and slightly misrepresents constraints ('must mirror Snake's code structure' is less precise than 'replicate the Snake game’s structure'). Both are complete, but A's brevity and precision give it the edge.

### codex | BA | mistral-large
- Winner: **A**
- Score A: 95 | Score B: 85
- Confidence: 90
- Reasoning: Summary A is more comprehensive and detailed, covering all key points including the FPS specifications, the identical structure of both games, and the constraints explicitly mentioned in the conversation. It is also concise and clear. Summary B is accurate but misses some details like the FPS specifications, the completeness of both games, and the specific constraints, making it slightly less complete. Both summaries are clear and concise, but A edges out B in completeness and accuracy.

### codex | BA | ministral-8b
- Winner: **A**
- Score A: 95 | Score B: 85
- Confidence: 95
- Reasoning: {'accuracy': {'A': 100, 'B': 90, 'note': 'Summary A correctly specifies the FPS (10 FPS for Snake, 60 FPS for Space Invaders) and explicitly states both games are complete and ready to execute. Summary B omits FPS details and is less precise about execution readiness.'}, 'completeness': {'A': 100, 'B': 80, 'note': 'Summary A includes all key constraints (single executable scripts, shebang/env setup, Space Invaders mirroring Snake’s structure) and explicitly mentions no further development is requested. Summary B omits constraints and next-step execution specifics.'}, 'conciseness': {'A': 95, 'B': 80, 'note': "Summary A is more concise and avoids redundant phrasing (e.g., 'subagent' in B is unnecessary). Summary B includes less critical details (e.g., 'subagent') and is slightly verbose."}, 'clarity': {'A': 100, 'B': 90, 'note': "Summary A is clear, structured, and easy to follow. Summary B’s phrasing (e.g., 'Spawned a subagent') is less professional and slightly confusing."}}

### codex | BA | magistral-small
- Winner: **A**
- Score A: 95 | Score B: 80
- Confidence: 90
- Reasoning: Summary A is more accurate (correctly notes both games are complete and ready to execute), more complete (includes FPS details and constraints), and more concise. Summary B inaccurately states the Space Invaders game was 'spawned' via a subagent and omits key details like FPS and constraints. Both are clear, but A's additional precision gives it the edge.

### claude | AB | mistral-large
- Winner: **A**
- Score A: 95 | Score B: 90
- Confidence: 90
- Reasoning: Both summaries are accurate, complete, concise, and clear. However, Summary A is slightly more concise and structured, making it easier to quickly grasp the key points. Summary B includes a bit more detail (e.g., 'Current Work in Progress' and explicit mention of not overwriting existing `.tmp` contents), but this additional information does not significantly improve clarity or completeness. Summary A's phrasing is more direct and avoids redundancy, giving it a slight edge.

### claude | AB | ministral-8b
- Winner: **error**
- Score A: 0 | Score B: 0
- Confidence: 0
- Reasoning: Failed to parse: ```json
{
  "winner": "tie",
  "score_A": 95,
  "score_B": 95,
  "confidence": 98,
  "reasoning": "
Both summaries are highly accurate, complete, concise, and clear. They effectively capture:
- **Accu

### claude | AB | magistral-small
- Winner: **A**
- Score A: 95 | Score B: 85
- Confidence: 90
- Reasoning: Summary A is more concise and clearer in specifying the exact filenames and their purposes (e.g., 'Whisper Large V3 config/script'). It also explicitly mentions the unrelated legacy content in `.tmp/`, which Summary B omits. Summary B is accurate and complete but slightly less concise and lacks the explicit mention of legacy content, making it marginally less clear.

### claude | BA | mistral-large
- Winner: **A**
- Score A: 95 | Score B: 90
- Confidence: 90
- Reasoning: Both summaries are accurate, complete, concise, and clear. However, Summary A provides slightly more detail about the current work in progress and explicitly mentions that existing `.tmp` contents were not overwritten, which adds clarity. Summary B is also strong but omits the non-overwrite detail and is marginally less structured in its presentation of the 'next steps' and 'key constraints'. Both are high-quality, but Summary A edges out due to these minor advantages.

### claude | BA | ministral-8b
- Winner: **tie**
- Score A: 95 | Score B: 95
- Confidence: 98
- Reasoning: {'accuracy': {'A': 100, 'B': 100, 'note': 'Both summaries accurately reflect the extracted files, commit tag, and purpose of the extraction.'}, 'completeness': {'A': 95, 'B': 95, 'note': 'Both summaries cover all key points (accomplished work, files, next steps, constraints). Minor redundancy in **Files Involved** in B (e.g., `.tmp/` prefix is unnecessary since context already specifies `.tmp` directory).'}, 'conciseness': {'A': 95, 'B': 90, 'note': "A is slightly more concise by avoiding redundant `.tmp/` prefix in file paths. B’s phrasing (e.g., 'Whisper Large V3 config/script') is slightly verbose."}, 'clarity': {'A': 95, 'B': 95, 'note': 'Both are clear, but A’s structure (bullet points) is marginally more straightforward for quick scanning.'}, 'overall': {'note': 'Both summaries are nearly identical in quality. The tie reflects minor stylistic differences (e.g., redundancy in B’s file paths, slight verbosity in B’s descriptions) rather than substantive errors.'}}

### claude | BA | magistral-small
- Winner: **A**
- Score A: 95 | Score B: 85
- Confidence: 90
- Reasoning: Summary A is more complete, covering all key points including 'Current Work in Progress' and 'Key Constraints' which Summary B omits. Summary B is slightly more concise but lacks some details (e.g., explicit mention of existing `.tmp` contents not being overwritten). Both are clear and accurate, but A's completeness gives it the edge.
