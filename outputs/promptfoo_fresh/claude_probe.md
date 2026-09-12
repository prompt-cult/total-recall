

**FACTUAL RECALL**
- Model tags referenced: `20260702_121100_main_spike15` (Sample Speech Model, Large Variant), `20260702_120900_main_spike17` (Sample Speech Model on a Cloud Provider).
- Error: `Path not found: speech/spike15` when listing the folder in the current working tree.
- Specific tag inspected: `20260702_120900_main_spike17`.
- Files found in tag: `spike17/0000_host.mp3`, `spike17/README.md`, `spike17/whisper-3-large.py`.

**DECISIONS MADE**
- Decided to extract files from the `spike17` tag into a `.tmp` directory instead of using local folders, since `speech/spike15` does not exist in the current tree.
- Chosen to use the `spike17` config as hints for a different project, keeping the main working tree clean.

**ARTIFACT TRACKING**
- Created/Modified: `.tmp/README.md`, `.tmp/whisper-3-large.py`, `.tmp/0000_host.mp3` (extracted from commit `20260702_120900_main_spike17`).
- Note: Other files in `.tmp/` (e.g., `e2e-testing-is-a-stack.md`, `history.txt`, `J`, `.DS_Store`) existed prior and were not extracted from this tag.

**LOGICAL CONTINUATION**
- Next step: Inspect `.tmp/whisper-3-large.py` in the other project to derive configuration hints for the new speech model work.
- Optional follow-up: If needed, inspect `spike15` tag (`20260702_121100_main_spike15`) or cherry-pick its config into the target repo.