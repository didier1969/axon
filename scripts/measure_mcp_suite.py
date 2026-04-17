#!/usr/bin/env python3
"""Run the live MCP measurement suite and persist timestamped artifacts."""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

from mcp_probe_common import default_sql_url, sql_query

PROJECT_ROOT = Path("/home/dstadel/projects/axon")
SCRIPT_ROOT = PROJECT_ROOT / "scripts"
RUNS_ROOT = PROJECT_ROOT / ".axon" / "mcp-measure-runs"
DEFAULT_URL = "http://127.0.0.1:44129/mcp"


def utc_stamp() -> str:
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H-%M-%SZ")


def sanitize_label(value: str) -> str:
    cleaned = "".join(ch if ch.isalnum() or ch in "._-" else "-" for ch in value.strip())
    cleaned = cleaned.strip("-")
    return cleaned or "run"


def run_py(script_name: str, extra_args: list[str], env: dict[str, str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(SCRIPT_ROOT / script_name), *extra_args],
        cwd=PROJECT_ROOT,
        text=True,
        capture_output=True,
        env=env,
    )


def parse_json_output(proc: subprocess.CompletedProcess[str]) -> dict[str, Any] | None:
    if proc.returncode != 0:
        return None
    try:
        return json.loads(proc.stdout)
    except Exception:
        return None


def summarize_core(core_payload: dict[str, Any] | None) -> dict[str, Any]:
    if not core_payload:
        return {"ok_tools": 0, "failed_tools": 0, "slow_tools_over_1500ms": []}
    results = core_payload.get("results", [])
    ok_tools = sum(1 for item in results if item.get("ok") is True)
    failed_tools = sum(1 for item in results if item.get("ok") is False)
    slow = [
        {"tool": item.get("tool"), "latency_ms": item.get("latency_ms")}
        for item in results
        if isinstance(item.get("latency_ms"), (int, float)) and float(item["latency_ms"]) > 1500
    ]
    return {
        "ok_tools": ok_tools,
        "failed_tools": failed_tools,
        "slow_tools_over_1500ms": slow,
    }


def suite_mode_label(warm_cache: bool) -> str:
    return "steady_state" if warm_cache else "cold"


def summarize_project_stack(stack_payload: dict[str, Any] | None) -> dict[str, Any]:
    if not stack_payload:
        return {}
    return {
        item["tool"]: item.get("latency_ms")
        for item in stack_payload.get("results", [])
        if "tool" in item and "latency_ms" in item
    }


def summarize_symbol_flow(flow_payload: dict[str, Any] | None) -> dict[str, Any]:
    if not flow_payload:
        return {}
    return {
        item["tool"]: {
            "latency_ms": item.get("latency_ms"),
            "preview": item.get("text_preview", ""),
        }
        for item in flow_payload.get("results", [])
        if "tool" in item
    }


def resolved_probe(step_results: dict[str, Any], requested_symbol: str | None, requested_exact_symbol: str | None) -> dict[str, Any]:
    core_payload = step_results.get("core_latency", {}).get("payload") or {}
    discovered = core_payload.get("discovered_probe") if isinstance(core_payload, dict) else None
    return {
        "requested_symbol": requested_symbol,
        "requested_exact_symbol": requested_exact_symbol,
        "symbol": core_payload.get("symbol") if isinstance(core_payload, dict) else requested_symbol,
        "exact_symbol": core_payload.get("exact_symbol") if isinstance(core_payload, dict) else requested_exact_symbol,
        "discovered_probe": discovered if isinstance(discovered, dict) else {},
    }


def collect_load_state(url: str, project: str, timeout: int) -> dict[str, Any]:
    sql_url = default_sql_url(url)
    escaped_project = project.replace("'", "''")

    def scalar(query: str) -> int:
        rows = sql_query(sql_url, timeout, query)
        if rows and rows[0]:
            value = rows[0][0]
            try:
                return int(value)
            except Exception:
                return 0
        return 0

    return {
        "project_file_counts": {
            "known": scalar(f"SELECT count(*) FROM File WHERE project_code = '{escaped_project}'"),
            "pending": scalar(
                f"SELECT count(*) FROM File WHERE project_code = '{escaped_project}' AND status = 'pending'"
            ),
            "indexing": scalar(
                f"SELECT count(*) FROM File WHERE project_code = '{escaped_project}' AND status = 'indexing'"
            ),
            "indexed": scalar(
                f"""SELECT count(*) FROM File
                WHERE project_code = '{escaped_project}'
                  AND status IN ('indexed', 'indexed_degraded')"""
            ),
        },
        "queue_depths": {
            "file_vectorization_queued": scalar(
                "SELECT count(*) FROM FileVectorizationQueue WHERE status = 'queued'"
            ),
            "file_vectorization_inflight": scalar(
                "SELECT count(*) FROM FileVectorizationQueue WHERE status = 'inflight'"
            ),
            "graph_projection_queued": scalar(
                "SELECT count(*) FROM GraphProjectionQueue WHERE status = 'queued'"
            ),
            "graph_projection_inflight": scalar(
                "SELECT count(*) FROM GraphProjectionQueue WHERE status = 'inflight'"
            ),
        },
    }


def main() -> int:
    parser = argparse.ArgumentParser(description="Run the live MCP measurement suite with timestamped artifacts.")
    parser.add_argument("--url", default=DEFAULT_URL, help=f"MCP URL (default: {DEFAULT_URL})")
    parser.add_argument("--project", default="AXO", help="Canonical project code (default: AXO)")
    parser.add_argument("--symbol", help="Loose symbol probe for search-oriented tools; defaults to live discovery")
    parser.add_argument("--exact-symbol", help="Exact symbol probe for path/impact/change_safety; defaults to live discovery")
    parser.add_argument("--timeout", type=int, default=20, help="Per-request timeout in seconds")
    parser.add_argument("--label", default="live", help="Short label for the measurement run")
    parser.add_argument(
        "--warm-cache",
        action="store_true",
        help="Run one full warmup pass before recording the suite, for steady-state measurements",
    )
    parser.add_argument("--output-root", type=Path, default=RUNS_ROOT, help=f"Artifacts root (default: {RUNS_ROOT})")
    args = parser.parse_args()

    run_dir = args.output_root / f"{utc_stamp()}-{sanitize_label(args.label)}"
    run_dir.mkdir(parents=True, exist_ok=True)

    env = os.environ.copy()
    env["PYTHONPATH"] = str(SCRIPT_ROOT) + (os.pathsep + env["PYTHONPATH"] if env.get("PYTHONPATH") else "")

    commands = {
        "core_latency": [
            "--url",
            args.url,
            "--project",
            args.project,
            "--timeout",
            str(args.timeout),
            "--json-out",
            str(run_dir / "core-latency.json"),
        ],
        "project_status_stack": [
            "--url",
            args.url,
            "--project",
            args.project,
            "--timeout",
            str(max(args.timeout, 30)),
            "--json-out",
            str(run_dir / "project-status-stack.json"),
        ],
        "symbol_flow": [
            "--url",
            args.url,
            "--project",
            args.project,
            "--timeout",
            str(args.timeout),
            "--json-out",
            str(run_dir / "symbol-flow.json"),
        ],
    }

    if args.symbol:
        commands["core_latency"][4:4] = ["--symbol", args.symbol]
    if args.exact_symbol:
        commands["core_latency"][4:4] = ["--exact-symbol", args.exact_symbol]
        commands["symbol_flow"][4:4] = ["--symbol", args.exact_symbol]
    elif args.symbol:
        commands["symbol_flow"][4:4] = ["--symbol", args.symbol]

    script_map = {
        "core_latency": "measure_mcp_core_latency.py",
        "project_status_stack": "measure_project_status_stack.py",
        "symbol_flow": "measure_symbol_flow_tools.py",
    }

    if args.warm_cache:
        warmup_results: dict[str, Any] = {}
        for key, cli_args in commands.items():
            proc = run_py(script_map[key], cli_args, env)
            warmup_results[key] = {
                "exit_code": proc.returncode,
                "payload": parse_json_output(proc),
            }
            (run_dir / f"{key}.warmup.stdout.log").write_text(proc.stdout or "", encoding="utf-8")
            (run_dir / f"{key}.warmup.stderr.log").write_text(proc.stderr or "", encoding="utf-8")

    step_results: dict[str, Any] = {}
    for key, cli_args in commands.items():
        proc = run_py(script_map[key], cli_args, env)
        (run_dir / f"{key}.stdout.log").write_text(proc.stdout or "", encoding="utf-8")
        (run_dir / f"{key}.stderr.log").write_text(proc.stderr or "", encoding="utf-8")
        step_results[key] = {
            "exit_code": proc.returncode,
            "payload": parse_json_output(proc),
        }

    summary = {
        "created_at": datetime.now(timezone.utc).isoformat(timespec="seconds"),
        "run_dir": str(run_dir),
        "url": args.url,
        "project": args.project,
        "warm_cache": args.warm_cache,
        "suite_mode": suite_mode_label(args.warm_cache),
        "load_state": collect_load_state(args.url, args.project, args.timeout),
        "probe": resolved_probe(step_results, args.symbol, args.exact_symbol),
        "steps": {
            "core_latency": {
                "exit_code": step_results["core_latency"]["exit_code"],
                "summary": summarize_core(step_results["core_latency"]["payload"]),
            },
            "project_status_stack": {
                "exit_code": step_results["project_status_stack"]["exit_code"],
                "summary": summarize_project_stack(step_results["project_status_stack"]["payload"]),
            },
            "symbol_flow": {
                "exit_code": step_results["symbol_flow"]["exit_code"],
                "summary": summarize_symbol_flow(step_results["symbol_flow"]["payload"]),
            },
        },
    }

    (run_dir / "summary.json").write_text(
        json.dumps(summary, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )
    print(json.dumps(summary, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
