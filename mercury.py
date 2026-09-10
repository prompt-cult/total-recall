#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Send a prompt + data file to Inception Mercury 2.5.

Usage:
  mercury.py <prompt_file> <data_file>

The prompt file is the user message template (e.g. the structured prompt).
The data file is appended to the prompt as the conversation to summarise.
The system message is a fixed "summarize conversations" instruction.
API key is read from the INCEPTION_API_KEY environment variable.

Example:
  export INCEPTION_API_KEY=sk_...
  ./mercury.py structured_prompt.txt rollout.jsonl
"""
from __future__ import annotations

import json
import os
import sys
import urllib.request


API_URL = "https://api.inceptionlabs.ai/v1/chat/completions"
MODEL = "mercury-2.5"
SYSTEM_PROMPT = "You are a helpful coding assistant that summarizes conversations."


def call_mercury(system_prompt: str, user_content: str, api_key: str) -> str:
    payload = {
        "model": MODEL,
        "messages": [
            {"role": "system", "content": system_prompt},
            {"role": "user", "content": user_content},
        ],
        "temperature": 0.1,
        "max_tokens": 4000,
        "reasoning_effort": "low",
    }
    req = urllib.request.Request(
        API_URL,
        data=json.dumps(payload).encode(),
        headers={
            "Authorization": f"Bearer {api_key}",
            "Content-Type": "application/json",
        },
    )
    resp = urllib.request.urlopen(req, timeout=120)
    result = json.loads(resp.read())
    return result["choices"][0]["message"]["content"]


def main() -> None:
    if len(sys.argv) != 3:
        print("Usage: mercury.py <prompt_file> <data_file>", file=sys.stderr)
        sys.exit(1)

    prompt_path = sys.argv[1]
    data_path = sys.argv[2]

    api_key = os.environ.get("INCEPTION_API_KEY")
    if not api_key:
        print("Error: INCEPTION_API_KEY not set in environment", file=sys.stderr)
        sys.exit(1)

    with open(prompt_path) as f:
        prompt_template = f.read()

    with open(data_path) as f:
        data_content = f.read()

    user_content = prompt_template + data_content
    result = call_mercury(SYSTEM_PROMPT, user_content, api_key)
    print(result)


if __name__ == "__main__":
    main()
