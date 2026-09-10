

*   **Accomplished:** Confirmed `spike15` and `spike17` tags exist in the repo but local folders are missing; identified specific tags to reference for Whisper/Vosk configs.
*   **Current Work:** Extracting files from tag `20260702_120900_main_spike17` into `.tmp` directory for inspection.
*   **Files Involved:** All files present in the `spike17/*` commit corresponding to the specified tag.
*   **Next Steps:** User will inspect extracted files to use as hints for a different project; potential to cherry-pick configs later.
*   **Key Decisions:** Use top-level README and tags instead of local folders; avoid moving files permanently (use `.tmp`).



- **Accomplished:** Extracted 3 specific files from the `spike17` git tag (`0000_host.mp3`, `README.md`, `whisper-3-large.py`) into `.tmp/`.
- **Current Work:** Isolating the correct files from the commit to avoid confusion with pre-existing files in the working directory.
- **Files Involved:** `.tmp/0000_host.mp3`, `.tmp/README.md`, `.tmp/whisper-3-large.py`.
- **Next Steps:** Use the `whisper-3-large.py` script and `0000_host.mp3` as hints for further implementation or testing.
- **Key Decision:** Clearly distinguished new extracted files from legacy files (PNGs, logs, spike01/02) already present in the directory.