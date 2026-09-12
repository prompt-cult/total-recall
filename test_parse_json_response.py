#!/usr/bin/env python3
"""Self-test for parse_json_response in evaluate_chunked.py.

Run: python3 test_parse_json_response.py
"""

from __future__ import annotations

import sys

from evaluate_chunked import parse_json_response
from promptfoo_experiment import parse_json_response as parse_json_response_promptfoo


JSON_OK = '{"winner": "A", "score_A": 90, "score_B": 80, "confidence": 70, "reasoning": "A is tighter"}'


def test_plain_json():
    return parse_json_response(JSON_OK)


def test_fenced_json():
    return parse_json_response("```json\n" + JSON_OK + "\n```")


def test_fenced_json_with_trailing_prose():
    return parse_json_response("```json\n" + JSON_OK + "\n```\nHope that helps!")


def test_prose_with_braces_before_json():
    text = "Sure! Here is {the verdict} you asked for:\n" + JSON_OK
    return parse_json_response(text)


def test_trailing_commas():
    text = '{"winner": "A", "score_A": 90, "score_B": 80, "confidence": 70, "reasoning": "close call",}'
    return parse_json_response(text)


def test_trailing_commas_nested():
    text = '{"winner": "A", "scores": [1, 2,], "score_A": 90, "score_B": 80, "confidence": 70, "reasoning": "ok",}'
    return parse_json_response(text)


def test_truncated_reply_is_error_not_crash():
    v = parse_json_response('{"winner": "A", "score_A": 90')
    assert v.get("winner") == "error", f"truncated reply should classify as error, got {v}"
    return v


def test_empty_reply_is_error_not_crash():
    v = parse_json_response("")
    assert v.get("winner") == "error", f"empty reply should classify as error, got {v}"
    return v


def test_prose_without_json_is_error_not_crash():
    v = parse_json_response("I cannot evaluate this conversation.")
    assert v.get("winner") == "error", f"no-JSON reply should classify as error, got {v}"
    return v


def test_promptfoo_wrapper_matches_hardened_parser():
    """promptfoo_experiment.parse_json_response delegates to the item04-hardened parser."""
    v = parse_json_response_promptfoo("Sure! Here is {the verdict} you asked for:\n" + JSON_OK)
    assert v.get("winner") == "A" and v.get("score_A") == 90 and v.get("score_B") == 80, (
        f"promptfoo wrapper should use hardened parser, got {v}"
    )
    return v


SUCCESS_CASES = [
    ("plain_json", test_plain_json),
    ("fenced_json", test_fenced_json),
    ("fenced_json_with_trailing_prose", test_fenced_json_with_trailing_prose),
    ("prose_with_braces_before_json", test_prose_with_braces_before_json),
    ("trailing_commas", test_trailing_commas),
    ("trailing_commas_nested", test_trailing_commas_nested),
    ("promptfoo_wrapper_matches_hardened_parser", test_promptfoo_wrapper_matches_hardened_parser),
]

ERROR_CASES = [
    ("truncated_reply_is_error_not_crash", test_truncated_reply_is_error_not_crash),
    ("empty_reply_is_error_not_crash", test_empty_reply_is_error_not_crash),
    ("prose_without_json_is_error_not_crash", test_prose_without_json_is_error_not_crash),
]


def main() -> int:
    passed = 0
    failed = 0
    for name, fn in SUCCESS_CASES:
        try:
            v = fn()
            assert v.get("winner") in ("A", "B", "tie"), f"{name}: expected real verdict, got {v}"
            assert v.get("score_A") == 90 and v.get("score_B") == 80 and v.get("confidence") == 70, (
                f"{name}: scores wrong, got {v}"
            )
            passed += 1
            print(f"PASS {name}")
        except Exception as e:
            failed += 1
            print(f"FAIL {name}: {e}")
    for name, fn in ERROR_CASES:
        try:
            fn()
            passed += 1
            print(f"PASS {name}")
        except Exception as e:
            failed += 1
            print(f"FAIL {name}: {e}")
    print(f"\n{passed} passed, {failed} failed")
    return 0 if failed == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
