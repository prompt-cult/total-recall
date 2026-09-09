### Summary

- **Accomplished**:
  - Successfully extracted rollouts from Vibe (200 messages), OpenCode (21 messages), Codex (14 messages), and Claude (7 messages) into a common JSONL format.
  - Debugged and fixed extraction logic for OpenCode (now reads from `part` table), Codex (handles `response_item` messages), and Claude (correctly parses `user` messages).

- **Current Work in Progress**:
  - Finalizing extraction script (`extract_rollouts.py`) to handle all tool formats correctly.

- **Files Involved**:
  - `/Users/Shared/inception-mercury-compaction/extract_rollouts.py` (updated multiple times to fix extraction logic).

- **Next Steps**:
  - Rerun the extraction script with the corrected logic to ensure all sessions are properly processed.
  - Validate the output JSONL files for consistency and correctness.

- **Key Constraints/Decisions**:
  - OpenCode: Content is stored in the `part` table, not the `message.data` column.
  - Codex: Messages are in `response_item` entries with `type: "message"` and roles like `user`, `assistant`, or `developer`.
  - Claude: User messages are under `type: "user"` with nested `message.content`.