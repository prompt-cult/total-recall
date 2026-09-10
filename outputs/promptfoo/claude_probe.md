

### Summary of Conversation

**1. Factual Recall**
- **Spikes Referenced:**
  - `spike15`: Whisper Large V3 Speech-to-Text. Tag: `20260702_121100_main_spike15`. Status: ✅ Tagged and archived.
  - `spike17`: Whisper Large V3 on Scaleway. Tag: `20260702_120900_main_spike17`. Status: ✅ Tagged and archived.
- **Error Encountered:** `Path not found: stenography/spike15` (no local folder exists).
- **Requested Action:** Extract files from tag `20260702_120900_main_spike17` into `.tmp` directory for inspection/hints.
- **Extracted Files from Tag:**
  - `0000_host.mp3`
  - `README.md`
  - `whisper-3-large.py`

**2. Decisions Made**
- **Decision:** Use the top-level README entry and tag for `spike15`/`spike17` instead of local folders, as local directories do not exist in the current working tree.
- **Decision:** Extract `spike17` files into `.tmp` rather than a dedicated spike folder to facilitate use as hints in a different project.

**3. Artifact Tracking**
- **Files Extracted into `.tmp/`:**
  - `.tmp/0000_host.mp3`
  - `.tmp/README.md`
  - `.tmp/whisper-3-large.py`
- **Note:** Other files present in `.tmp/` (e.g., `e2e-testing-is-a-stack.md`, `history.txt`, `spike01`/`spike02` contents) pre-existed and were not part of the `spike17` tag extraction.

**4. Logical Continuation**
- **Next Step:** Review `.tmp/whisper-3-large.py` and `.tmp/README.md` to extract Whisper Large V3 configuration and implementation hints for integration into the new project.