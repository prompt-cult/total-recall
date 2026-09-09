# Summary

## Accomplished
- **Completed full 3-phase Mercury vs Haiku/alternative model evaluation**: extracted rollouts from 4 CLI tools (Vibe, OpenCode, Codex, Claude) → generated summaries with both models → ran pairwise A-B/B-A blind scoring with judge panel → aggregated results into `results.json` and `REPORT.md`
- **Discovered and diagnosed OpenCode Zen API issue**: Claude, GPT, Gemini, Muse models all return 500; only DeepSeek, GLM, Kimi, MiniMax work
- **Fixed report generation bug** in `evaluate.py` (multiple arguments to `lines.append()`)

## Current Work in Progress
- **Re-running evaluation** with working Zen models: using `deepseek-v4-flash` as the summarization comparison baseline instead of Haiku, and `glm-5.3-flash`, `deepseek-v4-flash`, `kimi-k3` as the judge panel (replacing Mistral)

## Files Involved
- `/Users/Shared/inception-mercury-compaction/evaluate.py` — main evaluation script (just updated with working Zen models)
- `/Users/Shared/inception-mercury-compaction/extract_rollouts.py` — rollout extraction
- `results.json`, `REPORT.md` — results (will be regenerated)
- `outputs/summaries/*` and `outputs/pairwise/*` — cached intermediate results

## Next Steps
1. Clear old results and cached summaries
2. Run `evaluate.py` with new Zen model configuration
3. Commit and push updated results

## Key Constraints/Decisions
- **Zen API outage for most models**: only 8 of 25 listed models actually work (500 errors on Claude/GPT/Gemini/Muse); using working models instead
- **Ordering bias confirmed in previous run**: A-B always favors position A regardless of model, indicates judge bias not model difference