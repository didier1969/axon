#!/usr/bin/env python3
"""Compare MCP responsiveness and recovery across Axon runtime modes."""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from statistics import mean
from typing import Any


PROJECT_ROOT = Path("/home/dstadel/projects/axon")
GRAPH_ROOT = PROJECT_ROOT / ".axon" / "graph_v2"
IST_DB = GRAPH_ROOT / "ist.db"
IST_WAL = GRAPH_ROOT / "ist.db.wal"
RUNS_ROOT = PROJECT_ROOT / ".axon" / "robustness-runs"
MCP_URL = os.environ.get("AXON_MCP_URL", "http://127.0.0.1:44129/mcp")


@dataclass(frozen=True)
class RequestSpec:
    name: str
    payload: dict[str, Any]


def utc_now_iso() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="seconds")


def sanitize_label(value: str) -> str:
    cleaned = re.sub(r"[^a-zA-Z0-9._-]+", "-", value.strip()).strip("-")
    return cleaned or "run"


def shell(
    args: list[str],
    *,
    check: bool = True,
    capture: bool = True,
    env: dict[str, str] | None = None,
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        args,
        cwd=PROJECT_ROOT,
        env=env,
        text=True,
        capture_output=capture,
        check=check,
    )


def run_script(script: str, extra_args: list[str] | None = None, *, check: bool = True) -> tuple[int, str]:
    args = ["bash", script]
    if extra_args:
        args.extend(extra_args)
    proc = shell(args, check=check)
    return proc.returncode, (proc.stdout or "") + (proc.stderr or "")


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


def parse_error_text(resp: dict[str, Any]) -> str:
    if resp.get("error") is not None:
        return json.dumps(resp["error"], ensure_ascii=False)
    result = resp.get("result")
    if not isinstance(result, dict):
        return ""
    content = result.get("content")
    if not isinstance(content, list):
        return ""
    chunks: list[str] = []
    for item in content:
        if isinstance(item, dict):
            text = item.get("text")
            if isinstance(text, str):
                chunks.append(text)
    return "\n".join(chunks)


def detect_axon_pid() -> int | None:
    try:
        proc = shell(["pgrep", "-af", "axon-core"], capture=True)
    except subprocess.CalledProcessError:
        return None

    for line in proc.stdout.splitlines():
        parts = line.split(maxsplit=1)
        if len(parts) == 2 and "bin/axon-core" in parts[1]:
            try:
                return int(parts[0])
            except ValueError:
                continue
    return None


def wait_for_mcp_ready(url: str, timeout_s: int) -> int:
    deadline = time.time() + timeout_s
    last_pid = None
    initialize = {
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "clientInfo": {"name": "qualify_mcp_robustness", "version": "1.0"},
            "capabilities": {},
        },
    }
    tools_list = {"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}}
    while time.time() < deadline:
        last_pid = detect_axon_pid()
        if last_pid is None:
            time.sleep(1)
            continue
        try:
            init_resp = rpc_call(url, initialize, 10)
            if init_resp.get("error") is not None:
                time.sleep(1)
                continue
            tools_resp = rpc_call(url, tools_list, 10)
            tools = tools_resp.get("result", {}).get("tools", [])
            if isinstance(tools, list) and tools:
                return last_pid
        except Exception:
            time.sleep(1)
    raise RuntimeError(f"MCP runtime not ready after {timeout_s}s (last pid={last_pid})")


def percentile(values: list[int], q: float) -> int:
    if not values:
        return 0
    if len(values) == 1:
        return values[0]
    ordered = sorted(values)
    index = max(0, min(len(ordered) - 1, int(round((len(ordered) - 1) * q))))
    return ordered[index]


def classify_response(resp: dict[str, Any]) -> tuple[str, bool, str]:
    if resp.get("error") is not None:
        message = json.dumps(resp["error"], ensure_ascii=False)
        return "jsonrpc_error", False, message

    text = parse_error_text(resp)
    lower = text.lower()
    result = resp.get("result")
    if not isinstance(result, dict):
        return "invalid_result", False, text
    if (
        lower.startswith("mcp error")
        or lower.startswith("not connected")
        or lower.startswith("axon backend is unavailable")
        or "error sending request for url" in lower
    ):
        return "backend_unavailable", False, text
    if result.get("isError"):
        return "app_error", True, text
    return "ok_result", True, text


def build_request_specs(project: str, soll_project: str, query: str, symbol: str) -> list[RequestSpec]:
    return [
        RequestSpec(
            "initialize",
            {
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2024-11-05",
                    "clientInfo": {"name": "qualify_mcp_robustness", "version": "1.0"},
                    "capabilities": {},
                },
            },
        ),
        RequestSpec(
            "tools_list",
            {"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}},
        ),
        RequestSpec(
            "health",
            {
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/call",
                "params": {"name": "health", "arguments": {"project": project}},
            },
        ),
        RequestSpec(
            "query",
            {
                "jsonrpc": "2.0",
                "id": 4,
                "method": "tools/call",
                "params": {"name": "query", "arguments": {"query": query, "project": project}},
            },
        ),
        RequestSpec(
            "inspect",
            {
                "jsonrpc": "2.0",
                "id": 5,
                "method": "tools/call",
                "params": {"name": "inspect", "arguments": {"symbol": symbol, "project": project}},
            },
        ),
        RequestSpec(
            "impact",
            {
                "jsonrpc": "2.0",
                "id": 6,
                "method": "tools/call",
                "params": {"name": "impact", "arguments": {"symbol": symbol, "depth": 2, "project": project}},
            },
        ),
        RequestSpec(
            "soll_work_plan",
            {
                "jsonrpc": "2.0",
                "id": 7,
                "method": "tools/call",
                "params": {
                    "name": "soll_work_plan",
                    "arguments": {"project_code": soll_project, "format": "json", "top": 3, "limit": 10},
                },
            },
        ),
        RequestSpec(
            "retrieve_context",
            {
                "jsonrpc": "2.0",
                "id": 8,
                "method": "tools/call",
                "params": {
                    "name": "retrieve_context",
                    "arguments": {
                        "question": f"Where is {symbol} wired?",
                        "project": project,
                        "token_budget": 900,
                    },
                },
            },
        ),
    ]


def summarize_events(mode: str, events: list[dict[str, Any]], thresholds: dict[str, Any]) -> dict[str, Any]:
    total = len(events)
    responded = sum(1 for e in events if e["responded"])
    ok_result = sum(1 for e in events if e["category"] == "ok_result")
    app_error = sum(1 for e in events if e["category"] == "app_error")
    jsonrpc_error = sum(1 for e in events if e["category"] == "jsonrpc_error")
    backend_unavailable = sum(1 for e in events if e["category"] == "backend_unavailable")
    timeout_count = sum(1 for e in events if e["category"] == "timeout")
    transport_error = sum(1 for e in events if e["category"] == "transport_error")
    invalid_json = sum(1 for e in events if e["category"] == "invalid_json")
    latencies = [e["duration_ms"] for e in events]
    responded_latencies = [e["duration_ms"] for e in events if e["responded"]]
    outage_categories = {"timeout", "transport_error", "invalid_json", "backend_unavailable", "jsonrpc_error"}
    ever_failed = any(e["category"] in outage_categories for e in events)
    recovery_time_ms = None
    recovered_without_restart = False
    first_outage_at = None
    for event in events:
        if event["category"] in outage_categories and first_outage_at is None:
            first_outage_at = event["started_at_ms"]
        elif first_outage_at is not None and event["responded"]:
            recovery_time_ms = max(0, event["started_at_ms"] - first_outage_at)
            recovered_without_restart = True
            break

    responsive_rate = responded / total if total else 0.0
    success_rate = ok_result / total if total else 0.0
    p95_latency_ms = percentile(responded_latencies, 0.95)
    verdict = "pass"
    if (
        responsive_rate < thresholds["responsive_rate_degraded"]
        or timeout_count > thresholds["max_timeouts_degraded"]
        or backend_unavailable > 0
        or transport_error > 0
        or jsonrpc_error > thresholds["max_jsonrpc_errors_degraded"]
    ):
        verdict = "degraded"
    elif (
        responsive_rate < thresholds["responsive_rate_warn"]
        or timeout_count > thresholds["max_timeouts_warn"]
        or p95_latency_ms > thresholds["p95_latency_warn_ms"]
        or (ever_failed and not recovered_without_restart)
    ):
        verdict = "warn"

    failures = [
        {
            "worker": event["worker"],
            "request": event["request"],
            "category": event["category"],
            "duration_ms": event["duration_ms"],
            "excerpt": event["excerpt"],
        }
        for event in events
        if event["category"] != "ok_result"
    ][:20]

    return {
        "mode": mode,
        "totals": {
            "requests": total,
            "responded": responded,
            "ok_result": ok_result,
            "app_error": app_error,
            "jsonrpc_error": jsonrpc_error,
            "backend_unavailable": backend_unavailable,
            "timeout": timeout_count,
            "transport_error": transport_error,
            "invalid_json": invalid_json,
        },
        "latency_ms": {
            "avg": int(mean(latencies)) if latencies else 0,
            "avg_responded": int(mean(responded_latencies)) if responded_latencies else 0,
            "p95": p95_latency_ms,
            "max": max(latencies) if latencies else 0,
        },
        "rates": {
            "responsive": round(responsive_rate, 4),
            "success": round(success_rate, 4),
        },
        "resilience": {
            "ever_failed": ever_failed,
            "recovered_without_restart": recovered_without_restart,
            "recovery_time_ms": recovery_time_ms,
        },
        "verdict": verdict,
        "failure_samples": failures,
    }


def compare_modes(summaries: list[dict[str, Any]]) -> dict[str, Any]:
    if len(summaries) < 2:
        return {}
    baseline = summaries[0]
    comparisons = []
    for candidate in summaries[1:]:
        comparisons.append(
            {
                "baseline_mode": baseline["mode"],
                "candidate_mode": candidate["mode"],
                "responsive_rate_delta": round(
                    candidate["rates"]["responsive"] - baseline["rates"]["responsive"], 4
                ),
                "success_rate_delta": round(
                    candidate["rates"]["success"] - baseline["rates"]["success"], 4
                ),
                "p95_latency_ms_delta": candidate["latency_ms"]["p95"] - baseline["latency_ms"]["p95"],
                "timeout_delta": candidate["totals"]["timeout"] - baseline["totals"]["timeout"],
                "backend_unavailable_delta": candidate["totals"]["backend_unavailable"]
                - baseline["totals"]["backend_unavailable"],
                "verdict": candidate["verdict"],
            }
        )
    return {"baseline": baseline["mode"], "comparisons": comparisons}


def run_mode(
    *,
    mode: str,
    url: str,
    project: str,
    soll_project: str,
    query: str,
    symbol: str,
    duration: int,
    warmup: int,
    concurrency: int,
    timeout: int,
    reset_ist: bool,
    run_dir: Path,
) -> dict[str, Any]:
    stop_code, stop_output = run_script("scripts/stop.sh", check=False)
    (run_dir / f"{mode}-stop.log").write_text(stop_output)
    if stop_code != 0 and detect_axon_pid() is not None:
        raise RuntimeError(f"stop.sh returned {stop_code} and axon-core is still running for mode={mode}")

    if reset_ist:
        for path in [IST_DB, IST_WAL]:
            try:
                path.unlink()
            except FileNotFoundError:
                pass

    start_arg = {
        "full": "--full",
        "graph_only": "--graph-only",
        "read_only": "--read-only",
        "mcp_only": "--mcp-only",
    }[mode]
    start_code, start_output = run_script("scripts/start.sh", [start_arg], check=False)
    (run_dir / f"{mode}-start.log").write_text(start_output)
    if start_code != 0 and detect_axon_pid() is None:
        raise RuntimeError(f"start.sh returned {start_code} and runtime did not start for mode={mode}")
    pid = wait_for_mcp_ready(url, 120)

    if warmup > 0:
        time.sleep(warmup)

    specs = build_request_specs(project, soll_project, query, symbol)
    deadline = time.time() + duration
    counter = 0
    counter_lock = threading.Lock()
    events: list[dict[str, Any]] = []
    events_lock = threading.Lock()

    def next_spec() -> RequestSpec:
        nonlocal counter
        with counter_lock:
            spec = specs[counter % len(specs)]
            counter += 1
            return spec

    def worker_loop(worker_id: int) -> None:
        while time.time() < deadline:
            spec = next_spec()
            t0 = time.time()
            started_at_ms = int(t0 * 1000)
            category = "ok_result"
            responded = False
            excerpt = ""
            try:
                resp = rpc_call(url, spec.payload, timeout)
                category, responded, excerpt = classify_response(resp)
            except urllib.error.HTTPError as exc:
                category = "transport_error"
                excerpt = f"HTTPError: {exc}"
            except urllib.error.URLError as exc:
                category = "transport_error"
                excerpt = f"URLError: {exc}"
            except TimeoutError as exc:
                category = "timeout"
                excerpt = f"TimeoutError: {exc}"
            except json.JSONDecodeError as exc:
                category = "invalid_json"
                excerpt = f"JSONDecodeError: {exc}"
            except OSError as exc:
                category = "transport_error"
                excerpt = f"OSError: {exc}"
            duration_ms = int((time.time() - t0) * 1000)
            with events_lock:
                events.append(
                    {
                        "worker": worker_id,
                        "request": spec.name,
                        "category": category,
                        "responded": responded,
                        "duration_ms": duration_ms,
                        "started_at_ms": started_at_ms,
                        "excerpt": excerpt[:500],
                    }
                )

    with ThreadPoolExecutor(max_workers=concurrency) as pool:
        for worker_id in range(concurrency):
            pool.submit(worker_loop, worker_id)

    thresholds = {
        "responsive_rate_warn": 0.99,
        "responsive_rate_degraded": 0.95,
        "max_timeouts_warn": 0,
        "max_timeouts_degraded": max(1, concurrency),
        "max_jsonrpc_errors_degraded": 0,
        "p95_latency_warn_ms": 1_500,
    }
    summary = summarize_events(mode, events, thresholds)
    summary["runtime"] = {
        "pid": pid,
        "start_exit_code": start_code,
        "warmup_seconds": warmup,
        "duration_seconds": duration,
        "concurrency": concurrency,
        "timeout_seconds": timeout,
        "reset_ist": reset_ist,
    }
    summary["thresholds"] = thresholds
    (run_dir / f"{mode}-events.ndjson").write_text(
        "".join(json.dumps(event, ensure_ascii=False) + "\n" for event in events),
        encoding="utf-8",
    )
    (run_dir / f"{mode}-summary.json").write_text(
        json.dumps(summary, indent=2, ensure_ascii=False) + "\n",
        encoding="utf-8",
    )
    return summary


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Qualify MCP responsiveness and recovery under load.")
    parser.add_argument("--url", default=MCP_URL, help=f"MCP URL (default: {MCP_URL})")
    parser.add_argument("--modes", default="mcp_only,full", help="Comma-separated runtime modes to compare")
    parser.add_argument("--duration", type=int, default=120, help="Load duration per mode in seconds")
    parser.add_argument("--warmup", type=int, default=5, help="Warm-up seconds after runtime readiness")
    parser.add_argument("--concurrency", type=int, default=4, help="Parallel request workers")
    parser.add_argument("--timeout", type=int, default=10, help="Per-request timeout seconds")
    parser.add_argument("--project", default="BookingSystem", help="Project scope for project-aware MCP tools")
    parser.add_argument("--query", default="booking", help="Query probe used for search-oriented tools")
    parser.add_argument("--symbol", default="parse_batch", help="Symbol probe used for symbol-oriented tools")
    parser.add_argument("--soll-project", default="AXO", help="SOLL project code for soll_work_plan")
    parser.add_argument("--label", default="mcp-robustness", help="Run label")
    parser.add_argument("--output-root", default=str(RUNS_ROOT), help=f"Run artifacts root (default: {RUNS_ROOT})")
    parser.add_argument("--reset-ist", action="store_true", help="Delete ist.db and ist.db.wal before each mode")
    parser.add_argument("--keep-running", action="store_true", help="Leave the last tested mode running")
    return parser.parse_args(argv)


def main(argv: list[str]) -> int:
    args = parse_args(argv)
    modes = [mode.strip() for mode in args.modes.split(",") if mode.strip()]
    for mode in modes:
        if mode not in {"full", "graph_only", "read_only", "mcp_only"}:
            raise SystemExit(f"Unsupported mode: {mode}")
    if args.duration <= 0 or args.concurrency <= 0 or args.timeout <= 0 or args.warmup < 0:
        raise SystemExit("duration, concurrency and timeout must be > 0; warmup must be >= 0")

    output_root = Path(args.output_root)
    output_root.mkdir(parents=True, exist_ok=True)
    started_at = datetime.now()
    run_name = f"{started_at.strftime('%Y-%m-%dT%H-%M-%S')}-{sanitize_label(args.label)}"
    run_dir = output_root / run_name
    run_dir.mkdir(parents=True, exist_ok=False)

    lock = {
        "schema_version": 1,
        "created_at": utc_now_iso(),
        "label": args.label,
        "modes": modes,
        "duration_seconds": args.duration,
        "warmup_seconds": args.warmup,
        "concurrency": args.concurrency,
        "timeout_seconds": args.timeout,
        "project": args.project,
        "query": args.query,
        "symbol": args.symbol,
        "soll_project": args.soll_project,
        "reset_ist": args.reset_ist,
        "keep_running": args.keep_running,
        "paths": {
            "project_root": str(PROJECT_ROOT),
            "run_dir": str(run_dir),
            "ist_db": str(IST_DB),
            "ist_wal": str(IST_WAL),
        },
    }
    (run_dir / "run.lock.json").write_text(json.dumps(lock, indent=2, ensure_ascii=True) + "\n")

    mode_summaries = []
    for mode in modes:
        print(f"[robustness] mode={mode} duration={args.duration}s concurrency={args.concurrency}")
        summary = run_mode(
            mode=mode,
            url=args.url,
            project=args.project,
            soll_project=args.soll_project,
            query=args.query,
            symbol=args.symbol,
            duration=args.duration,
            warmup=args.warmup,
            concurrency=args.concurrency,
            timeout=args.timeout,
            reset_ist=args.reset_ist,
            run_dir=run_dir,
        )
        mode_summaries.append(summary)

    comparison = compare_modes(mode_summaries)
    payload = {
        "created_at": utc_now_iso(),
        "run_dir": str(run_dir),
        "modes": mode_summaries,
        "comparison": comparison,
        "overall_verdict": "degraded"
        if any(item["verdict"] == "degraded" for item in mode_summaries)
        else ("warn" if any(item["verdict"] == "warn" for item in mode_summaries) else "pass"),
        "diagnostic_only": True,
    }
    (run_dir / "summary.json").write_text(
        json.dumps(payload, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
    )

    print(f"[robustness] run_dir={run_dir}")
    for item in mode_summaries:
        print(
            f"- mode={item['mode']} verdict={item['verdict']} responsive={item['rates']['responsive']:.4f} "
            f"success={item['rates']['success']:.4f} p95={item['latency_ms']['p95']}ms "
            f"timeouts={item['totals']['timeout']} backend_unavailable={item['totals']['backend_unavailable']} "
            f"recovered={item['resilience']['recovered_without_restart']}"
        )
    if comparison.get("comparisons"):
        for item in comparison["comparisons"]:
            print(
                f"- compare {item['baseline_mode']} -> {item['candidate_mode']}: "
                f"responsive_delta={item['responsive_rate_delta']:+.4f} "
                f"success_delta={item['success_rate_delta']:+.4f} "
                f"p95_delta={item['p95_latency_ms_delta']}ms "
                f"timeout_delta={item['timeout_delta']} "
                f"backend_unavailable_delta={item['backend_unavailable_delta']}"
            )

    if not args.keep_running:
        stop_code, stop_output = run_script("scripts/stop.sh", check=False)
        (run_dir / "final-stop.log").write_text(stop_output)
        if stop_code != 0 and detect_axon_pid() is not None:
            print("[robustness] warning: final stop.sh did not fully stop runtime", file=sys.stderr)

    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
