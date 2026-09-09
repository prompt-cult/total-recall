

- **Accomplished:** Updated `evaluate.py` to use OpenCode Zen models (`deepseek-v4-flash` and GLM/Kimi) instead of Mistral, fixed report generation bugs, and successfully ran the Mercury vs. DeepSeek summarization evaluation with pairwise judging.
- **Files Involved:** `evaluate.py`, `extract_rollouts.py`, `REPORT.md`, `results.json`, and pairwise output JSONs in `outputs/pairwise/`.
- **Key Constraints:** OpenCode Zen returns 500 errors for Claude, Gemini, and GPT models; only DeepSeek, GLM, Kimi, and MiniMax models are functional for summarization and judging.
- **Next Steps:** Commit and push the updated evaluation results and script changes to the repository.
- **Decision:** Replaced Mistral fallback with working Zen models to align with user instructions while maintaining the Mercury comparison.