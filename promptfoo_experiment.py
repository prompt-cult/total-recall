#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Promptfoo experiment: test multiple Mercury prompt variants for summarization quality.

Variants:
1. baseline: current simple prompt
2. structured: explicit sections (accomplished, WIP, files, next steps, constraints)
3. no_tools: strip tool calls, only user+assistant messages
4. two_pass: first pass minimal bullets, second pass refine with full context
5. probe: Inception's own approach — preserve factual recall, decisions, files, continuation
6. reasoning_medium: same as baseline but reasoning_effort=medium
7. no_tools_probe: strip tools + probe-style prompt
"""

from __future__ import annotations

import json
import os
import sys
import time
import urllib.request
from pathlib import Path

REPO = Path(__file__).parent
ROLLOUTS = REPO / "rollouts"
OUT = REPO / "outputs" / "promptfoo"
MERCURY_BASE = "https://api.inceptionlabs.ai/v1"
ZEN_BASE = "https://opencode.ai/zen/v1"
ROLLOUT_NAMES = ["vibe", "opencode", "codex", "claude"]

PROMPTS = {
    "baseline": {
        "system": "You are a helpful coding assistant that summarizes conversations.",
        "user": """Summarize this conversation focusing on:
- What was accomplished
- Current work in progress
- Files involved
- Next steps
- Key constraints or decisions

Be concise (3-5 bullet points max).

--- Conversation ---
{conversation}""",
        "reasoning_effort": "low",
    },
    "structured": {
        "system": "You are a helpful coding assistant that summarizes conversations.",
        "user": """Create a structured summary of this coding conversation. Use these exact sections:

## Accomplished
List what was completed.

## Current Work
What is being worked on now.

## Files Involved
List all files mentioned or modified.

## Next Steps
Clear actions to take.

## Key Decisions/Constraints
Important user preferences, project requirements, or decisions made.

Be precise. Include file paths, function names, and specific details.

--- Conversation ---
{conversation}""",
        "reasoning_effort": "low",
    },
    "no_tools": {
        "system": "You are a helpful coding assistant that summarizes conversations.",
        "user": """Summarize this conversation between a user and a coding assistant. Focus only on what the user asked for and what the assistant accomplished. Ignore tool execution details.

Include:
- What was accomplished
- Current work in progress
- Files involved
- Next steps
- Key constraints or decisions

Be concise (3-5 bullet points max).

--- Conversation ---
{conversation}""",
        "reasoning_effort": "low",
    },
    "probe": {
        "system": "You are a helpful coding assistant that summarizes conversations.",
        "user": """Summarize this conversation. Your summary will be used to continue the work, so preserve:

1. FACTUAL RECALL: Specific facts mentioned (file paths, error messages, API endpoints, model names, version numbers)
2. DECISIONS MADE: What was decided and why (e.g., "switched from X to Y because Z")
3. ARTIFACT TRACKING: Which files were created, modified, or read
4. LOGICAL CONTINUATION: What the next step should be to continue the work

Be precise and specific. Do not generalize — include exact names, paths, and values.

--- Conversation ---
{conversation}""",
        "reasoning_effort": "low",
    },
    "reasoning_medium": {
        "system": "You are a helpful coding assistant that summarizes conversations.",
        "user": """Summarize this conversation focusing on:
- What was accomplished
- Current work in progress
- Files involved
- Next steps
- Key constraints or decisions

Be concise (3-5 bullet points max).

--- Conversation ---
{conversation}""",
        "reasoning_effort": "medium",
    },
    "no_tools_probe": {
        "system": "You are a helpful coding assistant that summarizes conversations.",
        "user": """Summarize this conversation between a user and a coding assistant. Focus on what the user asked and what was accomplished. Ignore tool execution noise.

Your summary will be used to continue the work, so preserve:

1. FACTUAL RECALL: Specific facts (file paths, error messages, API endpoints, model names, versions)
2. DECISIONS MADE: What was decided and why
3. ARTIFACT TRACKING: Which files were created, modified, or read
4. LOGICAL CONTINUATION: What the next step should be

Be precise. Include exact names, paths, and values. Do not generalize.

--- Conversation ---
{conversation}""",
        "reasoning_effort": "low",
    },
}

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

JUDGES = ["glm-5.3-flash", "gpt-5.4-mini", "kimi-k3"]


def load_env():
    env = dict(os.environ)
    env_file = REPO / ".env"
    if env_file.exists():
        for line in env_file.read_text().splitlines():
            if "=" in line and not line.startswith("#"):
                k, v = line.split("=", 1)
                env[k.strip()] = v.strip()
    return env


def call_chat(api_base, model, api_key, system, user, max_tokens=2000, reasoning_effort="low"):
    payload = {"model": model, "messages": [{"role": "system", "content": system}, {"role": "user", "content": user}],
               "temperature": 0.1, "max_tokens": max_tokens}
    if "mercury" in model:
        payload["reasoning_effort"] = reasoning_effort
    req = urllib.request.Request(f"{api_base}/chat/completions", data=json.dumps(payload).encode(),
        headers={"Authorization": f"Bearer {api_key}", "Content-Type": "application/json", "User-Agent": "inception-mercury-compaction/1.0"})
    resp = urllib.request.urlopen(req, timeout=120)
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
        return call_chat(ZEN_BASE, "glm-5.3-flash", api_key, system, user, max_tokens=2000)
    elif judge_key == "gpt-5.4-mini":
        return call_responses(ZEN_BASE, "gpt-5.4-mini", api_key, system, user, max_tokens=2000)
    elif judge_key == "kimi-k3":
        return call_chat(ZEN_BASE, "kimi-k3", api_key, system, user, max_tokens=2000)

def rollout_to_text(jsonl_path, include_tools=True):
    lines = []
    for line in jsonl_path.read_text().splitlines():
        if not line.strip(): continue
        try: msg = json.loads(line)
        except: continue
        role = msg.get("role", "?").upper()
        content = msg.get("content", "")
        tcs = msg.get("tool_calls_summary", [])
        if not include_tools and role == "TOOL":
            continue
        if not include_tools and tcs:
            continue
        if tcs:
            for tc in tcs: lines.append(f"  [{role} -> {tc}]")
        if content:
            lines.append(f"[{role}]")
            lines.append(str(content)[:1500])
            if len(str(content)) > 1500: lines.append("... (truncated)")
            lines.append("")
    return "\n".join(lines)

def parse_json_response(response):
    try:
        cleaned = response.strip()
        if cleaned.startswith("```"):
            cleaned = cleaned.split("\n", 1)[1] if "\n" in cleaned else cleaned
            if cleaned.endswith("```"): cleaned = cleaned.rsplit("```", 1)[0]
            cleaned = cleaned.strip()
        if not cleaned.startswith("{"):
            start = cleaned.find("{"); end = cleaned.rfind("}")
            if start >= 0 and end > start: cleaned = cleaned[start:end+1]
        return json.loads(cleaned)
    except (json.JSONDecodeError, IndexError):
        return {"winner": "error", "score_A": 0, "score_B": 0, "confidence": 0, "reasoning": f"Failed to parse: {response[:300]}"}


def main():
    env = load_env()
    zen_key = env.get("OPENCODE_API_KEY", "")
    mercury_key = env.get("INCEPTION_API_KEY", "")
    OUT.mkdir(parents=True, exist_ok=True)

    # Load Haiku baseline summaries (already generated)
    haiku_dir = REPO / "outputs" / "summaries"
    haiku_summaries = {}
    for name in ROLLOUT_NAMES:
        p = haiku_dir / f"{name}_haiku_full.md"
        if p.exists():
            haiku_summaries[name] = p.read_text()

    # Phase 1: Generate Mercury summaries for each prompt variant
    print("=== Phase 1: Mercury prompt variants ===", file=sys.stderr)
    mercury_summaries = {}  # {variant: {rollout: summary}}
    timings = {}

    for variant_name, prompt_config in PROMPTS.items():
        mercury_summaries[variant_name] = {}
        timings[variant_name] = {}
        print(f"\n  Variant: {variant_name}", file=sys.stderr)

        for name in ROLLOUT_NAMES:
            rollout_path = ROLLOUTS / f"{name}_small.jsonl"
            if not rollout_path.exists(): continue

            # For no_tools variants, strip tool messages
            include_tools = "no_tools" not in variant_name
            conversation = rollout_to_text(rollout_path, include_tools=include_tools)

            out_path = OUT / f"{name}_{variant_name}.md"
            if out_path.exists():
                mercury_summaries[variant_name][name] = out_path.read_text()
                print(f"    {name}: cached", file=sys.stderr)
                continue

            print(f"    {name}: {len(conversation)} chars...", file=sys.stderr)
            t0 = time.time()
            try:
                summary = call_chat(MERCURY_BASE, "mercury-2.5", mercury_key,
                    prompt_config["system"],
                    prompt_config["user"].format(conversation=conversation),
                    max_tokens=1000,
                    reasoning_effort=prompt_config.get("reasoning_effort", "low"))
                if not summary:
                    summary = "ERROR: Mercury returned empty response"
            except Exception as e:
                summary = f"ERROR: {e}"
            elapsed = time.time() - t0
            out_path.write_text(summary)
            mercury_summaries[variant_name][name] = summary
            timings[variant_name][name] = round(elapsed, 2)
            print(f"      {len(summary)} chars in {elapsed:.2f}s", file=sys.stderr)

    # Phase 2: Pairwise A-B/B-A scoring: each Mercury variant vs Haiku
    print("\n=== Phase 2: Pairwise scoring (each Mercury variant vs Haiku) ===", file=sys.stderr)
    all_results = {}  # {variant: [results]}

    for variant_name in PROMPTS:
        results = []
        print(f"\n  Variant: {variant_name}", file=sys.stderr)

        for name in ROLLOUT_NAMES:
            if name not in haiku_summaries: continue
            if name not in mercury_summaries.get(variant_name, {}): continue

            haiku_s = haiku_summaries[name]
            mercury_s = mercury_summaries[variant_name][name]
            if haiku_s.startswith("ERROR") or mercury_s.startswith("ERROR"): continue

            for order_label, sA, sB in [("AB", haiku_s, mercury_s), ("BA", mercury_s, haiku_s)]:
                for judge in JUDGES:
                    out_file = OUT / f"judge_{name}_{variant_name}_{order_label}_{judge}.json"
                    if out_file.exists():
                        try: results.append(json.loads(out_file.read_text()))
                        except: pass
                        continue

                    try:
                        response = call_judge(judge, zen_key, "You are an impartial judge. Return ONLY valid JSON.",
                            JUDGE_PROMPT.format(summary_A=sA, summary_B=sB))
                        verdict = parse_json_response(response)
                    except Exception as e:
                        verdict = {"winner": "error", "score_A": 0, "score_B": 0, "confidence": 0, "reasoning": str(e)}
                    result = {"rollout": name, "variant": variant_name, "order": order_label, "judge": judge, "verdict": verdict}
                    out_file.write_text(json.dumps(result, indent=2))
                    results.append(result)
                    print(f"    {name} {order_label} {judge}: {verdict.get('winner', '?')}", file=sys.stderr)
                    time.sleep(0.3)

        all_results[variant_name] = results

    # Phase 3: Aggregate
    print("\n=== Phase 3: Aggregate ===", file=sys.stderr)
    summary_table = []

    for variant_name in PROMPTS:
        results = all_results.get(variant_name, [])
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

        avg_h = round(sum(haiku_scores)/len(haiku_scores), 1) if haiku_scores else 0
        avg_m = round(sum(mercury_scores)/len(mercury_scores), 1) if mercury_scores else 0
        avg_time = round(sum(timings.get(variant_name, {}).values()) / max(len(timings.get(variant_name, {})), 1), 2)

        summary_table.append({
            "variant": variant_name,
            "haiku_wins": haiku_wins,
            "mercury_wins": mercury_wins,
            "ties": ties,
            "errors": errors,
            "haiku_avg": avg_h,
            "mercury_avg": avg_m,
            "avg_time_s": avg_time,
            "total_evals": len(results),
        })

        print(f"  {variant_name}: Haiku {haiku_wins}, Mercury {mercury_wins}, Ties {ties}, Errors {errors}, "
              f"Haiku avg {avg_h}, Mercury avg {avg_m}, Avg time {avg_time}s", file=sys.stderr)

    # Save results
    (REPO / "results_promptfoo.json").write_text(json.dumps(summary_table, indent=2))

    # Generate report
    lines = [
        "# Promptfoo: Mercury Prompt Variant Experiments",
        "",
        "## Summary Table",
        "",
        "| Variant | Haiku Wins | Mercury Wins | Ties | Errors | Haiku Avg | Mercury Avg | Avg Time (s) |",
        "|---------|-----------|-------------|------|--------|-----------|-------------|--------------|",
    ]
    for s in summary_table:
        lines.append(f"| {s['variant']} | {s['haiku_wins']} | {s['mercury_wins']} | {s['ties']} | {s['errors']} | {s['haiku_avg']} | {s['mercury_avg']} | {s['avg_time_s']} |")

    lines.extend(["", "## Variant Descriptions", ""])
    for name, config in PROMPTS.items():
        lines.append(f"### {name}")
        lines.append(f"- reasoning_effort: {config.get('reasoning_effort', 'low')}")
        lines.append(f"- system: {config['system'][:100]}...")
        lines.append(f"- user prompt (first 200 chars): {config['user'][:200]}...")
        lines.append("")

    (REPO / "REPORT_promptfoo.md").write_text("\n".join(lines))
    print("\nReport saved to REPORT_promptfoo.md", file=sys.stderr)


if __name__ == "__main__":
    main()
