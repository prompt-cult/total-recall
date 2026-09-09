#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Extract small rollouts from vibe, opencode, codex, claude into a common JSONL format.

Each output line is a JSON object with:
  {"role": "user|assistant|tool", "content": "...", "tool_calls": [...]}

Tool calls are summarized to one-line descriptions, not raw file contents.
"""

from __future__ import annotations

import json
import os
import sqlite3
import sys
from pathlib import Path

# --- Vibe ---
def extract_vibe(session_id: str, out_path: Path) -> int:
    """Extract a Vibe session using the miner script output."""
    import subprocess
    result = subprocess.run(
        ["uv", "run", os.path.expanduser("~/.vibe/skills/mistral-vibe-chat-history/miner.py"),
         "--session", session_id],
        capture_output=True, text=True, timeout=30
    )
    if result.returncode != 0:
        print(f"  Vibe: miner failed: {result.stderr[:200]}", file=sys.stderr)
        return 0
    count = 0
    with open(out_path, "w") as f:
        for line in result.stdout.strip().splitlines():
            if not line.strip():
                continue
            try:
                msg = json.loads(line)
                role = msg.get("role", "")
                content = msg.get("content", "")
                tool_calls = msg.get("tool_calls", [])
                if role in ("user", "assistant", "tool") and (content or tool_calls):
                    # Summarize tool calls
                    if tool_calls:
                        summarized = []
                        for tc in tool_calls:
                            fn = tc.get("function", {})
                            name = fn.get("name", "?")
                            args_str = fn.get("arguments", "{}")
                            try:
                                args = json.loads(args_str)
                            except (json.JSONDecodeError, TypeError):
                                args = {}
                            if name == "bash":
                                summarized.append(f"bash: {args.get('command', '')[:200]}")
                            elif name in ("write_file", "edit"):
                                fp = args.get("file_path", "?")
                                if name == "write_file":
                                    summarized.append(f"write_file({fp}, {len(args.get('content', ''))} chars)")
                                else:
                                    summarized.append(f"edit({fp})")
                            elif name == "read_file":
                                summarized.append(f"read_file({args.get('file_path', '?')})")
                            elif name == "grep":
                                summarized.append(f"grep({args.get('pattern', '?')})")
                            elif name == "task":
                                summarized.append(f"task({args.get('agent', '?')})")
                            elif name == "todo":
                                summarized.append(f"todo({args.get('action', '?')})")
                            elif name == "skill":
                                summarized.append(f"skill({args.get('name', '?')})")
                            else:
                                summarized.append(f"{name}(...)")
                        msg["tool_calls_summary"] = summarized
                        msg.pop("tool_calls", None)
                    # Truncate large content
                    if content and len(str(content)) > 2000:
                        msg["content"] = str(content)[:2000] + "... (truncated)"
                    f.write(json.dumps(msg) + "\n")
                    count += 1
            except json.JSONDecodeError:
                continue
    return count


# --- OpenCode ---
def extract_opencode(session_id: str, out_path: Path) -> int:
    """Extract an OpenCode session from SQLite. Message metadata in message.data,
    actual content in the part table."""
    db_path = Path.home() / ".local" / "share" / "opencode" / "opencode.db"
    conn = sqlite3.connect(str(db_path))
    conn.row_factory = sqlite3.Row
    cursor = conn.cursor()

    # Get messages with their parts
    cursor.execute(
        "SELECT m.id, m.data, p.data as part_data "
        "FROM message m LEFT JOIN part p ON p.message_id = m.id "
        "WHERE m.session_id = ? ORDER BY m.id ASC, p.time_created ASC",
        (session_id,)
    )
    rows = cursor.fetchall()
    conn.close()

    # Group parts by message
    messages = {}
    for row in rows:
        msg_id = row["id"]
        if msg_id not in messages:
            try:
                msg_data = json.loads(row["data"])
            except (json.JSONDecodeError, TypeError):
                msg_data = {}
            messages[msg_id] = {
                "role": msg_data.get("role", "unknown"),
                "parts": []
            }
        if row["part_data"]:
            try:
                part = json.loads(row["part_data"])
                messages[msg_id]["parts"].append(part)
            except (json.JSONDecodeError, TypeError):
                pass

    count = 0
    with open(out_path, "w") as f:
        for msg_id, info in messages.items():
            role = info["role"]
            # Build content from parts
            parts = []
            for part in info["parts"]:
                ptype = part.get("type", "")
                if ptype == "text":
                    parts.append(part.get("text", ""))
                elif ptype == "tool":
                    parts.append(f"tool: {part.get('name', '?')}")
                elif ptype == "tool-result":
                    parts.append(f"tool_result: {str(part.get('content', ''))[:500]}")
                else:
                    parts.append(json.dumps(part)[:300])
            content = " ".join(parts)
            if len(content) > 2000:
                content = content[:2000] + "... (truncated)"
            if role in ("user", "assistant", "tool") and content:
                f.write(json.dumps({"role": role, "content": content}) + "\n")
                count += 1
    return count


# --- Codex ---
def extract_codex(jsonl_path: Path, out_path: Path) -> int:
    """Extract a Codex CLI session from JSONL. Uses type+payload format."""
    count = 0
    with open(out_path, "w") as f:
        for line in jsonl_path.read_text(encoding="utf-8").splitlines():
            if not line.strip():
                continue
            try:
                msg = json.loads(line)
            except json.JSONDecodeError:
                continue
            msg_type = msg.get("type", "")
            if msg_type != "response_item":
                continue
            payload = msg.get("payload", {})
            ptype = payload.get("type", "")
            role = "unknown"
            content = ""
            if ptype == "message":
                role = payload.get("role", "unknown")
                if role == "developer":
                    continue  # skip system/developer messages
                raw_content = payload.get("content", [])
                if isinstance(raw_content, list):
                    parts = []
                    for c in raw_content:
                        if isinstance(c, dict):
                            if c.get("type") == "input_text":
                                parts.append(c.get("text", ""))
                            elif c.get("type") == "output_text":
                                parts.append(c.get("text", ""))
                            elif c.get("type") == "text":
                                parts.append(c.get("text", ""))
                        else:
                            parts.append(str(c))
                    content = " ".join(parts)
                else:
                    content = str(raw_content)
            elif ptype == "function_call":
                role = "assistant"
                name = payload.get("name", "?")
                args = payload.get("arguments", "")
                if isinstance(args, str) and len(args) > 200:
                    args = args[:200] + "..."
                content = f"tool_call: {name}({args})"
            elif ptype == "function_call_output":
                role = "tool"
                output = payload.get("output", "")
                content = str(output)[:1000]
            else:
                continue
            if role in ("user", "assistant", "tool") and content:
                if len(content) > 2000:
                    content = content[:2000] + "... (truncated)"
                f.write(json.dumps({"role": role, "content": content}) + "\n")
                count += 1
    return count


# --- Claude ---
def extract_claude(jsonl_path: Path, out_path: Path) -> int:
    """Extract a Claude Code session from JSONL. Uses type field with message.content."""
    count = 0
    with open(out_path, "w") as f:
        for line in jsonl_path.read_text(encoding="utf-8").splitlines():
            if not line.strip():
                continue
            try:
                msg = json.loads(line)
            except json.JSONDecodeError:
                continue
            msg_type = msg.get("type", "")
            if msg_type not in ("user", "assistant"):
                continue
            message = msg.get("message", {})
            role = message.get("role", msg_type)
            content = message.get("content", "")
            if isinstance(content, list):
                parts = []
                for c in content:
                    if isinstance(c, dict):
                        if c.get("type") == "text":
                            parts.append(c.get("text", ""))
                        elif c.get("type") == "tool_use":
                            parts.append(f"tool_call: {c.get('name', '?')}")
                        elif c.get("type") == "tool_result":
                            parts.append(f"tool_result: {str(c.get('content', ''))[:500]}")
                    else:
                        parts.append(str(c))
                content = " ".join(parts)
            content = str(content)
            # Skip meta/command messages
            if content.startswith("<local-command-caveat>") or content.startswith("<command-name>"):
                continue
            if role in ("user", "assistant", "tool") and content:
                if len(content) > 2000:
                    content = content[:2000] + "... (truncated)"
                f.write(json.dumps({"role": role, "content": content}) + "\n")
                count += 1
    return count


# --- Main ---
def main() -> None:
    out_dir = Path(__file__).parent / "rollouts"
    out_dir.mkdir(exist_ok=True)

    # 1. Vibe: session 4836855e (149 messages, small enough)
    print("Extracting Vibe session 4836855e...", file=sys.stderr)
    count = extract_vibe("4836855e", out_dir / "vibe_small.jsonl")
    print(f"  -> {count} messages", file=sys.stderr)

    # 2. OpenCode: session with 21 messages
    print("Extracting OpenCode session ses_02d8c49b4ffeKA7Jw6U5YJl0Pn...", file=sys.stderr)
    count = extract_opencode("ses_02d8c49b4ffeKA7Jw6U5YJl0Pn", out_dir / "opencode_small.jsonl")
    print(f"  -> {count} messages", file=sys.stderr)

    # 3. Codex: session with 37 lines, 14 response_items
    codex_path = Path.home() / ".codex/sessions/2026/04/28/rollout-2026-04-28T11-04-07-019dd38b-cd17-74d1-b52f-cc1806cb4a49.jsonl"
    print(f"Extracting Codex session {codex_path.name}...", file=sys.stderr)
    count = extract_codex(codex_path, out_dir / "codex_small.jsonl")
    print(f"  -> {count} messages", file=sys.stderr)

    # 4. Claude: small session with enough content
    claude_path = Path.home() / ".claude/projects/-Users-Shared-stenography/22e81356-28ec-4ac9-8d9e-9fd3f746f1b7.jsonl"
    print(f"Extracting Claude session {claude_path.name}...", file=sys.stderr)
    count = extract_claude(claude_path, out_dir / "claude_small.jsonl")
    print(f"  -> {count} messages", file=sys.stderr)

    print("\nDone. Rollouts in:", out_dir, file=sys.stderr)


if __name__ == "__main__":
    main()
