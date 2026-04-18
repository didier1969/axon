#!/usr/bin/env python3
"""Replay MCP guidance goldens against fixture or live responses."""

from __future__ import annotations

import argparse
import json
import sys
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Any


DEFAULT_URL = "http://127.0.0.1:44129/mcp"


@dataclass
class CaseResult:
    name: str
    status: str  # ok | fail
    note: str


def rpc_call(url: str, payload: dict[str, Any], timeout: int) -> dict[str, Any]:
    data = json.dumps(payload).encode("utf-8")
    req = urllib.request.Request(
        url,
        data=data,
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(req, timeout=timeout) as resp:
        return json.loads(resp.read().decode("utf-8"))


def extract_guidance(response: dict[str, Any]) -> dict[str, Any] | None:
    result = response.get("result")
    if not isinstance(result, dict):
        return None
    data = result.get("data")
    if not isinstance(data, dict):
        return None
    if any(
        key in data
        for key in ("problem_class", "likely_cause", "next_best_actions", "confidence", "soll")
    ):
        return data
    shadow = data.get("_shadow")
    if not isinstance(shadow, dict):
        return None
    guidance = shadow.get("guidance")
    return guidance if isinstance(guidance, dict) else None


def compare_case(case: dict[str, Any], response: dict[str, Any]) -> CaseResult:
    name = str(case.get("name", "unnamed"))
    expect = case.get("expect", {})
    guidance = extract_guidance(response)

    if expect.get("guidance_absent") is True:
        if guidance is None:
            return CaseResult(name, "ok", "guidance absent as expected")
        return CaseResult(name, "fail", "guidance unexpectedly present")

    if guidance is None:
        return CaseResult(name, "fail", "guidance missing")

    expected_problem_class = expect.get("problem_class")
    if expected_problem_class is not None and guidance.get("problem_class") != expected_problem_class:
        return CaseResult(
            name,
            "fail",
            f"problem_class mismatch: expected={expected_problem_class} actual={guidance.get('problem_class')}",
        )

    expected_likely_cause = expect.get("likely_cause")
    if expected_likely_cause is not None and guidance.get("likely_cause") != expected_likely_cause:
        return CaseResult(
            name,
            "fail",
            f"likely_cause mismatch: expected={expected_likely_cause} actual={guidance.get('likely_cause')}",
        )

    expected_confidence = expect.get("confidence")
    if expected_confidence is not None and guidance.get("confidence") != expected_confidence:
        return CaseResult(
            name,
            "fail",
            f"confidence mismatch: expected={expected_confidence} actual={guidance.get('confidence')}",
        )

    expected_actions = expect.get("next_best_actions")
    if isinstance(expected_actions, list):
        actual_actions = guidance.get("next_best_actions")
        if actual_actions != expected_actions:
            return CaseResult(
                name,
                "fail",
                f"next_best_actions mismatch: expected={expected_actions} actual={actual_actions}",
            )

    expected_soll = expect.get("soll")
    if isinstance(expected_soll, dict):
        actual_soll = guidance.get("soll")
        if not isinstance(actual_soll, dict):
            return CaseResult(name, "fail", "expected soll block is missing")
        for key, value in expected_soll.items():
            if actual_soll.get(key) != value:
                return CaseResult(
                    name,
                    "fail",
                    f"soll.{key} mismatch: expected={value} actual={actual_soll.get(key)}",
                )

    return CaseResult(name, "ok", "ok")


def load_response(case: dict[str, Any], url: str, timeout: int) -> dict[str, Any]:
    source = case.get("source", "fixture")
    if source == "fixture":
        response = case.get("response")
        if not isinstance(response, dict):
            raise ValueError(f"fixture case {case.get('name')} missing response object")
        return response
    if source == "live":
        tool = case.get("tool")
        args = case.get("args", {})
        if not isinstance(tool, str):
            raise ValueError(f"live case {case.get('name')} missing tool")
        if not isinstance(args, dict):
            raise ValueError(f"live case {case.get('name')} has invalid args")
        return rpc_call(
            url,
            {
                "jsonrpc": "2.0",
                "id": 7001,
                "method": "tools/call",
                "params": {"name": tool, "arguments": args},
            },
            timeout,
        )
    raise ValueError(f"unknown case source: {source}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--goldens",
        default="scripts/mcp_guidance_goldens.json",
        help="Path to the guidance golden corpus JSON",
    )
    parser.add_argument("--url", default=DEFAULT_URL, help="MCP HTTP endpoint for live cases")
    parser.add_argument("--timeout", type=int, default=10, help="RPC timeout in seconds")
    parser.add_argument("--json-out", help="Optional path to save the full result JSON")
    parser.add_argument(
        "--source",
        choices=("fixture", "live", "all"),
        default="all",
        help="Restrict execution to fixture cases, live cases, or all",
    )
    parser.add_argument(
        "--name-pattern",
        help="Only run cases whose name contains this substring",
    )
    args = parser.parse_args()

    corpus_path = Path(args.goldens)
    corpus = json.loads(corpus_path.read_text(encoding="utf-8"))
    cases = corpus.get("cases", [])
    if not isinstance(cases, list) or not cases:
        print("guidance corpus has no cases", file=sys.stderr)
        return 2

    if args.source != "all":
        cases = [case for case in cases if case.get("source", "fixture") == args.source]

    if args.name_pattern:
        needle = args.name_pattern
        cases = [case for case in cases if needle in str(case.get("name", ""))]

    if not cases:
        print("no guidance cases selected", file=sys.stderr)
        return 2

    results: list[CaseResult] = []
    for case in cases:
        try:
            response = load_response(case, args.url, args.timeout)
            results.append(compare_case(case, response))
        except Exception as exc:  # noqa: BLE001
            results.append(CaseResult(str(case.get("name", "unnamed")), "fail", f"{type(exc).__name__}: {exc}"))

    ok = sum(1 for result in results if result.status == "ok")
    fail = sum(1 for result in results if result.status == "fail")
    false_positive_absent_failures = sum(
        1
        for case, result in zip(cases, results, strict=True)
        if case.get("expect", {}).get("guidance_absent") is True and result.status == "fail"
    )

    payload = {
        "goldens": str(corpus_path),
        "cases": len(results),
        "ok": ok,
        "fail": fail,
        "false_positive_guidance_failures": false_positive_absent_failures,
        "results": [result.__dict__ for result in results],
    }

    if args.json_out:
        Path(args.json_out).write_text(json.dumps(payload, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")

    for result in results:
        print(f"[{result.status.upper()}] {result.name}: {result.note}")
    print(
        json.dumps(
            {
                "cases": len(results),
                "ok": ok,
                "fail": fail,
                "false_positive_guidance_failures": false_positive_absent_failures,
                "verdict": "pass" if fail == 0 else "fail",
            },
            ensure_ascii=False,
        )
    )

    return 0 if fail == 0 else 1


if __name__ == "__main__":
    raise SystemExit(main())
