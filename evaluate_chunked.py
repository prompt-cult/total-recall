#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Mercury chunked vs Haiku full summarization comparison with timings.

1. Summarize each rollout with Haiku (full chunk) — record time
2. Split each rollout in half, summarize each half with Mercury — record time
3. Concatenate the two Mercury half-summaries
4. A-B/B-A pairwise scoring: Haiku-full vs Mercury-concatenated
5. Report timings and quality

Judges: glm-5.3-flash, gpt-5.4-mini, kimi-k3 (all via Zen, max_tokens=2000)
"""

from __future__ import annotations

import json
import os
import re
import sys
import time
import urllib.request
from pathlib import Path

REPO = Path(__file__).parent
ROLLOUTS = REPO / "rollouts"
OUT = REPO / "outputs"
SUMMARY_DIR = OUT / "summaries"
PAIRWISE_DIR = OUT / "pairwise"
TIMINGS_DIR = OUT / "timings"

SUMMARY_PROMPT = """\
You are a coding assistant. Summarize this conversation focusing on:
- What was accomplished
- Current work in progress
- Files involved
- Next steps
- Key constraints or decisions

Be concise (3-5 bullet points max).

--- Conversation ---
{conversation}
"""

JUDGE_PROMPT = """\
You are an impartial judge evaluating two conversation summaries (A and B).
You do NOT know which model produced which summary.

Evaluate based on:
1. Accuracy - Correctly captures key information from the conversation
2. Completeness - Includes all important points (accomplished work, files, next steps)
3. Conciseness - Brief and to the point
4. Clarity - Easy to understand

Return ONLY a JSON object with these fields:
{{
  "winner": "A" | "B" | "tie",
  "score_A": 0-100,
  "score_B": 0-100,
  "confidence": 0-100,
  "reasoning": "brief explanation"
}}

---
Summary A:
{summary_A}

---
Summary B:
{summary_B}
"""

ZEN_BASE = "https://opencode.ai/zen/v1"
MERCURY_BASE = "https://api.inceptionlabs.ai/v1"

ROLLOUT_NAMES = ["vibe", "opencode", "codex", "claude"]


def load_env() -> dict[str, str]:
    env = dict(os.environ)
    env_file = REPO / ".env"
    if env_file.exists():
        for line in env_file.read_text().splitlines():
            if "=" in line and not line.startswith("#"):
                k, v = line.split("=", 1)
                env[k.strip()] = v.strip()
    return env


def call_chat(api_base, model, api_key, system, user, max_tokens=2000, timeout=120):
    payload = {"model": model, "messages": [{"role": "system", "content": system}, {"role": "user", "content": user}], "temperature": 0.1, "max_tokens": max_tokens}
    if "mercury" in model:
        payload["reasoning_effort"] = "low"
    req = urllib.request.Request(f"{api_base}/chat/completions", data=json.dumps(payload).encode(),
        headers={"Authorization": f"Bearer {api_key}", "Content-Type": "application/json", "User-Agent": "inception-mercury-compaction/1.0"})
    resp = urllib.request.urlopen(req, timeout=timeout)
    return json.loads(resp.read())["choices"][0]["message"]["content"]

def call_messages(api_base, model, api_key, system, user, max_tokens=2000):
    payload = {"model": model, "max_tokens": max_tokens, "system": system, "messages": [{"role": "user", "content": user}]}
    req = urllib.request.Request(f"{api_base}/messages", data=json.dumps(payload).encode(),
        headers={"x-api-key": api_key, "Content-Type": "application/json", "anthropic-version": "2023-06-01", "User-Agent": "inception-mercury-compaction/1.0"})
    resp = urllib.request.urlopen(req, timeout=120)
    result = json.loads(resp.read())
    content = result.get("content", [])
    if isinstance(content, list):
        return " ".join(b.get("text", "") for b in content if b.get("type") == "text")
    return str(content)

def call_responses(api_base, model, api_key, system, user, max_tokens=2000):
    payload = {"model": model, "instructions": system, "input": user, "max_output_tokens": max_tokens}
    req = urllib.request.Request(f"{api_base}/responses", data=json.dumps(payload).encode(),
        headers={"Authorization": f"Bearer {api_key}", "Content-Type": "application/json", "User-Agent": "inception-mercury-compaction/1.0"})
    resp = urllib.request.urlopen(req, timeout=120)
    result = json.loads(resp.read())
    for item in result.get("output", []):
        if item.get("type") == "message":
            content = item.get("content", [])
            if isinstance(content, list):
                return " ".join(c.get("text", "") for c in content if c.get("type") == "output_text")
    return str(result.get("output", ""))

def call_judge(judge_key, api_key, system, user):
    if judge_key == "glm-5.3-flash":
        return call_chat(ZEN_BASE, "glm-5.3-flash", api_key, system, user, max_tokens=16000, timeout=300)
    elif judge_key == "gpt-5.4-mini":
        return call_responses(ZEN_BASE, "gpt-5.4-mini", api_key, system, user, max_tokens=2000)
    elif judge_key == "kimi-k3":
        return call_chat(ZEN_BASE, "kimi-k3", api_key, system, user, max_tokens=2000)
    return ""

def rollout_to_text(jsonl_path):
    lines = []
    for line in jsonl_path.read_text().splitlines():
        if not line.strip(): continue
        try: msg = json.loads(line)
        except: continue
        role = msg.get("role", "?").upper()
        content = msg.get("content", "")
        tcs = msg.get("tool_calls_summary", [])
        if tcs:
            for tc in tcs: lines.append(f"  [{role} -> {tc}]")
        if content:
            lines.append(f"[{role}]")
            lines.append(str(content)[:1500])
            if len(str(content)) > 1500: lines.append("... (truncated)")
            lines.append("")
    return "\n".join(lines)

def split_messages(jsonl_path):
    """Split a rollout JSONL into two halves."""
    msgs = []
    for line in jsonl_path.read_text().splitlines():
        if not line.strip(): continue
        try: msgs.append(json.loads(line))
        except: continue
    mid = len(msgs) // 2
    return msgs[:mid], msgs[mid:]

def msgs_to_text(msgs):
    lines = []
    for msg in msgs:
        role = msg.get("role", "?").upper()
        content = msg.get("content", "")
        tcs = msg.get("tool_calls_summary", [])
        if tcs:
            for tc in tcs: lines.append(f"  [{role} -> {tc}]")
        if content:
            lines.append(f"[{role}]")
            lines.append(str(content)[:1500])
            if len(str(content)) > 1500: lines.append("... (truncated)")
            lines.append("")
    return "\n".join(lines)

def extract_json_object(text):
    """Extract the first parseable JSON object via brace matching, skipping braces inside strings.

    Tries each '{' candidate until one yields a balanced, parseable object, so prose
    containing brace literals before the JSON does not break extraction.
    """
    pos = text.find("{")
    while pos >= 0:
        depth = 0
        in_string = False
        escape = False
        end = -1
        for i in range(pos, len(text)):
            c = text[i]
            if in_string:
                if escape:
                    escape = False
                elif c == "\\":
                    escape = True
                elif c == '"':
                    in_string = False
            else:
                if c == '"':
                    in_string = True
                elif c == "{":
                    depth += 1
                elif c == "}":
                    depth -= 1
                    if depth == 0:
                        end = i
                        break
        if end < 0:
            raise ValueError("unbalanced JSON object (truncated reply)")
        candidate = text[pos:end+1]
        try:
            return json.loads(candidate)
        except json.JSONDecodeError:
            try:
                return json.loads(re.sub(r",\s*([}\]])", r"\1", candidate))
            except json.JSONDecodeError:
                pos = text.find("{", pos + 1)
    raise ValueError("no parseable JSON object found")

def parse_json_response(response):
    try:
        cleaned = response.strip()
        if cleaned.startswith("```"):
            cleaned = re.sub(r"^```[A-Za-z0-9_-]*[ \t]*\n?", "", cleaned)
            cleaned = re.sub(r"```[ \t]*\n?", "", cleaned)
            cleaned = cleaned.strip()
        obj = extract_json_object(cleaned)
        if not isinstance(obj, dict):
            raise ValueError("not a JSON object")
        return obj
    except (ValueError, json.JSONDecodeError, AttributeError):
        return {"winner": "error", "score_A": 0, "score_B": 0, "confidence": 0, "reasoning": f"Failed to parse: {response[:300]}"}

JUDGES = ["glm-5.3-flash", "gpt-5.4-mini", "kimi-k3"]

def main():
    env = load_env()
    zen_key = env.get("OPENCODE_API_KEY", "")
    mercury_key = env.get("INCEPTION_API_KEY", "")
    SUMMARY_DIR.mkdir(parents=True, exist_ok=True)
    PAIRWISE_DIR.mkdir(parents=True, exist_ok=True)
    TIMINGS_DIR.mkdir(parents=True, exist_ok=True)

    timings = {}

    print("=== Phase 1: Haiku full-chunk summarization ===", file=sys.stderr)
    for name in ROLLOUT_NAMES:
        rollout_path = ROLLOUTS / f"{name}_small.jsonl"
        if not rollout_path.exists(): continue
        conversation = rollout_to_text(rollout_path)
        out_path = SUMMARY_DIR / f"{name}_haiku_full.md"
        if out_path.exists():
            print(f"  {name}: cached", file=sys.stderr)
            continue
        print(f"  {name}: {len(conversation)} chars -> Haiku...", file=sys.stderr)
        t0 = time.time()
        summary = call_messages(ZEN_BASE, "claude-haiku-4-5", zen_key,
            "You are a helpful coding assistant that summarizes conversations.",
            SUMMARY_PROMPT.format(conversation=conversation), max_tokens=1000)
        elapsed = time.time() - t0
        out_path.write_text(summary)
        timings[f"{name}_haiku_full"] = {"time_s": round(elapsed, 2), "chars": len(summary), "input_chars": len(conversation)}
        print(f"    {len(summary)} chars in {elapsed:.2f}s", file=sys.stderr)

    print("\n=== Phase 2: Mercury half-chunk summarization ===", file=sys.stderr)
    for name in ROLLOUT_NAMES:
        rollout_path = ROLLOUTS / f"{name}_small.jsonl"
        if not rollout_path.exists(): continue
        half1, half2 = split_messages(rollout_path)
        text1 = msgs_to_text(half1)
        text2 = msgs_to_text(half2)
        out_path = SUMMARY_DIR / f"{name}_mercury_concat.md"
        if out_path.exists():
            print(f"  {name}: cached", file=sys.stderr)
            continue
        print(f"  {name}: half1={len(text1)} chars, half2={len(text2)} chars -> Mercury x2...", file=sys.stderr)
        t0 = time.time()
        s1 = call_chat(MERCURY_BASE, "mercury-2.5", mercury_key,
            "You are a helpful coding assistant that summarizes conversations.",
            SUMMARY_PROMPT.format(conversation=text1), max_tokens=1000)
        t1 = time.time()
        s2 = call_chat(MERCURY_BASE, "mercury-2.5", mercury_key,
            "You are a helpful coding assistant that summarizes conversations.",
            SUMMARY_PROMPT.format(conversation=text2), max_tokens=1000)
        t2 = time.time()
        concat = s1 + "\n\n" + s2
        out_path.write_text(concat)
        total = t2 - t0
        timings[f"{name}_mercury_half1"] = {"time_s": round(t1 - t0, 2), "chars": len(s1), "input_chars": len(text1)}
        timings[f"{name}_mercury_half2"] = {"time_s": round(t2 - t1, 2), "chars": len(s2), "input_chars": len(text2)}
        timings[f"{name}_mercury_total"] = {"time_s": round(total, 2), "chars": len(concat)}
        print(f"    half1: {len(s1)} chars in {t1-t0:.2f}s, half2: {len(s2)} chars in {t2-t1:.2f}s, total: {total:.2f}s", file=sys.stderr)

    # Save timings
    (TIMINGS_DIR / "timings.json").write_text(json.dumps(timings, indent=2))

    print("\n=== Phase 3: Pairwise A-B/B-A scoring ===", file=sys.stderr)
    results = []
    for name in ROLLOUT_NAMES:
        haiku_path = SUMMARY_DIR / f"{name}_haiku_full.md"
        mercury_path = SUMMARY_DIR / f"{name}_mercury_concat.md"
        if not haiku_path.exists() or not mercury_path.exists(): continue
        haiku_summary = haiku_path.read_text()
        mercury_summary = mercury_path.read_text()
        if haiku_summary.startswith("ERROR") or mercury_summary.startswith("ERROR"): continue

        for order_label, sA, sB in [("AB", haiku_summary, mercury_summary), ("BA", mercury_summary, haiku_summary)]:
            for judge in JUDGES:
                out_file = PAIRWISE_DIR / f"{name}_{order_label}_{judge}.json"
                if out_file.exists():
                    try: results.append(json.loads(out_file.read_text()))
                    except: pass
                    print(f"    {name} {order_label} {judge}: cached", file=sys.stderr)
                    continue
                print(f"    {name} {order_label} {judge}: judging...", file=sys.stderr)
                try:
                    response = call_judge(judge, zen_key, "You are an impartial judge. Return ONLY valid JSON.",
                        JUDGE_PROMPT.format(summary_A=sA, summary_B=sB))
                    verdict = parse_json_response(response)
                    if verdict.get("winner") == "error":
                        print(f"      -> parse failed, retrying once with strict instruction", file=sys.stderr)
                        retry_response = call_judge(judge, zen_key, "You are an impartial judge. Return ONLY valid JSON.",
                            JUDGE_PROMPT.format(summary_A=sA, summary_B=sB) + "\n\nReturn ONLY the JSON object, no other text.")
                        retry_verdict = parse_json_response(retry_response)
                        if retry_verdict.get("winner") != "error":
                            verdict = retry_verdict
                            print(f"      -> retry succeeded", file=sys.stderr)
                        else:
                            print(f"      -> retry also failed to parse", file=sys.stderr)
                except Exception as e:
                    verdict = {"winner": "error", "score_A": 0, "score_B": 0, "confidence": 0, "reasoning": str(e)}
                result = {"rollout": name, "order": order_label, "judge": judge, "verdict": verdict}
                out_file.write_text(json.dumps(result, indent=2))
                results.append(result)
                print(f"      -> {verdict.get('winner', '?')}", file=sys.stderr)
                time.sleep(0.5)

    print("\n=== Phase 4: Aggregate ===", file=sys.stderr)
    # Aggregate
    haiku_wins = mercury_wins = ties = errors = 0
    haiku_scores = []
    mercury_scores = []
    for r in results:
        v = r["verdict"]
        w = v.get("winner", "error")
        sA = v.get("score_A", 0)
        sB = v.get("score_B", 0)
        if r["order"] == "AB":
            hs, ms = sA, sB
            if w == "A": haiku_wins += 1
            elif w == "B": mercury_wins += 1
            elif w == "tie": ties += 1
            else: errors += 1
        else:
            hs, ms = sB, sA
            if w == "B": haiku_wins += 1
            elif w == "A": mercury_wins += 1
            elif w == "tie": ties += 1
            else: errors += 1
        haiku_scores.append(hs)
        mercury_scores.append(ms)

    agg = {
        "total": len(results),
        "haiku_wins": haiku_wins, "mercury_wins": mercury_wins, "ties": ties, "errors": errors,
        "haiku_avg": round(sum(haiku_scores)/len(haiku_scores), 1) if haiku_scores else 0,
        "mercury_avg": round(sum(mercury_scores)/len(mercury_scores), 1) if mercury_scores else 0,
        "timings": timings,
    }
    (REPO / "results.json").write_text(json.dumps(agg, indent=2))

    # Print timing comparison
    print("\n=== TIMINGS ===", file=sys.stderr)
    print(f"{'Rollout':<12} {'Haiku(s)':<10} {'Merc-1(s)':<10} {'Merc-2(s)':<10} {'Merc-Tot':<10} {'Speedup':<10}", file=sys.stderr)
    for name in ROLLOUT_NAMES:
        ht = timings.get(f"{name}_haiku_full", {}).get("time_s", 0)
        m1 = timings.get(f"{name}_mercury_half1", {}).get("time_s", 0)
        m2 = timings.get(f"{name}_mercury_half2", {}).get("time_s", 0)
        mt = timings.get(f"{name}_mercury_total", {}).get("time_s", 0)
        speedup = f"{ht/mt:.1f}x" if mt > 0 else "N/A"
        print(f"{name:<12} {ht:<10} {m1:<10} {m2:<10} {mt:<10} {speedup:<10}", file=sys.stderr)

    print(f"\n=== RESULTS ===", file=sys.stderr)
    print(f"Haiku wins: {haiku_wins}, Mercury wins: {mercury_wins}, Ties: {ties}, Errors: {errors}", file=sys.stderr)
    print(f"Haiku avg: {agg['haiku_avg']}, Mercury avg: {agg['mercury_avg']}", file=sys.stderr)

    # Generate report
    lines = [
        "# Mercury Chunked vs Haiku Full: Summarization Comparison",
        "",
        "## Methodology",
        "",
        "- **Haiku**: Summarizes the full rollout in one call (Anthropic Messages API via Zen)",
        "- **Mercury**: Splits rollout in half, summarizes each half separately, concatenates",
        "- **Judges**: glm-5.3-flash, gpt-5.4-mini, kimi-k3 (all via Zen, max_tokens=2000)",
        "- **Orderings**: A-B and B-A to detect ordering bias",
        "",
        "## Timings",
        "",
        "| Rollout | Haiku (s) | Mercury Half 1 (s) | Mercury Half 2 (s) | Mercury Total (s) | Speedup |",
        "|---------|-----------|--------------------|--------------------|-------------------|---------|",
    ]
    for name in ROLLOUT_NAMES:
        ht = timings.get(f"{name}_haiku_full", {}).get("time_s", 0)
        m1 = timings.get(f"{name}_mercury_half1", {}).get("time_s", 0)
        m2 = timings.get(f"{name}_mercury_half2", {}).get("time_s", 0)
        mt = timings.get(f"{name}_mercury_total", {}).get("time_s", 0)
        speedup = f"{ht/mt:.1f}x" if mt > 0 else "N/A"
        lines.append(f"| {name} | {ht} | {m1} | {m2} | {mt} | {speedup} |")

    lines.extend([
        "",
        "## Results",
        "",
        f"| Metric | Haiku (full) | Mercury (concat) |",
        f"|--------|-------------|-----------------|",
        f"| Wins | {haiku_wins} | {mercury_wins} |",
        f"| Ties | {ties} | |",
        f"| Errors | {errors} | |",
        f"| Avg Score | {agg['haiku_avg']} | {agg['mercury_avg']} |",
        "",
        "## Individual Evaluations",
        "",
    ])
    for r in results:
        v = r["verdict"]
        lines.append(f"### {r['rollout']} | {r['order']} | {r['judge']}")
        lines.append(f"- Winner: **{v.get('winner', '?')}**")
        lines.append(f"- Score A: {v.get('score_A', 0)} | Score B: {v.get('score_B', 0)}")
        lines.append(f"- Confidence: {v.get('confidence', 0)}")
        lines.append(f"- Reasoning: {v.get('reasoning', 'N/A')[:200]}")
        lines.append("")

    (REPO / "REPORT.md").write_text("\n".join(lines))
    print("Report saved to REPORT.md", file=sys.stderr)


if __name__ == "__main__":
    main()
