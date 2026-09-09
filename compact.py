#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Universal context compaction for CLI coding agents.

Detects the session format (JSONL or SQLite) for Mistral Vibe, OpenCode,
Codex CLI, Claude Code, or Cursor. Finds the last compaction point, extracts
messages since then, prunes large tool outputs, and sends to Inception Mercury
2.5 for summarization.

Usage:
  compact.py --tool vibe                          Compact current Vibe session
  compact.py --tool vibe --session 7b00dfb9       Compact a specific session
  compact.py --tool opencode                       Compact latest OpenCode session
  compact.py --tool vibe --dry-run                 Show what would be compacted
  compact.py --tool vibe --model mercury-2         Use a different Mercury model
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import urllib.request
from datetime import datetime, timezone
from pathlib import Path

# --- Tool definitions ---

TOOLS = {
    "vibe": {
        "storage": "jsonl",
        "root": Path.home() / ".vibe" / "logs" / "session",
        "pattern": "session_*",
        "messages_file": "messages.jsonl",
        "meta_file": "meta.json",
    },
    "opencode": {
        "storage": "sqlite",
        "root": Path.home() / ".local" / "share" / "opencode",
        "db_file": "opencode.db",
    },
    "codex": {
        "storage": "jsonl",
        "root": Path.home() / ".codex" / "sessions",
        "pattern": "*",
        "messages_file": "messages.jsonl",
    },
    "claude": {
        "storage": "jsonl",
        "root": Path.home() / ".claude" / "projects",
        "pattern": "*",
        "messages_file": "messages.jsonl",
    },
    "cursor": {
        "storage": "sqlite",
        "root": Path.home() / ".cursor" / "workspace",
        "db_file": "cursor.db",
    },
}

# Compaction prompt — synthesizes the best of Claude Code, Codex CLI, and OpenCode
COMPACTION_SYSTEM_PROMPT = """\
You are performing a CONTEXT CHECKPOINT COMPACTION. Create a handoff summary \
for another LLM that will resume the task.

Include:
- Current progress and key decisions made
- Important context, constraints, or user preferences
- What remains to be done (clear next steps)
- Any critical data, examples, or references needed to continue
- Files that were modified or created
- Key errors or blockers encountered

Be concise, structured, and focused on helping the next LLM seamlessly \
continue the work. Use bullet points and sections. Do not include tool \
outputs or raw file contents — just summarize what they revealed.\
"""

COMPACTION_PREFIX = """\
Another language model started to solve this problem and produced a summary \
of its thinking process. You also have access to the state of the tools that \
were used by that language model. Use this to build on the work that has \
already been done and avoid duplicating work. Here is the summary produced by \
the other language model:\
"""

# Prune tool outputs larger than this many characters
PRUNE_TOOL_OUTPUT_CHARS = 1000
# Preserve the most recent N messages verbatim alongside the summary
PRESERVE_RECENT_MESSAGES = 10


def load_env() -> str:
    """Load INCEPTION_API_KEY from env or .env file."""
    key = os.environ.get("INCEPTION_API_KEY")
    if key:
        return key
    for p in [Path.cwd()] + list(Path.cwd().parents):
        env_file = p / ".env"
        if env_file.exists():
            for line in env_file.read_text().splitlines():
                if line.startswith("INCEPTION_API_KEY="):
                    key = line.split("=", 1)[1].strip()
                    if key:
                        return key
    print("Error: INCEPTION_API_KEY not found in env or .env file", file=sys.stderr)
    sys.exit(1)


# --- JSONL session handling ---

def find_jsonl_sessions(tool_config: dict) -> list[Path]:
    """Find all session directories for a JSONL-based tool."""
    root = tool_config["root"]
    if not root.exists():
        return []
    pattern = tool_config.get("pattern", "*")
    sessions = []
    for entry in root.glob(pattern):
        if entry.is_dir() and (entry / tool_config["messages_file"]).exists():
            sessions.append(entry)
    sessions.sort(key=lambda p: p.stat().st_mtime, reverse=True)
    return sessions


def load_jsonl_messages(session_dir: Path, messages_file: str = "messages.jsonl") -> list[dict]:
    """Load all messages from a JSONL session."""
    path = session_dir / messages_file
    if not path.exists():
        return []
    msgs = []
    for line in path.read_text(encoding="utf-8").splitlines():
        if not line.strip():
            continue
        try:
            msgs.append(json.loads(line))
        except json.JSONDecodeError:
            continue
    return msgs


def find_last_compaction_jsonl(messages: list[dict]) -> int:
    """Find the index of the last compaction/summary message.
    Returns the index after the compaction, or 0 if none found."""
    for i in range(len(messages) - 1, -1, -1):
        msg = messages[i]
        content = msg.get("content", "")
        if not content:
            continue
        content_str = str(content)
        # Check for compaction markers used by different tools
        markers = [
            "You are continuing a trajectory after a context compaction",
            "Another language model started to solve this problem",
            "Here is the summary produced by the other language model",
            "<compaction>",
            "## Compaction Summary",
            "Context Compaction",
        ]
        for marker in markers:
            if marker in content_str:
                return i + 1
    return 0


def extract_messages_for_compaction(messages: list[dict], since_index: int) -> list[dict]:
    """Extract messages since the last compaction point, pruning large tool outputs."""
    extracted = []
    for msg in messages[since_index:]:
        role = msg.get("role", "unknown")
        content = msg.get("content", "")

        # Prune large tool outputs
        if role == "tool" and content and len(str(content)) > PRUNE_TOOL_OUTPUT_CHARS:
            pruned = str(content)[:PRUNE_TOOL_OUTPUT_CHARS] + "\n... (pruned for compaction)"
            msg = dict(msg)
            msg["content"] = pruned

        # Skip empty messages
        if not content and not msg.get("tool_calls"):
            continue

        extracted.append(msg)
    return extracted


def _summarize_tool_call(name: str, args_str: str) -> str:
    """Produce a compact one-line summary of a tool call."""
    try:
        args = json.loads(args_str)
    except (json.JSONDecodeError, TypeError):
        return f"{name}({str(args_str)[:200]})"
    if name == "bash":
        cmd = args.get("command", "")
        return f"bash: {cmd[:300]}"
    if name in ("write_file", "edit"):
        fp = args.get("file_path", "?")
        if name == "write_file":
            content = args.get("content", "")
            return f"write_file({fp}, {len(content)} chars)"
        old = args.get("old_string", "")
        return f"edit({fp}, {len(old)} chars replaced)"
    if name == "read_file":
        return f"read_file({args.get('file_path', '?')})"
    if name == "grep":
        return f"grep({args.get('pattern', '?')}, {args.get('path', '.')})"
    if name == "task":
        return f"task({args.get('agent', '?')})"
    if name == "todo":
        return f"todo({args.get('action', '?')})"
    if name == "skill":
        return f"skill({args.get('name', '?')})"
    return f"{name}({json.dumps(args)[:300]})"


def messages_to_text(messages: list[dict]) -> str:
    """Convert messages to a compact text representation for the LLM.

    Tool calls and results are summarized, not dumped verbatim — Mercury needs
    conversational context, not raw file contents.
    """
    lines = []
    for msg in messages:
        role = msg.get("role", "unknown").upper()
        content = msg.get("content", "")

        if msg.get("tool_calls"):
            for tc in msg["tool_calls"]:
                fn = tc.get("function", {})
                name = fn.get("name", "?")
                args = fn.get("arguments", "")
                lines.append(f"  {role} -> {_summarize_tool_call(name, args)}")

        if content:
            content_str = str(content)
            if role == "TOOL":
                lines.append(f"  TOOL RESULT: {content_str[:500]}")
                if len(content_str) > 500:
                    lines.append("  ... (truncated)")
            else:
                lines.append(f"[{role}]")
                lines.append(content_str[:1500])
                if len(content_str) > 1500:
                    lines.append("... (truncated)")
            lines.append("")

    return "\n".join(lines)


# --- SQLite session handling ---

def load_sqlite_messages(db_path: Path) -> list[dict]:
    """Load messages from a SQLite database (OpenCode, Cursor)."""
    import sqlite3
    conn = sqlite3.connect(str(db_path))
    conn.row_factory = sqlite3.Row
    cursor = conn.cursor()

    # Try common schemas
    msgs = []
    try:
        cursor.execute("SELECT * FROM messages ORDER BY id ASC")
        for row in cursor.fetchall():
            msg = dict(row)
            msgs.append(msg)
    except sqlite3.OperationalError:
        try:
            cursor.execute("SELECT * FROM chat ORDER BY id ASC")
            for row in cursor.fetchall():
                msg = dict(row)
                msgs.append(msg)
        except sqlite3.OperationalError:
            pass

    conn.close()
    return msgs


# --- Mercury API ---

def compact_with_mercury(text: str, api_key: str, model: str = "mercury-2.5") -> str:
    """Send text to Inception Mercury for compaction."""
    payload = {
        "model": model,
        "messages": [
            {"role": "system", "content": COMPACTION_SYSTEM_PROMPT},
            {"role": "user", "content": text},
        ],
        "temperature": 0.1,
        "max_tokens": 4000,
        "reasoning_effort": "low",
    }

    req = urllib.request.Request(
        "https://api.inceptionlabs.ai/v1/chat/completions",
        data=json.dumps(payload).encode(),
        headers={
            "Authorization": f"Bearer {api_key}",
            "Content-Type": "application/json",
        },
    )

    resp = urllib.request.urlopen(req, timeout=120)
    result = json.loads(resp.read())
    return result["choices"][0]["message"]["content"]


# --- Main ---

def main() -> None:
    parser = argparse.ArgumentParser(
        description="Universal context compaction for CLI coding agents using Inception Mercury.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument(
        "--tool", choices=list(TOOLS.keys()), required=True,
        help="Which CLI tool's session to compact"
    )
    parser.add_argument(
        "--session", type=str, default=None,
        help="Session ID (partial match). Defaults to most recent session."
    )
    parser.add_argument(
        "--model", type=str, default="mercury-2.5",
        help="Mercury model to use (default: mercury-2.5)"
    )
    parser.add_argument(
        "--dry-run", action="store_true",
        help="Show what would be compacted without calling the API"
    )
    parser.add_argument(
        "--output", type=str, default=None,
        help="Write compaction summary to this file (default: stdout)"
    )

    args = parser.parse_args()
    tool_config = TOOLS[args.tool]
    api_key = load_env()

    # Find session
    if tool_config["storage"] == "jsonl":
        sessions = find_jsonl_sessions(tool_config)
        if not sessions:
            print(f"Error: No sessions found in {tool_config['root']}", file=sys.stderr)
            sys.exit(1)

        if args.session:
            matching = [s for s in sessions if args.session in s.name]
            if not matching:
                print(f"Error: No session matching '{args.session}'", file=sys.stderr)
                sys.exit(1)
            session_dir = matching[0]
        else:
            session_dir = sessions[0]

        print(f"Session: {session_dir.name}", file=sys.stderr)
        messages = load_jsonl_messages(session_dir, tool_config["messages_file"])

    elif tool_config["storage"] == "sqlite":
        db_path = tool_config["root"] / tool_config["db_file"]
        if not db_path.exists():
            print(f"Error: Database not found at {db_path}", file=sys.stderr)
            sys.exit(1)
        print(f"Database: {db_path}", file=sys.stderr)
        messages = load_sqlite_messages(db_path)
    else:
        print(f"Error: Unknown storage type for tool '{args.tool}'", file=sys.stderr)
        sys.exit(1)

    if not messages:
        print("Error: No messages found in session", file=sys.stderr)
        sys.exit(1)

    print(f"Total messages: {len(messages)}", file=sys.stderr)

    # Find last compaction point
    if tool_config["storage"] == "jsonl":
        compaction_idx = find_last_compaction_jsonl(messages)
    else:
        compaction_idx = 0  # SQLite: no compaction markers yet

    since_count = len(messages) - compaction_idx
    print(f"Last compaction point: message {compaction_idx}", file=sys.stderr)
    print(f"Messages since compaction: {since_count}", file=sys.stderr)

    if since_count == 0:
        print("Nothing to compact — already at compaction point.", file=sys.stderr)
        sys.exit(0)

    # Extract and prepare messages
    to_compact = extract_messages_for_compaction(messages, compaction_idx)
    print(f"Messages to compact (after pruning): {len(to_compact)}", file=sys.stderr)

    # Preserve recent messages verbatim
    recent = to_compact[-PRESERVE_RECENT_MESSAGES:]
    to_summarize = to_compact[:-PRESERVE_RECENT_MESSAGES] if len(to_compact) > PRESERVE_RECENT_MESSAGES else []

    if not to_summarize:
        print("Not enough messages to warrant compaction.", file=sys.stderr)
        sys.exit(0)

    text = messages_to_text(to_summarize)
    print(f"Text to summarize: {len(text)} chars", file=sys.stderr)

    if args.dry_run:
        print("\n--- DRY RUN: would send to Mercury ---\n", file=sys.stderr)
        print(text[:2000])
        if len(text) > 2000:
            print(f"... ({len(text)} chars total)")
        print(f"\n--- Would preserve {len(recent)} recent messages verbatim ---", file=sys.stderr)
        sys.exit(0)

    # Compact with Mercury
    print(f"Compacting with {args.model}...", file=sys.stderr)
    summary = compact_with_mercury(text, api_key, args.model)
    print(f"Summary: {len(summary)} chars", file=sys.stderr)

    # Build output: compaction prefix + summary + recent messages
    output_parts = [
        f"# Context Compaction — {datetime.now(timezone.utc).strftime('%Y-%m-%dT%H:%M:%SZ')}",
        f"# Model: {args.model}",
        f"# Session: {session_dir.name if tool_config['storage'] == 'jsonl' else 'sqlite'}",
        "",
        COMPACTION_PREFIX,
        "",
        summary,
        "",
        f"## Recent messages (preserved verbatim, last {len(recent)}):",
        "",
        messages_to_text(recent),
    ]

    output = "\n".join(output_parts)

    if args.output:
        Path(args.output).write_text(output)
        print(f"Written to {args.output}", file=sys.stderr)
    else:
        print(output)


if __name__ == "__main__":
    main()
