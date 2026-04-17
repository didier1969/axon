#!/usr/bin/env python3
"""Compare two MCP measurement suite runs and flag regressions."""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from typing import Any


DEFAULT_MAX_CORE_REGRESSION_MS = 250.0
DEFAULT_MAX_STACK_REGRESSION_MS = 500.0
DEFAULT_MAX_SYMBOL_REGRESSION_MS = 400.0
DEFAULT_LOAD_RATIO_WARN = 1.25


def load_summary(path: Path) -> dict[str, Any]:
    return json.loads(path.read_text(encoding="utf-8"))


def latest_summaries(root: Path) -> list[Path]:
    return sorted(root.glob("*/summary.json"))


def resolve_paths(args: argparse.Namespace) -> tuple[Path, Path]:
    if args.base and args.candidate:
        return args.base, args.candidate

    if args.candidate and not args.base:
        candidate_summary = load_summary(args.candidate)
        candidate_mode = suite_mode(candidate_summary)
        summaries = [
            path
            for path in latest_summaries(args.runs_root)
            if path.resolve() != args.candidate.resolve()
        ]
        same_mode = [
            path for path in summaries if suite_mode(load_summary(path)) == candidate_mode
        ]
        if same_mode:
            return same_mode[-1], args.candidate
        if summaries:
            return summaries[-1], args.candidate
        raise SystemExit("Need at least one previous MCP run summary to compare against candidate.")

    summaries = latest_summaries(args.runs_root)
    if len(summaries) < 2:
        raise SystemExit("Need at least two MCP run summaries to compare.")
    return summaries[-2], summaries[-1]


def core_map(summary: dict[str, Any]) -> dict[str, float]:
    payload = (
        summary.get("steps", {})
        .get("core_latency", {})
        .get("summary", {})
    )
    slow = payload.get("slow_tools_over_1500ms", [])
    return {
        item.get("tool"): float(item.get("latency_ms"))
        for item in slow
        if item.get("tool") and isinstance(item.get("latency_ms"), (int, float))
    }


def stack_map(summary: dict[str, Any]) -> dict[str, float]:
    payload = summary.get("steps", {}).get("project_status_stack", {}).get("summary", {})
    return {
        key: float(value)
        for key, value in payload.items()
        if isinstance(value, (int, float))
    }


def symbol_map(summary: dict[str, Any]) -> dict[str, float]:
    payload = summary.get("steps", {}).get("symbol_flow", {}).get("summary", {})
    result: dict[str, float] = {}
    for key, value in payload.items():
        if isinstance(value, dict) and isinstance(value.get("latency_ms"), (int, float)):
            result[key] = float(value["latency_ms"])
    return result


def load_depth(summary: dict[str, Any]) -> int:
    queue_depths = summary.get("load_state", {}).get("queue_depths", {})
    queued = queue_depths.get("file_vectorization_queued", 0)
    inflight = queue_depths.get("file_vectorization_inflight", 0)
    if not isinstance(queued, (int, float)) or not isinstance(inflight, (int, float)):
        return 0
    return int(queued + inflight)


def suite_mode(summary: dict[str, Any]) -> str:
    value = summary.get("suite_mode")
    return value if isinstance(value, str) else ("steady_state" if summary.get("warm_cache") else "cold")


def compare_family(
    family: str,
    base: dict[str, float],
    candidate: dict[str, float],
    max_regression_ms: float,
) -> list[dict[str, Any]]:
    findings: list[dict[str, Any]] = []
    for key in sorted(set(base) | set(candidate)):
        base_ms = base.get(key)
        cand_ms = candidate.get(key)
        if base_ms is None or cand_ms is None:
            findings.append(
                {
                    "family": family,
                    "tool": key,
                    "status": "shape_changed",
                    "base_ms": base_ms,
                    "candidate_ms": cand_ms,
                    "delta_ms": None,
                }
            )
            continue
        delta = cand_ms - base_ms
        status = "ok" if delta <= max_regression_ms else "regressed"
        findings.append(
            {
                "family": family,
                "tool": key,
                "status": status,
                "base_ms": round(base_ms, 1),
                "candidate_ms": round(cand_ms, 1),
                "delta_ms": round(delta, 1),
                "max_regression_ms": max_regression_ms,
            }
        )
    return findings


def main() -> int:
    parser = argparse.ArgumentParser(description="Compare two MCP suite summary runs.")
    parser.add_argument("--base", type=Path, help="Base summary.json")
    parser.add_argument("--candidate", type=Path, help="Candidate summary.json")
    parser.add_argument(
        "--runs-root",
        type=Path,
        default=Path("/home/dstadel/projects/axon/.axon/mcp-measure-runs"),
        help="Default root used when --base/--candidate are omitted",
    )
    parser.add_argument("--json-out", type=Path, help="Optional JSON output path")
    parser.add_argument(
        "--max-core-regression-ms",
        type=float,
        default=DEFAULT_MAX_CORE_REGRESSION_MS,
    )
    parser.add_argument(
        "--max-stack-regression-ms",
        type=float,
        default=DEFAULT_MAX_STACK_REGRESSION_MS,
    )
    parser.add_argument(
        "--max-symbol-regression-ms",
        type=float,
        default=DEFAULT_MAX_SYMBOL_REGRESSION_MS,
    )
    parser.add_argument(
        "--load-ratio-warn",
        type=float,
        default=DEFAULT_LOAD_RATIO_WARN,
        help="If candidate vectorization load exceeds base by this ratio, stack regressions downgrade to warn.",
    )
    args = parser.parse_args()

    base_path, candidate_path = resolve_paths(args)
    base = load_summary(base_path)
    candidate = load_summary(candidate_path)

    findings = []
    mode_mismatch = suite_mode(base) != suite_mode(candidate)
    findings.extend(
        compare_family(
            "core_slow_tools",
            core_map(base),
            core_map(candidate),
            args.max_core_regression_ms,
        )
    )
    findings.extend(
        compare_family(
            "project_status_stack",
            stack_map(base),
            stack_map(candidate),
            args.max_stack_regression_ms,
        )
    )
    findings.extend(
        compare_family(
            "symbol_flow",
            symbol_map(base),
            symbol_map(candidate),
            args.max_symbol_regression_ms,
        )
    )

    base_load = load_depth(base)
    candidate_load = load_depth(candidate)
    load_ratio = (candidate_load / base_load) if base_load > 0 else None
    load_contended = load_ratio is not None and load_ratio >= args.load_ratio_warn
    if load_contended:
        for item in findings:
            if item["family"] == "project_status_stack" and item["status"] == "regressed":
                item["status"] = "warn_load"
                item["note"] = (
                    f"candidate vectorization load ratio {load_ratio:.2f} exceeds {args.load_ratio_warn:.2f}"
                )

    verdict = "ok"
    if any(item["status"] == "regressed" for item in findings):
        verdict = "fail"
    elif any(item["status"] in {"shape_changed", "warn_load"} for item in findings):
        verdict = "warn"
    if mode_mismatch and verdict == "ok":
        verdict = "warn"

    payload = {
        "verdict": verdict,
        "base": str(base_path),
        "candidate": str(candidate_path),
        "suite_mode": {
            "base": suite_mode(base),
            "candidate": suite_mode(candidate),
            "mode_mismatch": mode_mismatch,
        },
        "load": {
            "base_vectorization_depth": base_load,
            "candidate_vectorization_depth": candidate_load,
            "ratio": round(load_ratio, 3) if load_ratio is not None else None,
            "load_contended": load_contended,
        },
        "findings": findings,
    }
    rendered = json.dumps(payload, ensure_ascii=False, indent=2)
    print(rendered)
    if args.json_out:
        args.json_out.write_text(rendered + "\n", encoding="utf-8")
    return 1 if verdict == "fail" else 0


if __name__ == "__main__":
    raise SystemExit(main())
