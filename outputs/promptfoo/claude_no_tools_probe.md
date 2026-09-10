

### Summary

**What the User Asked**
The user confirmed that `spike15` and `spike17` are archived Git tags, not local folders. They requested extraction of files from the `spike17` tag (specifically commit `20260702_120900_main_spike17`) into a `.tmp` directory to inspect them as hints for a different project, without altering the current working tree.

**What Was Accomplished**
*   **Extraction:** Files from tag `20260702_120900_main_spike17` were extracted into `.tmp/`.
*   **Files Retrieved:**
    *   `.tmp/README.md`
    *   `.tmp/whisper-3-large.py` (Whisper Large V3 config/script)
    *   `.tmp/0000_host.mp3` (test audio)
*   **Clarification:** Verified that other existing files in `.tmp/` (e.g., `e2e-testing-is-a-stack.md`, `history.txt`, `PNGs`) pre-dated this operation and were not part of the `spike17` tag.

**Factual Recall & Artifact Tracking**
*   **Git Tag:** `20260702_120900_main_spike17`
*   **Target Directory:** `.tmp/`
*   **Key File:** `.tmp/whisper-3-large.py`
*   **Original Paths (in tag):** `spike17/README.md`, `spike17/whisper-3-large.py`, `spike17/0000_host.mp3`

**Next Steps**
The user intends to use `.tmp/whisper-3-large.py` and associated files as references for their other project. Future work may involve cherry-picking relevant Whisper/Vosk configurations from this history into the target repository.