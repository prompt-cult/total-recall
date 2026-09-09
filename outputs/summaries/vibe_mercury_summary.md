

- **Accomplished:** Created and iteratively fixed `extract_rollouts.py` to extract rollouts from Vibe, OpenCode, Codex, and Claude into a unified JSONL format; successfully increased message extraction from 0 to 21 for OpenCode and 3 to 8 for Codex by correcting schema parsing logic.
- **Current WIP:** Finalizing the extraction script to handle all four providers robustly, ensuring proper handling of nested data structures (e.g., OpenCode `part` table, Codex `response_item` payloads).
- **Files Involved:** `/Users/Shared/inception-mercury-compaction/extract_rollouts.py` (main script), various session files in `~/.vibe`, `~/.opencode`, `~/.codex`, and `~/.claude`.
- **Next Steps:** Run the finalized script to generate complete rollouts, verify output consistency across providers, and potentially automate session selection for larger datasets.
- **Key Decisions:** Adapted extraction logic to account for provider-specific schema differences (SQLite vs. JSONL, nested content fields) rather than enforcing a single rigid format.