#!/usr/bin/env python3
"""Unified MCP qualification orchestrator."""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any


SCRIPT_ROOT = Path(__file__).resolve().parent
PROJECT_ROOT = SCRIPT_ROOT.parent
DEFAULT_RUNS_ROOT = PROJECT_ROOT / ".axon" / "mcp-qualification-runs"
DEFAULT_MCP_URL = "http://127.0.0.1:44129/mcp"
DEFAULT_COMPARE_RUNS_ROOT = PROJECT_ROOT / ".axon" / "mcp-measure-runs"

SURFACE_CHOICES = ("core", "soll", "all")
CHECK_CHOICES = ("quality", "latency", "robustness", "guidance")
MODE_CHOICES = ("cold", "steady-state", "both")
MUTATION_CHOICES = ("off", "dry-run", "safe-live", "full")


@dataclass(frozen=True)
class InvocationPlan:
    surface: str
    checks: tuple[str, ...]
    mode: str
    mutations: str
    project: str
    soll_project: str
    baseline: str | None
    strict: bool
    timeout: int
    label: str | None
    json_out: Path | None
    artifacts_root: Path
    scenario_file: Path | None
    keep_running: bool
    reset_ist: bool
    top_slowest: int
    name_pattern: str | None
    query: str | None
    symbol: str | None
    url: str
    skip_regression: bool


def csv_values(raw: str | None) -> tuple[str, ...]:
    if not raw:
        return ()
    values = [item.strip() for item in raw.split(",")]
    return tuple(item for item in values if item)


def utc_stamp() -> str:
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H-%M-%SZ")


def utc_iso() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="seconds")


def sanitize_label(value: str) -> str:
    cleaned = re.sub(r"[^a-zA-Z0-9._-]+", "-", value.strip()).strip("-")
    return cleaned or "run"


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--surface", choices=SURFACE_CHOICES, default="core", help="Qualification surface")
    parser.add_argument(
        "--checks",
        default="quality,latency",
        help="Comma-separated checks: quality,latency,robustness,guidance",
    )
    parser.add_argument("--mode", choices=MODE_CHOICES, default="steady-state", help="Qualification mode")
    parser.add_argument(
        "--mutations",
        choices=MUTATION_CHOICES,
        default="off",
        help="Mutation level for write-capable qualification",
    )
    parser.add_argument("--project", default="AXO", help="Project code for core-oriented probes")
    parser.add_argument("--soll-project", default="", help="Explicit SOLL project code override")
    parser.add_argument("--query", default="", help="Query probe override")
    parser.add_argument("--symbol", default="", help="Symbol probe override")
    parser.add_argument("--strict", action="store_true", help="Treat warnings as failures when supported")
    parser.add_argument("--baseline", default="auto", help="Baseline summary.json or 'auto'")
    parser.add_argument("--json-out", default="", help="Optional aggregated summary output path")
    parser.add_argument("--label", default="", help="Run label")
    parser.add_argument("--timeout", type=int, default=60, help="Timeout in seconds for delegated checks")
    parser.add_argument(
        "--artifacts-root",
        type=Path,
        default=DEFAULT_RUNS_ROOT,
        help=f"Qualification artifacts root (default: {DEFAULT_RUNS_ROOT})",
    )
    parser.add_argument("--scenario-file", default="", help="Optional override scenario for quality validation")
    parser.add_argument("--keep-running", action="store_true", help="Keep runtime alive after robustness runs")
    parser.add_argument("--reset-ist", action="store_true", help="Reset IST before robustness runs")
    parser.add_argument("--top-slowest", type=int, default=5, help="Top slowest items to retain in summaries")
    parser.add_argument("--name-pattern", default="", help="Guidance case filter")
    parser.add_argument("--skip-regression", action="store_true", help="Skip run-to-run latency comparison")
    parser.add_argument("--url", default=DEFAULT_MCP_URL, help=f"MCP endpoint (default: {DEFAULT_MCP_URL})")
    return parser


def canonical_checks(raw: str) -> tuple[str, ...]:
    values = csv_values(raw)
    if not values:
        raise SystemExit("At least one check is required via --checks.")
    unknown = sorted(set(values) - set(CHECK_CHOICES))
    if unknown:
        raise SystemExit(f"Unknown checks: {', '.join(unknown)}")
    seen: list[str] = []
    for value in values:
        if value not in seen:
            seen.append(value)
    return tuple(seen)


def normalize_plan(args: argparse.Namespace) -> InvocationPlan:
    checks = canonical_checks(args.checks)
    soll_project = args.soll_project or args.project
    baseline = None if args.baseline in {"", "auto"} else args.baseline
    scenario_file = Path(args.scenario_file) if args.scenario_file else None
    json_out = Path(args.json_out) if args.json_out else None

    if args.surface == "core" and args.mutations != "off":
        raise SystemExit("Core surface does not accept --mutations other than 'off'.")
    if args.surface == "all" and scenario_file is not None:
        raise SystemExit("--scenario-file is only supported for --surface core or --surface soll.")
    if args.surface in {"soll", "all"} and args.mutations in {"safe-live", "full"} and scenario_file is None:
        raise SystemExit(
            "SOLL-inclusive mutation modes 'safe-live' and 'full' require --scenario-file in phase 1."
        )
    if args.reset_ist and "robustness" not in checks:
        raise SystemExit("--reset-ist is only valid with --checks including robustness.")
    if args.keep_running and "robustness" not in checks:
        raise SystemExit("--keep-running is only valid with --checks including robustness.")
    if baseline and "latency" not in checks:
        raise SystemExit("--baseline requires --checks including latency.")
    if args.skip_regression and "latency" not in checks:
        raise SystemExit("--skip-regression requires --checks including latency.")

    return InvocationPlan(
        surface=args.surface,
        checks=checks,
        mode=args.mode,
        mutations=args.mutations,
        project=args.project,
        soll_project=soll_project,
        baseline=baseline,
        strict=bool(args.strict),
        timeout=args.timeout,
        label=args.label or None,
        json_out=json_out,
        artifacts_root=args.artifacts_root,
        scenario_file=scenario_file,
        keep_running=bool(args.keep_running),
        reset_ist=bool(args.reset_ist),
        top_slowest=args.top_slowest,
        name_pattern=args.name_pattern or None,
        query=args.query or None,
        symbol=args.symbol or None,
        url=args.url,
        skip_regression=bool(args.skip_regression),
    )


def plan_payload(plan: InvocationPlan) -> dict[str, Any]:
    return {
        "surface": plan.surface,
        "checks": list(plan.checks),
        "mode": plan.mode,
        "mutations": plan.mutations,
        "project": plan.project,
        "soll_project": plan.soll_project,
        "baseline": plan.baseline or "auto",
        "strict": plan.strict,
        "timeout": plan.timeout,
        "label": plan.label,
        "json_out": str(plan.json_out) if plan.json_out else None,
        "artifacts_root": str(plan.artifacts_root),
        "scenario_file": str(plan.scenario_file) if plan.scenario_file else None,
        "keep_running": plan.keep_running,
        "reset_ist": plan.reset_ist,
        "top_slowest": plan.top_slowest,
        "name_pattern": plan.name_pattern,
        "query": plan.query,
        "symbol": plan.symbol,
        "url": plan.url,
        "skip_regression": plan.skip_regression,
    }


def ensure_dir(path: Path) -> Path:
    path.mkdir(parents=True, exist_ok=True)
    return path


def run_py(script_name: str, extra_args: list[str], *, cwd: Path | None = None) -> subprocess.CompletedProcess[str]:
    env = os.environ.copy()
    env["PYTHONPATH"] = str(SCRIPT_ROOT) + (os.pathsep + env["PYTHONPATH"] if env.get("PYTHONPATH") else "")
    return subprocess.run(
        [sys.executable, str(SCRIPT_ROOT / script_name), *extra_args],
        cwd=cwd or PROJECT_ROOT,
        text=True,
        capture_output=True,
        env=env,
    )


def write_logs(base_path: Path, proc: subprocess.CompletedProcess[str]) -> None:
    base_path.with_suffix(".stdout.log").write_text(proc.stdout or "", encoding="utf-8")
    base_path.with_suffix(".stderr.log").write_text(proc.stderr or "", encoding="utf-8")


def parse_json_file(path: Path) -> dict[str, Any] | None:
    if not path.exists():
        return None
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except Exception:
        return None


def parse_json_stdout(proc: subprocess.CompletedProcess[str]) -> dict[str, Any] | None:
    try:
        return json.loads(proc.stdout)
    except Exception:
        return None


def tool_slowest_from_validate(payload: dict[str, Any] | None, limit: int) -> list[dict[str, Any]]:
    if not payload:
        return []
    results = payload.get("results", [])
    items = [
        {
            "tool": item.get("name"),
            "latency_ms": item.get("duration_ms"),
            "status": item.get("status"),
        }
        for item in results
        if isinstance(item, dict) and isinstance(item.get("duration_ms"), int)
    ]
    return sorted(items, key=lambda item: item["latency_ms"], reverse=True)[:limit]


def verdict_from_validate(payload: dict[str, Any] | None, strict: bool) -> str:
    if not payload:
        return "fail"
    summary = payload.get("summary", {})
    fail = int(summary.get("fail", 0))
    warn = int(summary.get("warn", 0))
    if fail > 0:
        return "fail"
    if warn > 0:
        return "fail" if strict else "warn"
    return "ok"


def verdict_from_compare(payload: dict[str, Any] | None) -> str:
    if not payload:
        return "fail"
    verdict = payload.get("verdict")
    if verdict == "ok":
        return "ok"
    if verdict == "warn":
        return "warn"
    return "fail"


def verdict_from_robustness(payload: dict[str, Any] | None) -> str:
    if not payload:
        return "fail"
    overall = payload.get("overall_verdict")
    if overall == "pass":
        return "ok"
    if overall == "warn":
        return "warn"
    return "fail"


def verdict_from_guidance(payload: dict[str, Any] | None) -> str:
    if not payload:
        return "fail"
    fail = int(payload.get("fail", 0))
    if fail > 0:
        return "fail"
    return "ok"


def summarize_soll_latency_from_validate(payload: dict[str, Any] | None, limit: int) -> dict[str, Any]:
    if not payload:
        return {"verdict": "fail", "slowest_tools": []}
    results = payload.get("results", [])
    durations = [
        {
            "tool": item.get("name"),
            "latency_ms": item.get("duration_ms"),
            "status": item.get("status"),
        }
        for item in results
        if isinstance(item, dict)
        and isinstance(item.get("duration_ms"), int)
        and str(item.get("name", "")).startswith(("soll_", "axon_init_project", "axon_apply_guidelines"))
    ]
    slowest = sorted(durations, key=lambda item: item["latency_ms"], reverse=True)[:limit]
    max_latency = max((item["latency_ms"] for item in durations), default=0)
    verdict = "ok"
    if max_latency > 1500:
        verdict = "warn"
    if any(item.get("status") == "fail" for item in durations):
        verdict = "fail"
    return {
        "verdict": verdict,
        "slowest_tools": slowest,
        "max_latency_ms": max_latency,
    }


def default_quality_scenario(surface: str, mutations: str) -> Path | None:
    if surface == "core":
        path = SCRIPT_ROOT / "mcp_scenarios" / "core_qualification.json"
        return path if path.exists() else None
    if surface == "soll":
        name = "soll_dry_run_qualification.json" if mutations == "dry-run" else "soll_readonly_qualification.json"
        path = SCRIPT_ROOT / "mcp_scenarios" / name
        return path if path.exists() else None
    return None


def run_quality_surface(
    plan: InvocationPlan,
    *,
    target_surface: str,
    run_dir: Path,
    scenario_override: Path | None = None,
) -> dict[str, Any]:
    json_path = run_dir / f"quality-{target_surface}.json"
    log_base = run_dir / f"quality-{target_surface}"
    project = plan.project if target_surface == "core" else plan.soll_project
    args = [
        "--url",
        plan.url,
        "--surface",
        target_surface,
        "--mutation-mode",
        plan.mutations if target_surface == "soll" else "off",
        "--project",
        project,
        "--timeout",
        str(plan.timeout),
        "--top-slowest",
        str(plan.top_slowest),
        "--json-out",
        str(json_path),
    ]
    if plan.strict:
        args.append("--strict")
    if plan.query:
        args.extend(["--query", plan.query])
    if plan.symbol:
        args.extend(["--symbol", plan.symbol])
    scenario_file = scenario_override or default_quality_scenario(target_surface, plan.mutations)
    if scenario_file:
        args.extend(["--scenario-file", str(scenario_file)])
    if target_surface == "soll" and plan.mutations == "full":
        args.append("--allow-mutations")
    proc = run_py("mcp_validate.py", args)
    write_logs(log_base, proc)
    payload = parse_json_file(json_path)
    verdict = verdict_from_validate(payload, plan.strict)
    return {
        "surface": target_surface,
        "engine": "mcp_validate.py",
        "command": ["python3", "scripts/mcp_validate.py", *args],
        "exit_code": proc.returncode,
        "verdict": verdict,
        "artifact": str(json_path),
        "scenario_file": str(scenario_file) if scenario_file else None,
        "payload": payload,
        "slowest_tools": tool_slowest_from_validate(payload, plan.top_slowest),
    }


def measure_suite_args(plan: InvocationPlan, *, warm_cache: bool, output_root: Path, label: str) -> list[str]:
    args = [
        "--url",
        plan.url,
        "--project",
        plan.project,
        "--timeout",
        str(min(plan.timeout, 60)),
        "--output-root",
        str(output_root),
        "--label",
        label,
    ]
    if warm_cache:
        args.append("--warm-cache")
    if plan.symbol:
        args.extend(["--symbol", plan.symbol])
    return args


def latest_summary(root: Path) -> Path | None:
    summaries = sorted(root.glob("*/summary.json"))
    return summaries[-1] if summaries else None


def run_core_latency_mode(plan: InvocationPlan, *, warm_cache: bool, run_dir: Path, label: str) -> dict[str, Any]:
    output_root = ensure_dir(run_dir / "measure-runs")
    log_base = run_dir / f"latency-{'steady' if warm_cache else 'cold'}"
    args = measure_suite_args(plan, warm_cache=warm_cache, output_root=output_root, label=label)
    proc = run_py("measure_mcp_suite.py", args)
    write_logs(log_base, proc)
    summary_path = latest_summary(output_root)
    payload = parse_json_file(summary_path) if summary_path else None
    verdict = "fail"
    if payload:
        slow = payload.get("steps", {}).get("core_latency", {}).get("summary", {}).get("slow_tools_over_1500ms", [])
        verdict = "warn" if slow else "ok"
    return {
        "engine": "measure_mcp_suite.py",
        "exit_code": proc.returncode,
        "artifact": str(summary_path) if summary_path else None,
        "payload": payload,
        "verdict": verdict,
        "mode": "steady-state" if warm_cache else "cold",
        "command": ["python3", "scripts/measure_mcp_suite.py", *args],
    }


def run_regression(plan: InvocationPlan, *, candidate_summary: Path, run_dir: Path) -> dict[str, Any]:
    json_path = run_dir / "latency-regression.json"
    log_base = run_dir / "latency-regression"
    args = ["--candidate", str(candidate_summary), "--json-out", str(json_path)]
    if plan.baseline:
        args.extend(["--base", plan.baseline])
    else:
        args.extend(["--runs-root", str(DEFAULT_COMPARE_RUNS_ROOT)])
    proc = run_py("compare_mcp_runs.py", args)
    write_logs(log_base, proc)
    payload = parse_json_file(json_path) or parse_json_stdout(proc)
    return {
        "engine": "compare_mcp_runs.py",
        "exit_code": proc.returncode,
        "artifact": str(json_path),
        "payload": payload,
        "verdict": verdict_from_compare(payload),
        "command": ["python3", "scripts/compare_mcp_runs.py", *args],
    }


def run_guidance(plan: InvocationPlan, *, run_dir: Path) -> dict[str, Any]:
    json_path = run_dir / "guidance.json"
    log_base = run_dir / "guidance"
    args = ["--json-out", str(json_path), "--timeout", str(min(plan.timeout, 30)), "--url", plan.url]
    if plan.name_pattern:
        args.extend(["--name-pattern", plan.name_pattern])
    proc = run_py("qualify_mcp_guidance.py", args)
    write_logs(log_base, proc)
    payload = parse_json_file(json_path)
    return {
        "engine": "qualify_mcp_guidance.py",
        "exit_code": proc.returncode,
        "artifact": str(json_path),
        "payload": payload,
        "verdict": verdict_from_guidance(payload),
        "command": ["python3", "scripts/qualify_mcp_guidance.py", *args],
    }


def run_robustness(plan: InvocationPlan, *, run_dir: Path) -> dict[str, Any]:
    output_root = ensure_dir(run_dir / "robustness-runs")
    log_base = run_dir / "robustness"
    args = [
        "--url",
        plan.url,
        "--project",
        plan.project,
        "--soll-project",
        plan.soll_project,
        "--timeout",
        str(min(plan.timeout, 30)),
        "--output-root",
        str(output_root),
        "--label",
        plan.label or "qualify-mcp",
    ]
    if plan.query:
        args.extend(["--query", plan.query])
    if plan.symbol:
        args.extend(["--symbol", plan.symbol])
    if plan.keep_running:
        args.append("--keep-running")
    if plan.reset_ist:
        args.append("--reset-ist")
    proc = run_py("qualify_mcp_robustness.py", args)
    write_logs(log_base, proc)
    summary_path = latest_summary(output_root)
    payload = parse_json_file(summary_path) if summary_path else None
    return {
        "engine": "qualify_mcp_robustness.py",
        "exit_code": proc.returncode,
        "artifact": str(summary_path) if summary_path else None,
        "payload": payload,
        "verdict": verdict_from_robustness(payload),
        "command": ["python3", "scripts/qualify_mcp_robustness.py", *args],
    }


def summarize_latency_results(results: list[dict[str, Any]], top_slowest: int) -> dict[str, Any]:
    slowest: list[dict[str, Any]] = []
    for result in results:
        payload = result.get("payload") or {}
        if result.get("engine") == "measure_mcp_suite.py":
            slowest.extend(
                payload.get("steps", {})
                .get("core_latency", {})
                .get("summary", {})
                .get("slow_tools_over_1500ms", [])
            )
        elif result.get("engine") == "mcp_validate.py":
            slowest.extend(result.get("slowest_tools") or [])
    slowest = [item for item in slowest if isinstance(item, dict)]
    slowest = sorted(
        slowest,
        key=lambda item: float(item.get("latency_ms", item.get("duration_ms", 0)) or 0.0),
        reverse=True,
    )[:top_slowest]
    return {"slowest_tools": slowest}


def summarize_latency_surfaces(latency_result: dict[str, Any], top_slowest: int) -> dict[str, Any]:
    results = latency_result.get("results", [])
    by_surface: dict[str, dict[str, Any]] = {}
    for result in results:
        surface = result.get("surface")
        if not isinstance(surface, str):
            continue
        entry = by_surface.setdefault(surface, {"verdicts": [], "methods": [], "slowest_tools": []})
        entry["verdicts"].append(result.get("verdict", "fail"))
        if isinstance(result.get("method"), str):
            entry["methods"].append(result["method"])
        entry["slowest_tools"].extend(result.get("slowest_tools") or [])
    for surface, entry in by_surface.items():
        entry["verdict"] = combine_verdicts(entry.pop("verdicts"))
        entry["slowest_tools"] = sorted(
            [
                item
                for item in entry["slowest_tools"]
                if isinstance(item, dict)
            ],
            key=lambda item: float(item.get("latency_ms", item.get("duration_ms", 0)) or 0.0),
            reverse=True,
        )[:top_slowest]
        entry["methods"] = sorted(set(entry["methods"]))
    return by_surface


def combine_verdicts(statuses: list[str]) -> str:
    relevant = [status for status in statuses if status != "skip"]
    if not relevant:
        return "skip"
    if any(status == "fail" for status in relevant):
        return "fail"
    if any(status == "warn" for status in relevant):
        return "warn"
    return "ok"


def build_operator_summary(payload: dict[str, Any]) -> str:
    lines = [
        f"verdict={payload['verdict']} surface={payload['surface']} checks={','.join(payload['checks'])} mutations={payload['mutations']}"
    ]
    for check, verdict in payload["subverdicts"].items():
        lines.append(f"- {check}: {verdict}")
    slowest = payload.get("highlights", {}).get("slowest_tools", [])
    if slowest:
        rendered = ", ".join(
            f"{item.get('tool')}={item.get('latency_ms', item.get('duration_ms'))}ms" for item in slowest[:5]
        )
        lines.append(f"- slowest: {rendered}")
    return "\n".join(lines)


def render_console_summary(payload: dict[str, Any]) -> str:
    lines = [
        "== qualify-mcp ==",
        f"surface={payload['surface']} checks={','.join(payload['checks'])} mode={payload['mode']} mutations={payload['mutations']}",
        f"verdict={payload['verdict']}",
    ]
    for check, verdict in payload["subverdicts"].items():
        lines.append(f"- {check}: {verdict}")
    artifacts = payload.get("artifacts", {})
    if artifacts:
        lines.append("- artifacts:")
        for key, value in artifacts.items():
            lines.append(f"  {key}: {value}")
    slowest = payload.get("highlights", {}).get("slowest_tools", [])
    if slowest:
        lines.append("- slowest tools:")
        for item in slowest[: payload.get("top_slowest", 5)]:
            latency = item.get("latency_ms", item.get("duration_ms"))
            lines.append(f"  {item.get('tool')}: {latency} ms")
    return "\n".join(lines)


def run_quality(plan: InvocationPlan, run_dir: Path) -> dict[str, Any]:
    if plan.surface == "all":
        core = run_quality_surface(plan, target_surface="core", run_dir=run_dir)
        soll = run_quality_surface(plan, target_surface="soll", run_dir=run_dir, scenario_override=plan.scenario_file)
        verdict = combine_verdicts([core["verdict"], soll["verdict"]])
        return {"verdict": verdict, "surfaces": {"core": core, "soll": soll}}
    target = "soll" if plan.surface == "soll" else "core"
    result = run_quality_surface(plan, target_surface=target, run_dir=run_dir, scenario_override=plan.scenario_file)
    return {"verdict": result["verdict"], "surfaces": {target: result}}


def run_latency(plan: InvocationPlan, run_dir: Path, quality_result: dict[str, Any] | None) -> dict[str, Any]:
    results: list[dict[str, Any]] = []
    if plan.surface in {"core", "all"}:
        if plan.mode in {"cold", "both"}:
            cold = run_core_latency_mode(plan, warm_cache=False, run_dir=run_dir, label=f"{plan.label or 'qualify'}-cold")
            cold["surface"] = "core"
            cold["method"] = "suite"
            results.append(cold)
        if plan.mode in {"steady-state", "both"}:
            steady = run_core_latency_mode(
                plan,
                warm_cache=True,
                run_dir=run_dir,
                label=f"{plan.label or 'qualify'}-steady-state",
            )
            steady["surface"] = "core"
            steady["method"] = "suite"
            results.append(steady)
        if not plan.skip_regression:
            candidate = next(
                (Path(result["artifact"]) for result in reversed(results) if result.get("artifact")),
                None,
            )
            if candidate:
                regression = run_regression(plan, candidate_summary=candidate, run_dir=run_dir)
                regression["surface"] = "core"
                regression["method"] = "regression"
                results.append(regression)
    if plan.surface in {"soll", "all"}:
        soll_quality = None
        if quality_result:
            soll_quality = quality_result.get("surfaces", {}).get("soll")
        if soll_quality is None and plan.surface == "soll":
            soll_quality = run_quality_surface(plan, target_surface="soll", run_dir=run_dir, scenario_override=plan.scenario_file)
        if soll_quality is not None:
            derived = summarize_soll_latency_from_validate(soll_quality.get("payload"), plan.top_slowest)
            results.append(
                {
                    "engine": "mcp_validate.py",
                    "artifact": soll_quality.get("artifact"),
                    "payload": soll_quality.get("payload"),
                    "verdict": derived["verdict"],
                    "slowest_tools": derived["slowest_tools"],
                    "derived": True,
                    "mode": "validator-derived",
                    "surface": "soll",
                    "method": "validator-durations",
                }
            )
    verdict = combine_verdicts([result["verdict"] for result in results])
    return {
        "verdict": verdict,
        "results": results,
        "summary": summarize_latency_results(results, plan.top_slowest),
        "surfaces": summarize_latency_surfaces({"results": results}, plan.top_slowest),
    }


def run_guidance_check(plan: InvocationPlan, run_dir: Path) -> dict[str, Any]:
    if plan.surface == "soll":
        return {
            "verdict": "skip",
            "note": "Guidance qualification is only implemented for core/all in phase 1.",
        }
    result = run_guidance(plan, run_dir=run_dir)
    return {"verdict": result["verdict"], "result": result}


def run_qualification(plan: InvocationPlan) -> dict[str, Any]:
    label = sanitize_label(plan.label or f"{plan.surface}-{'-'.join(plan.checks)}")
    run_dir = ensure_dir(plan.artifacts_root / f"{utc_stamp()}-{label}")
    subverdicts: dict[str, str] = {}
    artifacts: dict[str, Any] = {}
    phases: dict[str, Any] = {}

    quality_result: dict[str, Any] | None = None
    if "quality" in plan.checks:
        quality_result = run_quality(plan, run_dir)
        subverdicts["quality"] = quality_result["verdict"]
        phases["quality"] = quality_result
        if plan.surface == "all":
            artifacts["quality_core"] = quality_result["surfaces"]["core"]["artifact"]
            artifacts["quality_soll"] = quality_result["surfaces"]["soll"]["artifact"]
        else:
            only_surface = next(iter(quality_result["surfaces"].values()))
            artifacts["quality"] = only_surface["artifact"]

    if "latency" in plan.checks:
        latency_result = run_latency(plan, run_dir, quality_result)
        subverdicts["latency"] = latency_result["verdict"]
        phases["latency"] = latency_result
        for index, result in enumerate(latency_result["results"], start=1):
            artifacts[f"latency_{index}"] = result.get("artifact")

    if "robustness" in plan.checks:
        robustness_result = run_robustness(plan, run_dir=run_dir)
        subverdicts["robustness"] = robustness_result["verdict"]
        phases["robustness"] = robustness_result
        artifacts["robustness"] = robustness_result.get("artifact")

    if "guidance" in plan.checks:
        guidance_result = run_guidance_check(plan, run_dir)
        subverdicts["guidance"] = guidance_result["verdict"]
        phases["guidance"] = guidance_result
        if isinstance(guidance_result.get("result"), dict):
            artifacts["guidance"] = guidance_result["result"].get("artifact")

    highlights = {
        "slowest_tools": phases.get("latency", {}).get("summary", {}).get("slowest_tools", []),
        "fragile_checks": [name for name, verdict in subverdicts.items() if verdict in {"warn", "fail"}],
    }

    payload = {
        "created_at": utc_iso(),
        "run_dir": str(run_dir),
        "selected_plan": plan_payload(plan),
        "surface": plan.surface,
        "checks": list(plan.checks),
        "mode": plan.mode,
        "mutations": plan.mutations,
        "subverdicts": subverdicts,
        "verdict": combine_verdicts(list(subverdicts.values())),
        "artifacts": artifacts,
        "phases": phases,
        "highlights": highlights,
        "top_slowest": plan.top_slowest,
    }
    payload["highlights"]["operator_summary"] = build_operator_summary(payload)
    return payload


def write_summary(plan: InvocationPlan, payload: dict[str, Any]) -> None:
    run_dir = Path(payload["run_dir"])
    summary_path = run_dir / "summary.json"
    summary_text = json.dumps(payload, ensure_ascii=False, indent=2) + "\n"
    summary_path.write_text(summary_text, encoding="utf-8")
    if plan.json_out:
        plan.json_out.parent.mkdir(parents=True, exist_ok=True)
        plan.json_out.write_text(summary_text, encoding="utf-8")


def exit_code_for_verdict(verdict: str) -> int:
    return 0 if verdict in {"ok", "skip"} else 1


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    plan = normalize_plan(args)
    payload = run_qualification(plan)
    write_summary(plan, payload)
    print(render_console_summary(payload))
    return exit_code_for_verdict(payload["verdict"])


if __name__ == "__main__":
    raise SystemExit(main())
