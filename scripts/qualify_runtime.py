#!/usr/bin/env python3
"""Unified Axon qualification orchestrator."""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import time
import urllib.error
import urllib.request
from datetime import datetime, timezone
from pathlib import Path
from typing import Any


PROJECT_ROOT = Path("/home/dstadel/projects/axon")
RUNS_ROOT = PROJECT_ROOT / ".axon" / "qualification-suite-runs"
MCP_URL = os.environ.get("AXON_MCP_URL", "http://127.0.0.1:44129/mcp")


def utc_now_iso() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="seconds")


def sanitize_label(value: str) -> str:
    cleaned = re.sub(r"[^a-zA-Z0-9._-]+", "-", value.strip()).strip("-")
    return cleaned or "run"


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Unified Axon qualification orchestrator.")
    parser.add_argument(
        "--profile",
        choices=["smoke", "demo", "full", "ingestion"],
        default="demo",
        help="Qualification profile to run. Default: demo",
    )
    parser.add_argument(
        "--mode",
        choices=["full", "graph_only", "read_only", "mcp_only"],
        default="graph_only",
        help="Primary runtime mode when --compare is not used. Default: graph_only",
    )
    parser.add_argument(
        "--compare",
        default="",
        help="Comma-separated runtime modes to run sequentially instead of --mode.",
    )
    parser.add_argument("--project", default="BookingSystem", help="Project scope for project-aware MCP tools")
    parser.add_argument("--query", default="booking", help="Default semantic query probe")
    parser.add_argument("--symbol", default="", help="Optional symbol probe for symbol-aware tools")
    parser.add_argument("--soll-project", default="AXO", help="SOLL project slug for soll_work_plan probes")
    parser.add_argument("--duration", type=int, default=60, help="Duration in seconds for robustness/ingestion runs")
    parser.add_argument("--warmup", type=int, default=2, help="Warmup in seconds before robustness load")
    parser.add_argument("--concurrency", type=int, default=2, help="Parallel workers for robustness profile")
    parser.add_argument("--timeout", type=int, default=20, help="Timeout in seconds for sub-commands")
    parser.add_argument("--interval", type=int, default=5, help="Sampling interval for ingestion qualification")
    parser.add_argument("--label", default="qualify-suite", help="Short label for output artifacts")
    parser.add_argument(
        "--output-root",
        default=str(RUNS_ROOT),
        help=f"Directory where run artifacts are stored. Default: {RUNS_ROOT}",
    )
    parser.add_argument("--reset-ist", action="store_true", help="Reset IST before robustness/ingestion sub-runs")
    parser.add_argument("--keep-running", action="store_true", help="Leave the last runtime running after completion")
    parser.add_argument(
        "--enforce-gate",
        action="store_true",
        help="Propagate ingestion truth drift gate when the profile includes ingestion qualification",
    )
    return parser.parse_args(argv)


def profile_steps(profile: str) -> list[str]:
    if profile == "smoke":
        return ["runtime_smoke", "mcp_validate"]
    if profile == "demo":
        return ["runtime_smoke", "mcp_validate", "mcp_robustness"]
    if profile == "full":
        return ["runtime_smoke", "mcp_validate", "mcp_robustness", "ingestion_qualify"]
    if profile == "ingestion":
        return ["ingestion_qualify"]
    raise ValueError(f"Unsupported profile: {profile}")


def normalize_modes(mode: str, compare: str) -> list[str]:
    if compare.strip():
        modes = [item.strip() for item in compare.split(",") if item.strip()]
    else:
        modes = [mode]
    seen: list[str] = []
    for item in modes:
        if item not in {"full", "graph_only", "read_only", "mcp_only"}:
            raise SystemExit(f"Unsupported mode: {item}")
        if item not in seen:
            seen.append(item)
    return seen


def combine_step_statuses(steps: list[dict[str, Any]]) -> str:
    statuses = [str(step.get("status", "pass")) for step in steps]
    if any(status == "fail" for status in statuses):
        return "fail"
    if any(status == "warn" for status in statuses):
        return "warn"
    return "pass"


def exit_code_for_verdict(verdict: str) -> int:
    if verdict == "pass":
        return 0
    if verdict == "warn":
        return 1
    return 2


def build_mode_comparison(mode_reports: list[dict[str, Any]]) -> dict[str, Any]:
    if len(mode_reports) < 2:
        return {}

    extracted: list[dict[str, Any]] = []
    for report in mode_reports:
        robustness = report.get("steps", {}).get("mcp_robustness")
        if not isinstance(robustness, dict):
            return {}
        summary = robustness.get("summary", {})
        modes = summary.get("modes", []) if isinstance(summary, dict) else []
        if not isinstance(modes, list) or len(modes) != 1 or not isinstance(modes[0], dict):
            return {}
        extracted.append(modes[0])

    baseline = extracted[0]
    comparisons = []
    for candidate in extracted[1:]:
        comparisons.append(
            {
                "baseline_mode": baseline["mode"],
                "candidate_mode": candidate["mode"],
                "responsive_rate_delta": round(
                    float(candidate["rates"]["responsive"]) - float(baseline["rates"]["responsive"]),
                    4,
                ),
                "success_rate_delta": round(
                    float(candidate["rates"]["success"]) - float(baseline["rates"]["success"]),
                    4,
                ),
                "p95_latency_ms_delta": int(candidate["latency_ms"]["p95"]) - int(baseline["latency_ms"]["p95"]),
                "timeout_delta": int(candidate["totals"]["timeout"]) - int(baseline["totals"]["timeout"]),
                "backend_unavailable_delta": int(candidate["totals"]["backend_unavailable"])
                - int(baseline["totals"]["backend_unavailable"]),
            }
        )
    return {"baseline": baseline["mode"], "comparisons": comparisons}


def shell(args: list[str], *, check: bool = False) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        args,
        cwd=PROJECT_ROOT,
        text=True,
        capture_output=True,
        check=check,
    )


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


def wait_for_mcp_ready(url: str, timeout_s: int) -> None:
    deadline = time.time() + timeout_s
    initialize = {
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "clientInfo": {"name": "qualify_runtime", "version": "1.0"},
            "capabilities": {},
        },
    }
    tools_list = {"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}}
    while time.time() < deadline:
        try:
            init_resp = rpc_call(url, initialize, 10)
            if init_resp.get("error") is not None:
                time.sleep(1)
                continue
            tools_resp = rpc_call(url, tools_list, 10)
            tools = tools_resp.get("result", {}).get("tools", [])
            if isinstance(tools, list) and tools:
                return
        except Exception:
            time.sleep(1)
    raise RuntimeError(f"MCP runtime not ready after {timeout_s}s")


def mode_flag(mode: str) -> str:
    return {
        "full": "--full",
        "graph_only": "--graph-only",
        "read_only": "--read-only",
        "mcp_only": "--mcp-only",
    }[mode]


def write_json(path: Path, payload: Any) -> None:
    path.write_text(json.dumps(payload, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")


def discover_summary_file(root: Path) -> Path | None:
    if not root.exists():
        return None
    summaries = sorted(root.glob("*/summary.json"))
    if summaries:
        return summaries[-1]
    summary = root / "summary.json"
    if summary.exists():
        return summary
    return None


def step_result(name: str, status: str, duration_ms: int, note: str, summary: Any = None) -> dict[str, Any]:
    payload = {
        "name": name,
        "status": status,
        "duration_ms": duration_ms,
        "note": note,
    }
    if summary is not None:
        payload["summary"] = summary
    return payload


def run_runtime_smoke(mode: str, run_dir: Path, url: str) -> dict[str, Any]:
    t0 = time.time()
    stop_proc = shell(["bash", "scripts/stop.sh"])
    start_proc = shell(["bash", "scripts/start.sh", mode_flag(mode)])
    (run_dir / "runtime-stop.log").write_text((stop_proc.stdout or "") + (stop_proc.stderr or ""), encoding="utf-8")
    (run_dir / "runtime-start.log").write_text((start_proc.stdout or "") + (start_proc.stderr or ""), encoding="utf-8")

    try:
        wait_for_mcp_ready(url, 120)
        return step_result("runtime_smoke", "pass", int((time.time() - t0) * 1000), "runtime ready")
    except Exception as exc:
        return step_result("runtime_smoke", "fail", int((time.time() - t0) * 1000), f"{type(exc).__name__}: {exc}")


def run_mcp_validate(args: argparse.Namespace, mode: str, run_dir: Path) -> dict[str, Any]:
    t0 = time.time()
    json_out = run_dir / "mcp_validate.json"
    cmd = [
        sys.executable,
        "scripts/mcp_validate.py",
        "--project",
        args.project,
        "--query",
        args.query,
        "--timeout",
        str(args.timeout),
        "--json-out",
        str(json_out),
    ]
    if args.symbol:
        cmd.extend(["--symbol", args.symbol])
    proc = shell(cmd)
    (run_dir / "mcp_validate.stdout.log").write_text((proc.stdout or "") + (proc.stderr or ""), encoding="utf-8")
    summary = {}
    if json_out.exists():
        summary = json.loads(json_out.read_text(encoding="utf-8"))
    step_status = "fail"
    note = f"exit={proc.returncode}"
    if isinstance(summary, dict):
        summary_block = summary.get("summary", {})
        fail = int(summary_block.get("fail", 0))
        warn = int(summary_block.get("warn", 0))
        if fail > 0:
            step_status = "fail"
        elif warn > 0:
            step_status = "warn"
        else:
            step_status = "pass"
        note = (
            f"ok={summary_block.get('ok', 0)} "
            f"warn={warn} fail={fail} skip={summary_block.get('skip', 0)}"
        )
    return step_result("mcp_validate", step_status, int((time.time() - t0) * 1000), note, summary)


def run_mcp_robustness(args: argparse.Namespace, mode: str, run_dir: Path) -> dict[str, Any]:
    t0 = time.time()
    output_root = run_dir / "robustness"
    output_root.mkdir(parents=True, exist_ok=True)
    cmd = [
        sys.executable,
        "scripts/qualify_mcp_robustness.py",
        "--modes",
        mode,
        "--duration",
        str(args.duration),
        "--warmup",
        str(args.warmup),
        "--concurrency",
        str(args.concurrency),
        "--timeout",
        str(args.timeout),
        "--project",
        args.project,
        "--query",
        args.query,
        "--soll-project",
        args.soll_project,
        "--label",
        f"{args.label}-{mode}",
        "--output-root",
        str(output_root),
        "--keep-running",
    ]
    if args.symbol:
        cmd.extend(["--symbol", args.symbol])
    if args.reset_ist:
        cmd.append("--reset-ist")
    proc = shell(cmd)
    (run_dir / "mcp_robustness.stdout.log").write_text((proc.stdout or "") + (proc.stderr or ""), encoding="utf-8")
    summary_path = discover_summary_file(output_root)
    summary = {}
    if summary_path is not None:
        summary = json.loads(summary_path.read_text(encoding="utf-8"))
    overall = summary.get("overall_verdict") if isinstance(summary, dict) else None
    step_status = "pass"
    if overall == "degraded":
        step_status = "fail"
    elif overall == "warn":
        step_status = "warn"
    note = f"verdict={overall or 'unknown'} exit={proc.returncode}"
    return step_result("mcp_robustness", step_status, int((time.time() - t0) * 1000), note, summary)


def run_ingestion_qualify(args: argparse.Namespace, mode: str, run_dir: Path) -> dict[str, Any]:
    t0 = time.time()
    output_root = run_dir / "ingestion"
    output_root.mkdir(parents=True, exist_ok=True)
    cmd = [
        sys.executable,
        "scripts/qualify_ingestion_run.py",
        "--mode",
        mode,
        "--duration",
        str(args.duration),
        "--interval",
        str(args.interval),
        "--label",
        f"{args.label}-{mode}",
        "--output-root",
        str(output_root),
    ]
    if not args.reset_ist:
        cmd.append("--no-reset-ist")
    if not args.keep_running:
        cmd.append("--stop-after")
    if args.enforce_gate:
        cmd.append("--enforce-gate")
    proc = shell(cmd)
    (run_dir / "ingestion_qualify.stdout.log").write_text((proc.stdout or "") + (proc.stderr or ""), encoding="utf-8")
    summary_path = discover_summary_file(output_root)
    summary = {}
    if summary_path is not None:
        summary = json.loads(summary_path.read_text(encoding="utf-8"))
    step_status = "pass" if proc.returncode == 0 else "fail"
    note = f"exit={proc.returncode}"
    if isinstance(summary, dict) and summary.get("truth_drift_detected"):
        step_status = "warn" if not args.enforce_gate else "fail"
        note = f"truth_drift_detected exit={proc.returncode}"
    return step_result("ingestion_qualify", step_status, int((time.time() - t0) * 1000), note, summary)


def run_mode_profile(args: argparse.Namespace, mode: str, suite_run_dir: Path) -> dict[str, Any]:
    mode_run_dir = suite_run_dir / mode
    mode_run_dir.mkdir(parents=True, exist_ok=True)
    steps: dict[str, dict[str, Any]] = {}
    ordered_steps: list[dict[str, Any]] = []

    for step_name in profile_steps(args.profile):
        if step_name == "runtime_smoke":
            result = run_runtime_smoke(mode, mode_run_dir, MCP_URL)
        elif step_name == "mcp_validate":
            if steps.get("runtime_smoke", {}).get("status") == "fail":
                result = step_result("mcp_validate", "fail", 0, "skipped because runtime_smoke failed")
            else:
                result = run_mcp_validate(args, mode, mode_run_dir)
        elif step_name == "mcp_robustness":
            if steps.get("runtime_smoke", {}).get("status") == "fail":
                result = step_result("mcp_robustness", "fail", 0, "skipped because runtime_smoke failed")
            else:
                result = run_mcp_robustness(args, mode, mode_run_dir)
        elif step_name == "ingestion_qualify":
            result = run_ingestion_qualify(args, mode, mode_run_dir)
        else:
            result = step_result(step_name, "fail", 0, "unsupported step")
        steps[step_name] = result
        ordered_steps.append(result)

    verdict = combine_step_statuses(ordered_steps)
    return {
        "mode": mode,
        "profile": args.profile,
        "verdict": verdict,
        "steps": steps,
        "step_order": [step["name"] for step in ordered_steps],
    }


def main(argv: list[str]) -> int:
    args = parse_args(argv)
    modes = normalize_modes(args.mode, args.compare)
    output_root = Path(args.output_root)
    output_root.mkdir(parents=True, exist_ok=True)

    started_at = datetime.now()
    run_name = f"{started_at.strftime('%Y-%m-%dT%H-%M-%S')}-{sanitize_label(args.label)}"
    run_dir = output_root / run_name
    run_dir.mkdir(parents=True, exist_ok=False)

    lock = {
        "schema_version": 1,
        "created_at": utc_now_iso(),
        "profile": args.profile,
        "mode": args.mode,
        "compare": args.compare,
        "modes": modes,
        "project": args.project,
        "query": args.query,
        "symbol": args.symbol,
        "soll_project": args.soll_project,
        "duration_seconds": args.duration,
        "warmup_seconds": args.warmup,
        "concurrency": args.concurrency,
        "timeout_seconds": args.timeout,
        "interval_seconds": args.interval,
        "reset_ist": args.reset_ist,
        "keep_running": args.keep_running,
        "paths": {
            "project_root": str(PROJECT_ROOT),
            "run_dir": str(run_dir),
        },
    }
    write_json(run_dir / "run.lock.json", lock)

    mode_reports = []
    for mode in modes:
        print(f"[qualify] profile={args.profile} mode={mode}")
        mode_reports.append(run_mode_profile(args, mode, run_dir))

    overall_verdict = combine_step_statuses(
        [{"status": report["verdict"]} for report in mode_reports]
    )
    comparison = build_mode_comparison(mode_reports)
    summary = {
        "created_at": utc_now_iso(),
        "run_dir": str(run_dir),
        "profile": args.profile,
        "modes": modes,
        "mode_reports": mode_reports,
        "comparison": comparison,
        "overall_verdict": overall_verdict,
    }
    write_json(run_dir / "summary.json", summary)

    print(f"[qualify] run_dir={run_dir}")
    print(f"[qualify] overall_verdict={overall_verdict}")
    for report in mode_reports:
        print(f"- mode={report['mode']} verdict={report['verdict']}")
        for step_name in report["step_order"]:
            step = report["steps"][step_name]
            print(f"  - {step_name}: {step['status']} ({step['duration_ms']} ms) :: {step['note']}")
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

    return exit_code_for_verdict(overall_verdict)


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
