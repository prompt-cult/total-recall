#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Mercury vs Haiku summarization comparison with pairwise A-B/B-A blind scoring.

Phase 1: Summarize 4 rollouts with both claude-haiku-4-5 and mercury-2.5
Phase 2: Pairwise A-B/B-A blind scoring by a panel of 3 flash judges
Phase 3: Aggregate results into REPORT.md

Based on the pairwise methodology from gist b60a4d9af6d7789e70220fb3901ec9ea
"""

from __future__ import annotations

import json
import os
import sys
import time
import urllib.request
from pathlib import Path

# --- Config ---

REPO = Path(__file__).parent
ROLLOUTS = REPO / "rollouts"
SUMMARY_DIR = REPO / "outputs" / "summaries"
PAIRWISE_DIR = REPO / "outputs" / "pairwise"

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

# Models to test for summarization
# Haiku via Anthropic Messages API, Mercury via OpenAI chat/completions
SUMMARY_MODELS = {
    "haiku": {
        "api_base": "https://opencode.ai/zen/v1",
        "model": "claude-haiku-4-5",
        "key_env": "OPENCODE_API_KEY",
        "api_type": "messages",
    },
    "mercury": {
        "api_base": "https://api.inceptionlabs.ai/v1",
        "model": "mercury-2.5",
        "key_env": "INCEPTION_API_KEY",
        "api_type": "chat",
    },
}

# Judge models — one from each working Zen endpoint type
JUDGE_MODELS = {
    "glm-5.3-flash": {
        "api_base": "https://opencode.ai/zen/v1",
        "model": "glm-5.3-flash",
        "key_env": "OPENCODE_API_KEY",
        "api_type": "chat",
    },
    "gpt-5.4-mini": {
        "api_base": "https://opencode.ai/zen/v1",
        "model": "gpt-5.4-mini",
        "key_env": "OPENCODE_API_KEY",
        "api_type": "responses",
    },
    "kimi-k3": {
        "api_base": "https://opencode.ai/zen/v1",
        "model": "kimi-k3",
        "key_env": "OPENCODE_API_KEY",
        "api_type": "chat",
    },
}

ROLLOUT_NAMES = ["vibe", "opencode", "codex", "claude"]


# --- API ---

def load_env() -> dict[str, str]:
    """Load .env file."""
    env = dict(os.environ)
    env_file = REPO / ".env"
    if env_file.exists():
        for line in env_file.read_text().splitlines():
            if "=" in line and not line.startswith("#"):
                k, v = line.split("=", 1)
                env[k.strip()] = v.strip()
    return env


def call_api(api_base: str, model: str, api_key: str, system: str, user: str,
             max_tokens: int = 2000, api_type: str = "chat") -> str:
    """Call an LLM endpoint. Supports three API types:
    - chat: OpenAI chat/completions (DeepSeek, GLM, Kimi, Mercury)
    - messages: Anthropic Messages API (Claude, Qwen)
    - responses: OpenAI Responses API (GPT, Grok, Muse)
    """
    if api_type == "messages":
        # Anthropic Messages API
        payload = {
            "model": model,
            "max_tokens": max_tokens,
            "system": system,
            "messages": [{"role": "user", "content": user}],
        }
        req = urllib.request.Request(
            f"{api_base}/messages",
            data=json.dumps(payload).encode(),
            headers={
                "x-api-key": api_key,
                "Content-Type": "application/json",
                "anthropic-version": "2023-06-01",
                "User-Agent": "inception-mercury-compaction/1.0",
            },
        )
        resp = urllib.request.urlopen(req, timeout=120)
        result = json.loads(resp.read())
        # Anthropic returns content as a list of blocks
        content = result.get("content", [])
        if isinstance(content, list):
            return " ".join(b.get("text", "") for b in content if b.get("type") == "text")
        return str(content)

    elif api_type == "responses":
        # OpenAI Responses API
        payload = {
            "model": model,
            "instructions": system,
            "input": user,
            "max_output_tokens": max_tokens,
        }
        req = urllib.request.Request(
            f"{api_base}/responses",
            data=json.dumps(payload).encode(),
            headers={
                "Authorization": f"Bearer {api_key}",
                "Content-Type": "application/json",
                "User-Agent": "inception-mercury-compaction/1.0",
            },
        )
        resp = urllib.request.urlopen(req, timeout=120)
        result = json.loads(resp.read())
        # Responses API returns output array with message objects
        output = result.get("output", [])
        for item in output:
            if item.get("type") == "message":
                content = item.get("content", [])
                if isinstance(content, list):
                    return " ".join(c.get("text", "") for c in content if c.get("type") == "output_text")
        return str(output)

    else:
        # OpenAI chat/completions
        payload = {
            "model": model,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user},
            ],
            "temperature": 0.1,
            "max_tokens": max_tokens,
        }
        if "mercury" in model:
            payload["reasoning_effort"] = "low"

        req = urllib.request.Request(
            f"{api_base}/chat/completions",
            data=json.dumps(payload).encode(),
            headers={
                "Authorization": f"Bearer {api_key}",
                "Content-Type": "application/json",
                "User-Agent": "inception-mercury-compaction/1.0",
            },
        )
        resp = urllib.request.urlopen(req, timeout=120)
        result = json.loads(resp.read())
        return result["choices"][0]["message"]["content"]


def rollout_to_text(jsonl_path: Path) -> str:
    """Convert a rollout JSONL to a readable conversation text."""
    lines = []
    for line in jsonl_path.read_text().splitlines():
        if not line.strip():
            continue
        try:
            msg = json.loads(line)
        except json.JSONDecodeError:
            continue
        role = msg.get("role", "?").upper()
        content = msg.get("content", "")
        tool_calls = msg.get("tool_calls_summary", [])
        if tool_calls:
            for tc in tool_calls:
                lines.append(f"  [{role} -> {tc}]")
        if content:
            lines.append(f"[{role}]")
            lines.append(str(content)[:1500])
            if len(str(content)) > 1500:
                lines.append("... (truncated)")
            lines.append("")
    return "\n".join(lines)


# --- Phase 1: Summarization ---

def run_summarization(env: dict[str, str]) -> None:
    """Summarize each rollout with both models."""
    SUMMARY_DIR.mkdir(parents=True, exist_ok=True)

    for rollout_name in ROLLOUT_NAMES:
        rollout_path = ROLLOUTS / f"{rollout_name}_small.jsonl"
        if not rollout_path.exists():
            print(f"  Skipping {rollout_name}: rollout file not found", file=sys.stderr)
            continue

        conversation = rollout_to_text(rollout_path)
        print(f"  {rollout_name}: {len(conversation)} chars of conversation", file=sys.stderr)

        for model_key, model_config in SUMMARY_MODELS.items():
            out_path = SUMMARY_DIR / f"{rollout_name}_{model_key}_summary.md"
            if out_path.exists():
                print(f"    {model_key}: already done", file=sys.stderr)
                continue

            api_key = env.get(model_config["key_env"], "")
            if not api_key:
                print(f"    {model_key}: no API key ({model_config['key_env']})", file=sys.stderr)
                continue

            print(f"    {model_key}: summarizing with {model_config['model']}...", file=sys.stderr)
            try:
                summary = call_api(
                    model_config["api_base"],
                    model_config["model"],
                    api_key,
                    "You are a helpful coding assistant that summarizes conversations.",
                    SUMMARY_PROMPT.format(conversation=conversation),
                    max_tokens=1000,
                    api_type=model_config.get("api_type", "chat"),
                )
                out_path.write_text(summary)
                print(f"    {model_key}: {len(summary)} chars written", file=sys.stderr)
            except Exception as e:
                print(f"    {model_key}: ERROR: {e}", file=sys.stderr)
                out_path.write_text(f"ERROR: {e}")

            time.sleep(1)  # rate limit courtesy


# --- Phase 2: Pairwise Judging ---

def run_pairwise(env: dict[str, str]) -> list[dict]:
    """Run pairwise A-B and B-A comparisons for each rollout."""
    PAIRWISE_DIR.mkdir(parents=True, exist_ok=True)
    results = []

    for rollout_name in ROLLOUT_NAMES:
        haiku_summary_path = SUMMARY_DIR / f"{rollout_name}_haiku_summary.md"
        mercury_summary_path = SUMMARY_DIR / f"{rollout_name}_mercury_summary.md"

        if not haiku_summary_path.exists() or not mercury_summary_path.exists():
            print(f"  Skipping {rollout_name}: missing summaries", file=sys.stderr)
            continue

        haiku_summary = haiku_summary_path.read_text()
        mercury_summary = mercury_summary_path.read_text()

        if haiku_summary.startswith("ERROR") or mercury_summary.startswith("ERROR"):
            print(f"  Skipping {rollout_name}: summary had error", file=sys.stderr)
            continue

        # A-B order: A=haiku, B=mercury
        # B-A order: A=mercury, B=haiku
        orderings = [
            ("AB", haiku_summary, mercury_summary),
            ("BA", mercury_summary, haiku_summary),
        ]

        for order_label, summary_A, summary_B in orderings:
            for judge_key, judge_config in JUDGE_MODELS.items():
                api_key = env.get(judge_config["key_env"], "")
                if not api_key:
                    print(f"    {judge_key}: no API key", file=sys.stderr)
                    continue

                out_file = PAIRWISE_DIR / f"{rollout_name}_{order_label}_{judge_key}.json"

                if out_file.exists():
                    print(f"    {rollout_name} {order_label} {judge_key}: already done", file=sys.stderr)
                    try:
                        results.append(json.loads(out_file.read_text()))
                    except json.JSONDecodeError:
                        pass
                    continue

                print(f"    {rollout_name} {order_label} {judge_key}: judging...", file=sys.stderr)
                try:
                    response = call_api(
                        judge_config["api_base"],
                        judge_config["model"],
                        api_key,
                        "You are an impartial judge. Return ONLY valid JSON.",
                        JUDGE_PROMPT.format(summary_A=summary_A, summary_B=summary_B),
                        max_tokens=500,
                        api_type=judge_config.get("api_type", "chat"),
                    )

                    # Try to parse JSON from response
                    try:
                        # Strip markdown code fences if present
                        cleaned = response.strip()
                        if cleaned.startswith("```"):
                            cleaned = cleaned.split("\n", 1)[1] if "\n" in cleaned else cleaned
                            if cleaned.endswith("```"):
                                cleaned = cleaned.rsplit("```", 1)[0]
                            cleaned = cleaned.strip()
                        if not cleaned.startswith("{"):
                            # Find first { and last }
                            start = cleaned.find("{")
                            end = cleaned.rfind("}")
                            if start >= 0 and end > start:
                                cleaned = cleaned[start:end+1]
                        verdict = json.loads(cleaned)
                    except (json.JSONDecodeError, IndexError):
                        verdict = {
                            "winner": "error",
                            "score_A": 0,
                            "score_B": 0,
                            "confidence": 0,
                            "reasoning": f"Failed to parse: {response[:200]}",
                        }

                    result = {
                        "rollout": rollout_name,
                        "order": order_label,
                        "judge": judge_key,
                        "verdict": verdict,
                    }
                    out_file.write_text(json.dumps(result, indent=2))
                    results.append(result)
                    print(f"      -> winner: {verdict.get('winner', '?')}", file=sys.stderr)

                except Exception as e:
                    print(f"      -> ERROR: {e}", file=sys.stderr)
                    result = {
                        "rollout": rollout_name,
                        "order": order_label,
                        "judge": judge_key,
                        "verdict": {
                            "winner": "error",
                            "score_A": 0,
                            "score_B": 0,
                            "confidence": 0,
                            "reasoning": str(e),
                        },
                    }
                    out_file.write_text(json.dumps(result, indent=2))
                    results.append(result)

                time.sleep(1)

    return results


# --- Phase 3: Aggregate ---

def aggregate_results(results: list[dict]) -> dict:
    """Aggregate pairwise results into a summary."""
    # Map back from A/B to haiku/mercury based on order
    # AB: A=haiku, B=mercury
    # BA: A=mercury, B=haiku
    haiku_wins = 0
    mercury_wins = 0
    ties = 0
    errors = 0
    haiku_scores = []
    mercury_scores = []

    by_rollout = {}
    by_judge = {}

    for r in results:
        rollout = r["rollout"]
        order = r["order"]
        judge = r["judge"]
        verdict = r["verdict"]
        winner = verdict.get("winner", "error")
        score_A = verdict.get("score_A", 0)
        score_B = verdict.get("score_B", 0)

        # Map to actual model names
        if order == "AB":
            haiku_score = score_A
            mercury_score = score_B
            if winner == "A":
                haiku_wins += 1
            elif winner == "B":
                mercury_wins += 1
            elif winner == "tie":
                ties += 1
            else:
                errors += 1
        else:  # BA
            haiku_score = score_B
            mercury_score = score_A
            if winner == "B":
                haiku_wins += 1
            elif winner == "A":
                mercury_wins += 1
            elif winner == "tie":
                ties += 1
            else:
                errors += 1

        haiku_scores.append(haiku_score)
        mercury_scores.append(mercury_score)

        # By rollout
        if rollout not in by_rollout:
            by_rollout[rollout] = {"haiku_wins": 0, "mercury_wins": 0, "ties": 0, "errors": 0,
                                    "haiku_scores": [], "mercury_scores": []}
        if winner == "tie":
            by_rollout[rollout]["ties"] += 1
        elif winner == "error":
            by_rollout[rollout]["errors"] += 1
        else:
            # Determine actual winner
            if order == "AB" and winner == "A":
                by_rollout[rollout]["haiku_wins"] += 1
            elif order == "AB" and winner == "B":
                by_rollout[rollout]["mercury_wins"] += 1
            elif order == "BA" and winner == "B":
                by_rollout[rollout]["haiku_wins"] += 1
            elif order == "BA" and winner == "A":
                by_rollout[rollout]["mercury_wins"] += 1
        by_rollout[rollout]["haiku_scores"].append(haiku_score)
        by_rollout[rollout]["mercury_scores"].append(mercury_score)

        # By judge
        if judge not in by_judge:
            by_judge[judge] = {"haiku_wins": 0, "mercury_wins": 0, "ties": 0, "errors": 0}
        if winner == "tie":
            by_judge[judge]["ties"] += 1
        elif winner == "error":
            by_judge[judge]["errors"] += 1
        else:
            if order == "AB" and winner == "A":
                by_judge[judge]["haiku_wins"] += 1
            elif order == "AB" and winner == "B":
                by_judge[judge]["mercury_wins"] += 1
            elif order == "BA" and winner == "B":
                by_judge[judge]["haiku_wins"] += 1
            elif order == "BA" and winner == "A":
                by_judge[judge]["mercury_wins"] += 1

    # Calculate averages
    avg_haiku = sum(haiku_scores) / len(haiku_scores) if haiku_scores else 0
    avg_mercury = sum(mercury_scores) / len(mercury_scores) if mercury_scores else 0

    # Calculate per-rollout averages
    for rollout, data in by_rollout.items():
        data["haiku_avg"] = sum(data["haiku_scores"]) / len(data["haiku_scores"]) if data["haiku_scores"] else 0
        data["mercury_avg"] = sum(data["mercury_scores"]) / len(data["mercury_scores"]) if data["mercury_scores"] else 0
        del data["haiku_scores"]
        del data["mercury_scores"]

    return {
        "total_evaluations": len(results),
        "haiku_wins": haiku_wins,
        "mercury_wins": mercury_wins,
        "ties": ties,
        "errors": errors,
        "haiku_avg_score": round(avg_haiku, 1),
        "mercury_avg_score": round(avg_mercury, 1),
        "by_rollout": by_rollout,
        "by_judge": by_judge,
    }


def generate_report(agg: dict, results: list[dict]) -> str:
    """Generate a markdown report."""
    lines = [
        "# Mercury 2.5 vs Claude Haiku 4.5: Summarization Comparison",
        "",
        "## Methodology",
        "",
        "Based on the pairwise blind evaluation pattern from",
        "[gist b60a4d9af6d7789e70220fb3901ec9ea](https://gist.github.com/simbo1905/b60a4d9af6d7789e70220fb3901ec9ea).",
        "",
        "- **Summarization models**: Claude Haiku 4.5 (via Zen Anthropic Messages API) vs Inception Mercury 2.5",
        "- **Rollouts**: Small sessions from Vibe, OpenCode, Codex CLI, Claude Code",
        "- **Judges**: 3 Zen models (glm-5.3-flash, gpt-5.4-mini, kimi-k3) across all 3 endpoint types",
        "- **Orderings**: Each pair evaluated in both A-B and B-A to detect ordering bias",
        "- **Scoring**: Each judge returns winner (A/B/tie), score_A (0-100), score_B (0-100), confidence, reasoning",
        "",
        "## Overall Results",
        "",
        f"| Metric | Haiku | Mercury |",
        f"|--------|-------|---------|",
        f"| Wins | {agg['haiku_wins']} | {agg['mercury_wins']} |",
        f"| Ties | {agg['ties']} | |",
        f"| Errors | {agg['errors']} | |",
        f"| Avg Score | {agg['haiku_avg_score']} | {agg['mercury_avg_score']} |",
        f"| Total Evaluations | {agg['total_evaluations']} | |",
        "",
        "## Per-Rollout Results",
        "",
        "| Rollout | Haiku Wins | Mercury Wins | Ties | Haiku Avg | Mercury Avg |",
        "|---------|-----------|-------------|------|-----------|-------------|",
    ]

    for rollout, data in sorted(agg["by_rollout"].items()):
        lines.append(
            f"| {rollout} | {data['haiku_wins']} | {data['mercury_wins']} | {data['ties']} | "
            f"{data['haiku_avg']} | {data['mercury_avg']} |"
        )

    lines.extend([
        "",
        "## Per-Judge Results",
        "",
        "| Judge | Haiku Wins | Mercury Wins | Ties | Errors |",
        "|-------|-----------|-------------|------|--------|",
    ])

    for judge, data in sorted(agg["by_judge"].items()):
        lines.append(
            f"| {judge} | {data['haiku_wins']} | {data['mercury_wins']} | {data['ties']} | {data['errors']} |"
        )

    lines.extend([
        "",
        "## Order Bias Check",
        "",
        "Comparing A-B vs B-A results to detect ordering bias:",
        "",
    ])

    # Check order bias
    ab_haiku = sum(1 for r in results if r["order"] == "AB" and r["verdict"].get("winner") == "A")
    ba_haiku = sum(1 for r in results if r["order"] == "BA" and r["verdict"].get("winner") == "B")
    ab_mercury = sum(1 for r in results if r["order"] == "AB" and r["verdict"].get("winner") == "B")
    ba_mercury = sum(1 for r in results if r["order"] == "BA" and r["verdict"].get("winner") == "A")
    ab_ties = sum(1 for r in results if r["order"] == "AB" and r["verdict"].get("winner") == "tie")
    ba_ties = sum(1 for r in results if r["order"] == "BA" and r["verdict"].get("winner") == "tie")

    lines.extend([
        "| Order | Haiku Wins | Mercury Wins | Ties |",
        "|-------|-----------|-------------|------|",
        f"| A-B (A=Haiku) | {ab_haiku} | {ab_mercury} | {ab_ties} |",
        f"| B-A (B=Haiku) | {ba_haiku} | {ba_mercury} | {ba_ties} |",
        "",
        "If the results are consistent across orderings, there is no ordering bias.",
        "If A-B favors A and B-A favors A (regardless of model), there is an ordering bias.",
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
        lines.append(f"- Reasoning: {v.get('reasoning', 'N/A')}")
        lines.append("")

    return "\n".join(lines)


# --- Main ---

def main() -> None:
    env = load_env()

    print("=== Phase 1: Summarization ===", file=sys.stderr)
    run_summarization(env)

    print("\n=== Phase 2: Pairwise Judging ===", file=sys.stderr)
    results = run_pairwise(env)

    print("\n=== Phase 3: Aggregation ===", file=sys.stderr)
    agg = aggregate_results(results)

    # Save results.json
    (REPO / "results.json").write_text(json.dumps(agg, indent=2))
    print(f"Results saved to results.json", file=sys.stderr)

    # Generate report
    report = generate_report(agg, results)
    (REPO / "REPORT.md").write_text(report)
    print(f"Report saved to REPORT.md", file=sys.stderr)

    # Print summary
    print(f"\n=== SUMMARY ===", file=sys.stderr)
    print(f"Haiku wins: {agg['haiku_wins']}, Mercury wins: {agg['mercury_wins']}, Ties: {agg['ties']}", file=sys.stderr)
    print(f"Haiku avg: {agg['haiku_avg_score']}, Mercury avg: {agg['mercury_avg_score']}", file=sys.stderr)


if __name__ == "__main__":
    main()
