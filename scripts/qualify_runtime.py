#!/usr/bin/env python3
"""Unified Axon qualification orchestrator."""

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
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

from runtime_contracts import (
    SUPPORTED_MODES,
    mode_contract,
    runtime_authority_contract,
)

sys.path.insert(0, str(Path(__file__).resolve().parent / "lib"))
import gpu_nvml  # noqa: E402

PROJECT_ROOT = Path(__file__).resolve().parents[1]
RUNS_ROOT = PROJECT_ROOT / ".axon" / "qualification-suite-runs"
MIN_RUNTIME_OBSERVATION_SEC = 8


def utc_now_iso() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="seconds")


def sanitize_label(value: str) -> str:
    cleaned = re.sub(r"[^a-zA-Z0-9._-]+", "-", value.strip()).strip("-")
    return cleaned or "run"


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Unified Axon qualification orchestrator.")
    parser.add_argument(
        "--instance",
        choices=["live", "dev"],
        default=os.environ.get("AXON_INSTANCE_KIND", ""),
        help="Target Axon instance. Default: env AXON_INSTANCE_KIND, otherwise dev.",
    )
    parser.add_argument(
        "--profile",
        choices=["smoke", "demo", "full", "ingestion", "retrieval"],
        default="demo",
        help="Qualification profile to run. Default: demo",
    )
    parser.add_argument(
        "--mode",
        choices=sorted(SUPPORTED_MODES),
        default="indexer_graph",
        help="Primary runtime mode when --compare is not used. Default: indexer_graph",
    )
    parser.add_argument(
        "--compare",
        default="",
        help="Comma-separated runtime modes to run sequentially instead of --mode.",
    )
    parser.add_argument("--project", default="BookingSystem", help="Project scope for project-aware MCP tools")
    parser.add_argument("--query", default="booking", help="Default semantic query probe")
    parser.add_argument("--symbol", default="", help="Optional symbol probe for symbol-aware tools")
    parser.add_argument("--soll-project", default="AXO", help="SOLL project code for soll_work_plan probes")
    parser.add_argument("--duration", type=int, default=60, help="Duration in seconds for robustness/ingestion runs")
    parser.add_argument("--warmup", type=int, default=2, help="Warmup in seconds before robustness load")
    parser.add_argument("--concurrency", type=int, default=2, help="Parallel workers for robustness profile")
    parser.add_argument("--timeout", type=int, default=20, help="Timeout in seconds for sub-commands")
    parser.add_argument("--interval", type=int, default=5, help="Sampling interval for ingestion qualification")
    parser.add_argument(
        "--resource-sample-interval-ms",
        type=int,
        default=200,
        help="Host resource sampling cadence in milliseconds during runtime_smoke. Default: 200",
    )
    parser.add_argument("--label", default="qualify-suite", help="Short label for output artifacts")
    parser.add_argument(
        "--output-root",
        default=str(RUNS_ROOT),
        help=f"Directory where run artifacts are stored. Default: {RUNS_ROOT}",
    )
    parser.add_argument("--reset-ist", action="store_true", help="Reset IST before robustness/ingestion sub-runs")
    parser.add_argument("--keep-running", action="store_true", help="Leave the last runtime running after completion")
    parser.add_argument(
        "--gpu-qualified-runtime",
        action="store_true",
        help="Override dev/shared runtime policy to request CUDA for throughput qualification runs.",
    )
    parser.add_argument(
        "--allow-mutations",
        action="store_true",
        help="Allow mutation-capable MCP validation probes. Default: disabled for SOLL safety.",
    )
    parser.add_argument(
        "--enforce-gate",
        action="store_true",
        help="Propagate ingestion truth drift gate when the profile includes ingestion qualification",
    )
    parser.add_argument(
        "--reuse-runtime",
        action="store_true",
        help="Reuse the currently running runtime for ingestion qualification instead of stopping and restarting it.",
    )
    parser.add_argument(
        "--retrieval-corpus",
        default=str(PROJECT_ROOT / "scripts" / "retrieval_context_cases.json"),
        help="Deterministic retrieve_context corpus JSON path",
    )
    # REQ-AXO-113 — fold qualify-dev-*-cold.sh + reset-dev-*-baseline.sh +
    # build-and-qualify-tensorrt-cold.sh into the unified qualify entrypoint.
    parser.add_argument(
        "--cold",
        action="store_true",
        help=(
            "Reset the dev runtime to a cold baseline before qualifying: stop, clean "
            "IST/run roots, restart, wait for stable measurement window. Implies "
            "--reuse-runtime. Dev-only. Indexer-only baseline when --tensorrt or "
            "--mode indexer_*; otherwise split (brain+indexer)."
        ),
    )
    parser.add_argument(
        "--tensorrt",
        action="store_true",
        help=(
            "Enable the TensorRT GPU embedding service envelope: validate manifest, "
            "force the dedicated GPU service on, set VRAM/workspace caps. Combine "
            "with --cold for the equivalent of the legacy qualify-dev-indexer-tensorrt-cold."
        ),
    )
    parser.add_argument(
        "--manifest",
        default="",
        help="TensorRT ORT artifact manifest path (used with --tensorrt). Default: $AXON_ORT_ARTIFACT_MANIFEST or .axon/ort-artifacts/onnxruntime-tensorrt-<set>/current.json",
    )
    parser.add_argument(
        "--build-tensorrt-from-tarball",
        default="",
        help="Optional: before --cold --tensorrt, build the ORT TensorRT artifact from this local tarball.",
    )
    parser.add_argument(
        "--max-vram-used-mb",
        type=int,
        default=None,
        help="TensorRT VRAM budget in MB. Default: 2048. Min: 1024.",
    )
    parser.add_argument(
        "--gpu-admission-vram-used-mb",
        type=int,
        default=None,
        help="Maximum already-used VRAM before GPU batch admission. Default: budget minus max(10%%, 512 MiB).",
    )
    parser.add_argument(
        "--tensorrt-workspace-mb",
        type=int,
        default=None,
        help="TensorRT workspace/memory-pool cap in MB. Default: budget minus 1024 MiB.",
    )
    return parser.parse_args(argv)


def _default_tensorrt_manifest() -> Path:
    cuda_pkg = os.environ.get("AXON_CUDA_PACKAGE_SET", "cudaPackages")
    cuda_label = cuda_pkg.replace("_", "-")
    return PROJECT_ROOT / ".axon" / "ort-artifacts" / f"onnxruntime-tensorrt-{cuda_label}" / "current.json"


def _validate_tensorrt_envelope(args: argparse.Namespace) -> tuple[int, int, int, Path]:
    """Resolve manifest path and VRAM envelope. Raises SystemExit on misuse."""
    if args.manifest:
        manifest_path = Path(args.manifest).resolve()
    elif os.environ.get("AXON_ORT_ARTIFACT_MANIFEST"):
        manifest_path = Path(os.environ["AXON_ORT_ARTIFACT_MANIFEST"]).resolve()
    else:
        manifest_path = _default_tensorrt_manifest()

    if not manifest_path.is_file():
        raise SystemExit(
            f"[qualify] --tensorrt: manifest not found: {manifest_path}\n"
            "Build it first with: axon qualify --cold --tensorrt --build-tensorrt-from-tarball PATH"
        )

    try:
        payload = json.loads(manifest_path.read_text())
    except json.JSONDecodeError as exc:
        raise SystemExit(f"[qualify] --tensorrt: manifest parse error: {exc}")
    if payload.get("provider") != "tensorrt":
        raise SystemExit(
            f"[qualify] --tensorrt: manifest provider must be 'tensorrt', found: {payload.get('provider')!r}"
        )

    validator = PROJECT_ROOT / "scripts" / "lib" / "validate_ort_manifest.py"
    proc = shell(["python3", str(validator), str(manifest_path)], timeout=60)
    if proc.returncode != 0:
        raise SystemExit(
            "[qualify] --tensorrt: manifest validation failed\n"
            + completed_output(proc.stdout)
            + completed_output(proc.stderr)
        )

    max_vram = args.max_vram_used_mb if args.max_vram_used_mb is not None else 2048
    if max_vram < 1024:
        raise SystemExit(f"[qualify] --max-vram-used-mb must be >= 1024, got {max_vram}")
    admission = args.gpu_admission_vram_used_mb
    if admission is None:
        reserve = max(max_vram // 10, 512)
        admission = max_vram - reserve
    if admission >= max_vram:
        raise SystemExit("[qualify] --gpu-admission-vram-used-mb must stay below --max-vram-used-mb")
    workspace = args.tensorrt_workspace_mb
    if workspace is None:
        workspace = max_vram - 1024 if max_vram > 1024 else max_vram
    if workspace < 512:
        raise SystemExit(f"[qualify] --tensorrt-workspace-mb must be >= 512, got {workspace}")
    if workspace > max_vram:
        raise SystemExit("[qualify] --tensorrt-workspace-mb must not exceed --max-vram-used-mb")

    return max_vram, admission, workspace, manifest_path


def perform_cold_reset(args: argparse.Namespace) -> None:
    """REQ-AXO-113 — fold qualify-dev-*-cold.sh + reset-dev-*-baseline.sh into qualify --cold."""
    if args.instance != "dev":
        raise SystemExit("[qualify] --cold is dev-only; pass --instance dev")

    indexer_only = args.tensorrt or args.mode in {"indexer_full", "indexer_vector", "indexer_graph"}

    # Step 1: optional TensorRT artifact build
    if args.build_tensorrt_from_tarball:
        tarball = Path(args.build_tensorrt_from_tarball).resolve()
        if not tarball.is_file():
            raise SystemExit(f"[qualify] --build-tensorrt-from-tarball: tarball not found: {tarball}")
        manifest_path = (
            Path(args.manifest).resolve() if args.manifest else _default_tensorrt_manifest()
        )
        env = os.environ.copy()
        env["AXON_ORT_ARTIFACT_MANIFEST"] = str(manifest_path)
        env["TENSORRT_LOCAL_TARBALL"] = str(tarball)
        print(f"[qualify] building TensorRT ORT artifact from {tarball}")
        proc = shell(
            ["bash", str(PROJECT_ROOT / "scripts" / "build_ort_tensorrt_artifact.sh")],
            env=env,
            timeout=1800,
        )
        if proc.returncode != 0:
            out = completed_output(proc.stdout) + completed_output(proc.stderr)
            raise SystemExit(f"[qualify] TensorRT artifact build failed (rc={proc.returncode}):\n{out}")
        if not manifest_path.is_file():
            raise SystemExit(f"[qualify] TensorRT manifest missing after build: {manifest_path}")
        args.manifest = str(manifest_path)

    # Step 2: TensorRT envelope (validate manifest + export env vars)
    if args.tensorrt:
        max_vram, admission, workspace, manifest_path = _validate_tensorrt_envelope(args)
        os.environ["AXON_ORT_ARTIFACT_MANIFEST"] = str(manifest_path)
        os.environ["AXON_GPU_EMBED_SERVICE_ENABLED"] = "1"
        os.environ["AXON_GPU_EMBED_SERVICE_RECYCLE_EVERY_BATCH"] = "0"
        os.environ["AXON_GPU_EMBED_SERVICE_TENSORRT"] = "1"
        os.environ.setdefault("AXON_GPU_TELEMETRY_BACKEND", "nvml")
        os.environ.setdefault("AXON_NVML_LIBRARY_PATH", "/usr/lib/wsl/lib/libnvidia-ml.so.1")
        os.environ["AXON_OPT_MAX_VRAM_USED_MB"] = str(max_vram)
        os.environ["AXON_CUDA_MEMORY_SOFT_LIMIT_MB"] = str(max_vram)
        os.environ["AXON_CUDA_MEMORY_LIMIT_MB"] = str(workspace)
        os.environ["AXON_GPU_PRIMARY_WORKER_MAX_USED_MB"] = str(admission)
        os.environ.setdefault("AXON_GPU_TELEMETRY_CACHE_TTL_MS", "250")
        os.environ.setdefault("AXON_TENSORRT_OVERSHOOT_MB", "7900")
        os.environ.setdefault("AXON_QUALIFY_STOP_ON_VRAM_OVERSHOOT", "1")
        print(
            f"[qualify] TensorRT envelope: max_vram={max_vram} admission={admission} "
            f"workspace={workspace} manifest={manifest_path}"
        )

    # Step 3: dev baseline reset (stop + clean + start + wait)
    baseline_kind = "indexer" if indexer_only else "split"
    print(f"[qualify] --cold: resetting dev baseline ({baseline_kind})")

    if indexer_only:
        baseline_script = (
            "set -euo pipefail; "
            "source scripts/lib/dev-baseline.sh; "
            "dev_baseline_require_dev_instance; "
            "dev_baseline_stop_split; "
            "dev_baseline_clean_state; "
            "dev_baseline_start_split; "
            "dev_baseline_wait_for_indexer_measurement_window 240"
        )
    else:
        baseline_script = (
            "set -euo pipefail; "
            "source scripts/lib/dev-baseline.sh; "
            "dev_baseline_require_dev_instance; "
            "dev_baseline_stop_split; "
            "dev_baseline_clean_state; "
            "dev_baseline_start_split; "
            "dev_baseline_wait_for_stable_measurement_window 240"
        )

    proc = shell(["bash", "-c", baseline_script], env=os.environ.copy(), timeout=600)
    if proc.returncode != 0:
        out = completed_output(proc.stdout) + completed_output(proc.stderr)
        raise SystemExit(f"[qualify] cold baseline reset failed (rc={proc.returncode}):\n{out}")
    print(f"[qualify] cold baseline ready ({baseline_kind})")


def normalize_instance(instance: str) -> str:
    normalized = (instance or "").strip().lower()
    if normalized in {"live", "dev"}:
        return normalized
    return "dev"


def default_mcp_url_for_instance(instance: str) -> str:
    if instance == "live":
        return "http://127.0.0.1:44129/mcp"
    return "http://127.0.0.1:44139/mcp"


def profile_steps(profile: str) -> list[str]:
    if profile == "smoke":
        return ["runtime_smoke", "mcp_validate"]
    if profile == "demo":
        return ["runtime_smoke", "mcp_validate", "mcp_robustness"]
    if profile == "full":
        return ["runtime_smoke", "mcp_validate", "retrieval_qualify", "mcp_robustness", "ingestion_qualify"]
    if profile == "ingestion":
        return ["ingestion_qualify"]
    if profile == "retrieval":
        return ["runtime_smoke", "mcp_validate", "retrieval_qualify"]
    raise ValueError(f"Unsupported profile: {profile}")


def normalize_modes(mode: str, compare: str) -> list[str]:
    if compare.strip():
        modes = [item.strip() for item in compare.split(",") if item.strip()]
    else:
        modes = [mode]
    seen: list[str] = []
    for item in modes:
        if item not in SUPPORTED_MODES:
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


def runtime_authority_contract_matches(runtime_truth_summary: dict[str, Any]) -> bool:
    contract = runtime_authority_contract(runtime_truth_summary.get("process_role"))
    return all(runtime_truth_summary.get(key) == value for key, value in contract.items())


def runtime_authority_restored(
    runtime_truth_summary: dict[str, Any], canonical_truth_restored: bool
) -> bool:
    if not canonical_truth_restored:
        return False
    contract = runtime_authority_contract(runtime_truth_summary.get("process_role"))
    return (
        runtime_truth_summary.get("public_mcp_authority") == contract["public_mcp_authority"]
        and runtime_truth_summary.get("soll_writer_authority") == contract["soll_writer_authority"]
        and runtime_truth_summary.get("ist_writer_authority") == contract["ist_writer_authority"]
    )


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


def summarize_runtime_quiescent(mode_reports: list[dict[str, Any]]) -> dict[str, Any]:
    rows: list[dict[str, Any]] = []
    statuses: list[str] = []
    for report in mode_reports:
        runtime_quiescent = report.get("runtime_quiescent", {})
        if not isinstance(runtime_quiescent, dict) or not runtime_quiescent:
            continue
        verdict = runtime_quiescent.get("qualification_verdict")
        available = runtime_quiescent.get("available")
        status = quiescent_step_status(runtime_quiescent)
        rows.append(
            {
                "mode": report.get("mode"),
                "instance": report.get("instance"),
                "status": status,
                "qualification_verdict": verdict,
                "available": available,
                "reason": runtime_quiescent.get("qualification_reason") or runtime_quiescent.get("reason"),
                "blocking_factors": runtime_quiescent.get("blocking_factors", []),
                "operator_focus": runtime_quiescent.get("operator_focus"),
                "recommended_next_measurement": runtime_quiescent.get("recommended_next_measurement"),
                "throughput_observed": runtime_quiescent.get("throughput_observed"),
                "throughput_recommendation": runtime_quiescent.get("throughput_recommendation"),
            }
        )
        statuses.append(status)
    if not rows:
        return {}
    overall = combine_step_statuses([{"status": status} for status in statuses])
    return {
        "overall_status": overall,
        "modes": rows,
    }


def command_env(
    mode: str, instance: str, mcp_url: str, gpu_qualified_runtime: bool = False
) -> dict[str, str]:
    env = os.environ.copy()
    contract = mode_contract(mode)
    env["AXON_INSTANCE_KIND"] = instance
    env["AXON_MCP_URL"] = mcp_url
    env["AXON_RUNTIME_SHADOW_ROLE"] = contract["shadow_role"]
    env["AXON_SPLIT_SHADOW_ONLY"] = "1" if contract["shadow_only"] else "0"
    if mode == "indexer_full":
        env["AXON_ENABLE_AUTONOMOUS_INGESTOR"] = "true"
        env["AXON_RUNTIME_PROFILE"] = "full_autonomous"
    if gpu_qualified_runtime:
        env["AXON_GPU_ACCESS_POLICY"] = "shared"
        env["AXON_EMBEDDING_PROVIDER"] = "cuda"
    return env


def shell(
    args: list[str],
    *,
    check: bool = False,
    env: dict[str, str] | None = None,
    timeout: int | None = None,
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        args,
        cwd=PROJECT_ROOT,
        text=True,
        capture_output=True,
        check=check,
        env=env,
        timeout=timeout,
    )


def completed_output(value: Any) -> str:
    if value is None:
        return ""
    if isinstance(value, bytes):
        return value.decode("utf-8", errors="replace")
    return str(value)


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


def call_tool(url: str, timeout: int, tool_name: str, arguments: dict[str, Any]) -> dict[str, Any]:
    payload = {
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": tool_name,
            "arguments": arguments,
        },
    }
    return rpc_call(url, payload, timeout)


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


def fetch_status_snapshot(url: str, timeout: int = 20) -> dict[str, Any]:
    response = call_tool(url, timeout, "status", {"mode": "full"})
    result = response.get("result", {})
    if not isinstance(result, dict):
        raise RuntimeError("status returned invalid result payload")
    data = result.get("data")
    if not isinstance(data, dict):
        raise RuntimeError("status missing data payload")
    return data


def resolve_effective_mcp_url(start_log: str, fallback_url: str) -> str:
    for line in start_log.splitlines():
        line = line.strip()
        if line.startswith("MCP Server: "):
            candidate = line.removeprefix("MCP Server: ").strip()
            if candidate:
                return candidate
    return fallback_url


def summarize_quiescent_status(status_data: dict[str, Any]) -> dict[str, Any]:
    runtime_authority = status_data.get("runtime_authority", {})
    if not isinstance(runtime_authority, dict):
        return {"available": False, "reason": "missing_runtime_authority"}
    quiescent = runtime_authority.get("quiescent_state", {})
    if not isinstance(quiescent, dict):
        return {"available": False, "reason": "missing_quiescent_state"}
    diagnosis = quiescent.get("diagnosis", {})
    wake_activity = quiescent.get("wake_activity", {})
    lane_liveness = quiescent.get("lane_liveness", {})
    backlog_drain = quiescent.get("backlog_drain", {})
    if not isinstance(diagnosis, dict):
        diagnosis = {}
    if not isinstance(wake_activity, dict):
        wake_activity = {}
    if not isinstance(lane_liveness, dict):
        lane_liveness = {}
    if not isinstance(backlog_drain, dict):
        backlog_drain = {}
    summary = {
        "available": True,
        "state": quiescent.get("state"),
        "authority_state": quiescent.get("authority_state"),
        "wake_contract_state": quiescent.get("wake_contract_state"),
        "qualification_verdict": diagnosis.get("qualification_verdict"),
        "qualification_reason": diagnosis.get("qualification_reason"),
        "measurement_readiness": diagnosis.get("measurement_readiness"),
        "actionable_now": diagnosis.get("actionable_now"),
        "blocking_factors": diagnosis.get("blocking_factors", []),
        "operator_focus": diagnosis.get("operator_focus"),
        "focus_recommendation": diagnosis.get("focus_recommendation"),
        "recommended_next_measurement": diagnosis.get("recommended_next_measurement"),
        "wake_noise_level": diagnosis.get("wake_noise_level"),
        "confidence": diagnosis.get("confidence"),
        "dominant_wake_source": wake_activity.get("dominant_wake_source"),
        "last_wake_source": wake_activity.get("last_wake_source"),
        "last_background_wake_detail": wake_activity.get("last_background_wake_detail"),
        "dominant_background_wake_detail": wake_activity.get("dominant_background_wake_detail"),
        "wakeups_last_60s": wake_activity.get("wakeups_last_60s"),
        "resume_latency_p95_ms": wake_activity.get("resume_latency_p95_ms"),
        "useful_resume_latency_p95_ms": wake_activity.get("useful_resume_latency_p95_ms"),
        "last_quiescent_exit_reason": wake_activity.get("last_quiescent_exit_reason"),
        "background_wake_memory_reclaimer_total": wake_activity.get(
            "background_wake_memory_reclaimer_total"
        ),
        "background_wake_shadow_optimizer_total": wake_activity.get(
            "background_wake_shadow_optimizer_total"
        ),
        "background_wake_runtime_trace_total": wake_activity.get(
            "background_wake_runtime_trace_total"
        ),
        "background_wake_reader_refresh_total": wake_activity.get(
            "background_wake_reader_refresh_total"
        ),
        "background_wake_autonomous_ingestor_total": wake_activity.get(
            "background_wake_autonomous_ingestor_total"
        ),
        "background_wake_ingress_promoter_total": wake_activity.get(
            "background_wake_ingress_promoter_total"
        ),
        "background_wake_federation_orchestrator_total": wake_activity.get(
            "background_wake_federation_orchestrator_total"
        ),
        "vector_lane_state": lane_liveness.get("vector_lane_state"),
        "vector_lane_last_success_age_ms": lane_liveness.get("vector_lane_last_success_age_ms"),
        "backlog_drain": backlog_drain,
    }
    if all(value is None for key, value in summary.items() if key not in {"available", "blocking_factors"}):
        summary["available"] = False
        summary["reason"] = "quiescent_surface_empty"
    return summary


def summarize_runtime_truth(status_data: dict[str, Any]) -> dict[str, Any]:
    runtime_authority = status_data.get("runtime_authority", {})
    if not isinstance(runtime_authority, dict):
        return {"available": False, "reason": "missing_runtime_authority"}
    runtime_state = runtime_authority.get("runtime_state", {})
    if not isinstance(runtime_state, dict):
        return {"available": False, "reason": "missing_runtime_state"}
    indexer_feed = runtime_state.get("indexer_feed", {})
    if not isinstance(indexer_feed, dict):
        indexer_feed = {}
    ist_snapshot = runtime_state.get("ist_snapshot", {})
    if not isinstance(ist_snapshot, dict):
        ist_snapshot = {}
    summary = {
        "available": True,
        "truth_status": status_data.get("truth_status"),
        "runtime_release_version": status_data.get("runtime_version", {}).get("release_version")
        if isinstance(status_data.get("runtime_version"), dict)
        else None,
        "runtime_build_id": status_data.get("runtime_version", {}).get("build_id")
        if isinstance(status_data.get("runtime_version"), dict)
        else None,
        "runtime_install_generation": status_data.get("runtime_version", {}).get("install_generation")
        if isinstance(status_data.get("runtime_version"), dict)
        else None,
        "process_role": runtime_state.get("process_role"),
        "public_mcp_authority": runtime_state.get("public_mcp_authority"),
        "soll_writer_authority": runtime_state.get("soll_writer_authority"),
        "ist_writer_authority": runtime_state.get("ist_writer_authority"),
        "brain_ready": runtime_state.get("brain_ready"),
        "indexer_ready": runtime_state.get("indexer_ready"),
        "system_converged": runtime_state.get("system_converged"),
        "indexer_feed_state": indexer_feed.get("state"),
        "indexer_feed_stale": indexer_feed.get("stale"),
        "indexer_feed_degraded_reason": indexer_feed.get("degraded_reason"),
        "indexer_feed_last_good_payload_at_ms": indexer_feed.get("last_good_payload_at_ms"),
        "ist_snapshot_state": ist_snapshot.get("state"),
        "ist_snapshot_stale": ist_snapshot.get("stale"),
        "ist_snapshot_degraded_reason": ist_snapshot.get("degraded_reason"),
        "ist_snapshot_unsafe_read": ist_snapshot.get("unsafe_read"),
        "ist_snapshot_trust_boundary": ist_snapshot.get("trust_boundary"),
        "ist_snapshot_computed_by": ist_snapshot.get("computed_by"),
        "degraded_notes": status_data.get("availability", {}).get("degraded_notes", []),
    }
    if all(value is None for key, value in summary.items() if key != "available"):
        summary["available"] = False
        summary["reason"] = "runtime_truth_surface_empty"
    return summary


def expected_runtime_version_for_instance(instance: str) -> dict[str, Any]:
    if instance == "live":
        manifest_path = PROJECT_ROOT / ".axon" / "live-release" / "current.json"
        if manifest_path.exists():
            try:
                manifest = json.loads(manifest_path.read_text())
            except Exception:
                return {"source": "live_manifest", "available": False}
            runtime = manifest.get("runtime_version", {})
            if isinstance(runtime, dict):
                return {
                    "source": "live_manifest",
                    "available": True,
                    "release_version": runtime.get("release_version"),
                    "build_id": runtime.get("build_id"),
                    "install_generation": runtime.get("install_generation"),
                }
        return {"source": "live_manifest", "available": False}

    package_version = None
    cargo_manifest = PROJECT_ROOT / "src" / "axon-core" / "Cargo.toml"
    if cargo_manifest.exists():
        for line in cargo_manifest.read_text().splitlines():
            if line.startswith("version = "):
                package_version = line.split('"')[1]
                break
    build_id = package_version or "unknown"
    try:
        build_id = (
            subprocess.run(
                ["git", "-C", str(PROJECT_ROOT), "describe", "--tags", "--always", "--dirty"],
                check=True,
                capture_output=True,
                text=True,
            ).stdout.strip()
            or build_id
        )
    except Exception:
        pass
    return {
        "source": "workspace",
        "available": package_version is not None,
        "release_version": package_version,
        "build_id": build_id,
        "install_generation": "workspace",
    }


def summarize_runtime_guardrails(mode: str, instance: str, runtime_truth_summary: dict[str, Any]) -> dict[str, Any]:
    shadow_mode = bool(mode_contract(mode)["shadow_only"])
    expected_runtime_version = expected_runtime_version_for_instance(instance)
    version_identity_verified = False
    if expected_runtime_version.get("available"):
        version_identity_verified = (
            runtime_truth_summary.get("runtime_release_version") == expected_runtime_version.get("release_version")
            and runtime_truth_summary.get("runtime_build_id") == expected_runtime_version.get("build_id")
            and runtime_truth_summary.get("runtime_install_generation")
            == expected_runtime_version.get("install_generation")
        )
    available = isinstance(runtime_truth_summary, dict) and runtime_truth_summary.get("available") is not False
    canonical_truth_restored = bool(available)
    if canonical_truth_restored:
        canonical_truth_restored = (
            runtime_truth_summary.get("truth_status") == "canonical"
            and runtime_truth_summary.get("brain_ready") is True
            and runtime_truth_summary.get("indexer_ready") is True
            and runtime_truth_summary.get("system_converged") is True
            and runtime_truth_summary.get("indexer_feed_state") == "fresh"
            and runtime_truth_summary.get("indexer_feed_stale") is False
            and runtime_truth_summary.get("indexer_feed_degraded_reason") is None
            and runtime_truth_summary.get("ist_snapshot_state") == "fresh"
            and runtime_truth_summary.get("ist_snapshot_stale") is False
            and runtime_truth_summary.get("ist_snapshot_unsafe_read") is False
            and runtime_truth_summary.get("ist_snapshot_degraded_reason") is None
            and version_identity_verified
            and runtime_authority_contract_matches(runtime_truth_summary)
        )
    canonical_authority_restored = runtime_authority_restored(
        runtime_truth_summary, canonical_truth_restored
    )
    promotion_allowed = canonical_authority_restored and not shadow_mode
    rollback_path_state = "green" if promotion_allowed else "red"
    return {
        "shadow_mode": shadow_mode,
        "expected_version_source": expected_runtime_version.get("source"),
        "version_identity_verified": version_identity_verified,
        "canonical_truth_restored": canonical_truth_restored,
        "canonical_authority_restored": canonical_authority_restored,
        "promotion_allowed": promotion_allowed,
        "rollback_path_state": rollback_path_state,
        "cutover_blocked": not promotion_allowed,
    }


def runtime_truth_requires_warn(
    runtime_truth_summary: dict[str, Any], guardrails_summary: dict[str, Any] | None = None
) -> bool:
    if not isinstance(runtime_truth_summary, dict) or runtime_truth_summary.get("available") is False:
        return True
    if isinstance(guardrails_summary, dict) and guardrails_summary.get("promotion_allowed") is False:
        return True
    indexer_feed_state = runtime_truth_summary.get("indexer_feed_state")
    if indexer_feed_state is None or indexer_feed_state != "fresh":
        return True
    if runtime_truth_summary.get("indexer_feed_stale") is True:
        return True
    if runtime_truth_summary.get("indexer_feed_degraded_reason"):
        return True
    ist_state = runtime_truth_summary.get("ist_snapshot_state")
    if ist_state is None or ist_state != "fresh":
        return True
    if runtime_truth_summary.get("ist_snapshot_unsafe_read") is True:
        return True
    if runtime_truth_summary.get("ist_snapshot_degraded_reason"):
        return True
    return False


def quiescent_backlog_drain(status_data: dict[str, Any]) -> dict[str, Any]:
    runtime_authority = status_data.get("runtime_authority", {})
    if not isinstance(runtime_authority, dict):
        return {}
    quiescent = runtime_authority.get("quiescent_state", {})
    if not isinstance(quiescent, dict):
        return {}
    backlog_drain = quiescent.get("backlog_drain", {})
    if not isinstance(backlog_drain, dict):
        return {}
    return backlog_drain


def gpu_vector_lease_diagnostics(status_data: dict[str, Any]) -> dict[str, Any]:
    runtime_authority = status_data.get("runtime_authority", {})
    if not isinstance(runtime_authority, dict):
        return {}
    lane_parameters = runtime_authority.get("lane_parameters", {})
    if not isinstance(lane_parameters, dict):
        return {}
    lease = lane_parameters.get("gpu_vector_lease", {})
    if not isinstance(lease, dict):
        return {}
    return lease


def should_probe_semantic_burn_rate(status_data: dict[str, Any], quiescent_summary: dict[str, Any]) -> bool:
    if quiescent_summary.get("recommended_next_measurement") in {
        "measure_semantic_backlog_burn_rate",
        "extend_semantic_burn_rate_probe",
    }:
        return True
    backlog_drain = quiescent_backlog_drain(status_data)
    burn_rate = backlog_drain.get("burn_rate", {}) if isinstance(backlog_drain.get("burn_rate"), dict) else {}
    lease = gpu_vector_lease_diagnostics(status_data)
    effective_backlog = int(burn_rate.get("effective_semantic_backlog_depth", 0) or 0)
    return (
        backlog_drain.get("provider_effective") == "cuda"
        and lease.get("owned_by_current_instance") is True
        and effective_backlog > 0
        and burn_rate.get("state") != "measurable_progress"
    )


def measure_semantic_backlog_burn_rate(
    url: str, initial_status: dict[str, Any], probe_window_sec: int = 20, timeout: int = 20
) -> dict[str, Any]:
    before = quiescent_backlog_drain(initial_status)
    before_burn_rate = before.get("burn_rate", {}) if isinstance(before.get("burn_rate"), dict) else {}
    time.sleep(probe_window_sec)
    after_status = fetch_status_snapshot(url, timeout=timeout)
    after = quiescent_backlog_drain(after_status)
    after_burn_rate = after.get("burn_rate", {}) if isinstance(after.get("burn_rate"), dict) else {}

    before_chunks_total = int(before.get("chunks_embedded_total", 0) or 0)
    after_chunks_total = int(after.get("chunks_embedded_total", 0) or 0)
    before_files_total = int(before.get("files_completed_total", 0) or 0)
    after_files_total = int(after.get("files_completed_total", 0) or 0)
    before_backlog = int(before_burn_rate.get("effective_semantic_backlog_depth", 0) or 0)
    after_backlog = int(after_burn_rate.get("effective_semantic_backlog_depth", 0) or 0)

    delta_chunks_total = max(0, after_chunks_total - before_chunks_total)
    delta_files_total = max(0, after_files_total - before_files_total)
    delta_backlog_depth = after_backlog - before_backlog
    measured_chunks_per_minute = (delta_chunks_total * 60.0) / probe_window_sec
    measured_files_per_minute = (delta_files_total * 60.0) / probe_window_sec

    after_semantic_health = after.get("semantic_health")
    after_lane_state = after.get("vector_lane_state")
    after_burn_state = after_burn_rate.get("state")
    if delta_chunks_total > 0 or delta_files_total > 0 or after_backlog < before_backlog:
        probe_state = "measurable_progress"
        recommendation = "track_burn_rate_until_backlog_turns_down"
    elif after_semantic_health == "healthy_draining" and after_lane_state == "healthy":
        probe_state = "still_warming_or_long_batch"
        recommendation = "extend_probe_window_before_calling_stall"
    elif after_semantic_health == "underfed":
        probe_state = "underfed"
        recommendation = "repair_semantic_feed_before_idle_tuning"
    elif after_semantic_health == "stalled":
        probe_state = "stalled"
        recommendation = "repair_semantic_lane_before_idle_tuning"
    else:
        probe_state = after_burn_state or "uncertain"
        recommendation = "observe_another_probe_window"

    return {
        "probe_window_sec": probe_window_sec,
        "state": probe_state,
        "recommendation": recommendation,
        "before": {
            "chunks_embedded_total": before_chunks_total,
            "files_completed_total": before_files_total,
            "effective_semantic_backlog_depth": before_backlog,
            "burn_rate_state": before_burn_rate.get("state"),
        },
        "after": {
            "chunks_embedded_total": after_chunks_total,
            "files_completed_total": after_files_total,
            "effective_semantic_backlog_depth": after_backlog,
            "burn_rate_state": after_burn_rate.get("state"),
            "semantic_health": after_semantic_health,
            "vector_lane_state": after_lane_state,
        },
        "delta": {
            "chunks_embedded_total": delta_chunks_total,
            "files_completed_total": delta_files_total,
            "effective_semantic_backlog_depth": delta_backlog_depth,
            "measured_chunks_per_minute": measured_chunks_per_minute,
            "measured_files_per_minute": measured_files_per_minute,
        },
        "status_after_probe": after_status,
    }


def quiescent_step_status(quiescent_summary: dict[str, Any]) -> str:
    if not isinstance(quiescent_summary, dict):
        return "warn"
    if quiescent_summary.get("available") is False:
        return "warn"
    throughput_probe = quiescent_summary.get("throughput_probe", {})
    if not isinstance(throughput_probe, dict):
        throughput_probe = {}
    if (
        quiescent_summary.get("qualification_reason") == "blocked_by_healthy_semantic_drain"
        and quiescent_summary.get("throughput_observed") is True
        and throughput_probe.get("state") in {"measurable_progress", "measurable_progress_after_extended_probe"}
    ):
        return "pass"
    verdict = quiescent_summary.get("qualification_verdict")
    if verdict == "pass":
        return "pass"
    if verdict in {"watch", "blocked"}:
        return "warn"
    return "pass"


def mode_flag(mode: str) -> str:
    return {
        "brain_only": "--brain-only",
        "indexer_graph": "--indexer-graph",
        "indexer_vector": "--indexer-vector",
        "indexer_full": "--indexer-full",
    }[mode]


def shadow_role_for_mode(mode: str) -> str:
    return mode_contract(mode)["shadow_role"]


def start_command_for_mode(mode: str) -> list[str]:
    contract = mode_contract(mode)
    if contract["start_script"]:
        return ["bash", contract["start_script"]]
    return ["bash", "scripts/start.sh", mode_flag(mode), "--skip-mcp-tests"]


def mcp_robustness_supported_mode(mode: str) -> str | None:
    if not mode_contract(mode)["shadow_only"]:
        return mode
    return None


def write_json(path: Path, payload: Any) -> None:
    path.write_text(json.dumps(payload, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")


def read_proc_stat() -> tuple[int, int] | None:
    try:
        first_line = Path("/proc/stat").read_text(encoding="utf-8").splitlines()[0]
    except Exception:
        return None
    parts = first_line.split()
    if not parts or parts[0] != "cpu" or len(parts) < 5:
        return None
    values = [int(value) for value in parts[1:]]
    idle = values[3] + (values[4] if len(values) > 4 else 0)
    total = sum(values)
    return total, idle


def read_meminfo() -> dict[str, int]:
    result: dict[str, int] = {}
    try:
        for line in Path("/proc/meminfo").read_text(encoding="utf-8").splitlines():
            if ":" not in line:
                continue
            key, raw_value = line.split(":", 1)
            value = raw_value.strip().split()[0]
            result[key] = int(value) * 1024
    except Exception:
        return {}
    return result


def read_gpu_sample() -> dict[str, Any]:
    """NVML-only GPU sample (REQ-AXO-902085) via the shared helper.

    Replaces the nvidia-smi subprocess probe; maps the canonical ``gpu_nvml``
    keys onto this script's ``gpu_*`` sample schema. Never raises.
    """
    status = gpu_nvml.gpu_status()
    if not status.get("available"):
        return {"available": False, "reason": status.get("error", "nvml_unavailable")}
    return {
        "available": True,
        "gpu_util_pct": status.get("utilization_gpu"),
        "gpu_mem_used_mb": status.get("memory_used_mb"),
        "gpu_mem_free_mb": status.get("memory_free_mb"),
        "gpu_mem_total_mb": status.get("memory_total_mb"),
        "gpu_temp_c": status.get("temperature_c"),
        "gpu_power_w": status.get("power_w"),
        "gpu_power_limit_w": status.get("power_limit_w"),
    }


def percentile(values: list[float], pct: float) -> float | None:
    if not values:
        return None
    if len(values) == 1:
        return values[0]
    ordered = sorted(values)
    index = (len(ordered) - 1) * pct
    lower = int(index)
    upper = min(lower + 1, len(ordered) - 1)
    if lower == upper:
        return ordered[lower]
    fraction = index - lower
    return ordered[lower] * (1.0 - fraction) + ordered[upper] * fraction


def parse_optional_float(raw: str) -> float | None:
    cleaned = raw.strip()
    if not cleaned or cleaned.upper() in {"N/A", "[N/A]"}:
        return None
    return float(cleaned)


def summarize_numeric_series(samples: list[dict[str, Any]], key: str) -> dict[str, Any]:
    values = [float(sample[key]) for sample in samples if isinstance(sample.get(key), (int, float))]
    if not values:
        return {}
    return {
        "min": min(values),
        "p50": percentile(values, 0.50),
        "p95": percentile(values, 0.95),
        "max": max(values),
        "avg": sum(values) / len(values),
        "samples": len(values),
    }


def summarize_progression_series(samples: list[dict[str, Any]], key: str) -> dict[str, Any]:
    values = [float(sample[key]) for sample in samples if isinstance(sample.get(key), (int, float))]
    if not values:
        return {}
    return {
        "first": values[0],
        "last": values[-1],
        "delta": values[-1] - values[0],
        "min": min(values),
        "max": max(values),
        "samples": len(values),
    }


def gpu_sawtooth_summary(samples: list[dict[str, Any]]) -> dict[str, Any]:
    values = [float(sample["gpu_util_pct"]) for sample in samples if isinstance(sample.get("gpu_util_pct"), (int, float))]
    if len(values) < 3:
        return {"samples": len(values)}
    deltas = [values[idx] - values[idx - 1] for idx in range(1, len(values))]
    sign_changes = 0
    previous_sign = 0
    for delta in deltas:
        sign = 1 if delta > 0 else -1 if delta < 0 else 0
        if sign != 0 and previous_sign != 0 and sign != previous_sign:
            sign_changes += 1
        if sign != 0:
            previous_sign = sign
    amplitudes = [abs(delta) for delta in deltas]
    return {
        "samples": len(values),
        "sign_changes": sign_changes,
        "avg_step_abs_pct": sum(amplitudes) / len(amplitudes) if amplitudes else 0.0,
        "max_step_abs_pct": max(amplitudes) if amplitudes else 0.0,
    }


def summarize_controller_modes(samples: list[dict[str, Any]]) -> dict[str, Any]:
    counts: dict[str, int] = {}
    for sample in samples:
        value = sample.get("controller_state")
        if not value:
            continue
        key = str(value)
        counts[key] = counts.get(key, 0) + 1
    if not counts:
        return {}
    dominant = max(counts.items(), key=lambda item: item[1])[0]
    return {"counts": counts, "dominant": dominant}


def summarize_categorical_series(samples: list[dict[str, Any]], key: str) -> dict[str, Any]:
    counts: dict[str, int] = {}
    for sample in samples:
        value = sample.get(key)
        if not value:
            continue
        bucket = str(value)
        counts[bucket] = counts.get(bucket, 0) + 1
    if not counts:
        return {}
    dominant = max(counts.items(), key=lambda item: item[1])[0]
    return {"counts": counts, "dominant": dominant}


def progression_delta(summary: dict[str, Any]) -> float:
    return float(summary.get("delta") or 0.0)


def per_minute(delta: float, duration_ms: int) -> float:
    if duration_ms <= 0:
        return 0.0
    return (delta * 60_000.0) / float(duration_ms)


def summarize_conversion_rates(summary: dict[str, Any]) -> dict[str, Any]:
    duration_ms = int(summary.get("duration_ms") or 0)
    pipeline = summary.get("pipeline_buffer", {})
    if not isinstance(pipeline, dict) or duration_ms <= 0:
        return {}

    buffered_to_persisted_delta = max(
        progression_delta(pipeline.get("ingress_promoted_total", {})),
        progression_delta(pipeline.get("ingress_durably_persisted_total", {})),
        progression_delta(pipeline.get("persisted_file_current", {})),
        progression_delta(pipeline.get("persisted_file_pending_current", {})),
    )
    persisted_to_graph_ready_delta = progression_delta(pipeline.get("graph_ready_current", {}))
    graph_ready_to_vector_ready_delta = progression_delta(pipeline.get("vector_ready_current", {}))

    return {
        "buffered_to_persisted_per_min": round(per_minute(buffered_to_persisted_delta, duration_ms), 2),
        "persisted_to_graph_ready_per_min": round(
            per_minute(max(0.0, persisted_to_graph_ready_delta), duration_ms), 2
        ),
        "graph_ready_to_vector_ready_per_min": round(
            per_minute(max(0.0, graph_ready_to_vector_ready_delta), duration_ms), 2
        ),
    }


def diagnose_conversion_pipeline(summary: dict[str, Any]) -> dict[str, Any]:
    duration_ms = int(summary.get("duration_ms") or 0)
    pipeline = summary.get("pipeline_buffer", {})
    if not isinstance(pipeline, dict) or duration_ms <= 0:
        return {
            "verdict": "insufficient_observation_window",
            "reason": "runtime_window_too_short_for_conversion_diagnosis",
        }

    rates = summarize_conversion_rates(summary)
    scan_delta = progression_delta(pipeline.get("scan_buffered_current", {}))
    pending_delta = progression_delta(pipeline.get("persisted_file_pending_current", {}))
    persisted_delta = progression_delta(pipeline.get("persisted_file_current", {}))
    graph_ready_delta = progression_delta(pipeline.get("graph_ready_current", {}))
    vector_ready_delta = progression_delta(pipeline.get("vector_ready_current", {}))
    flush_delta = progression_delta(pipeline.get("ingress_flush_count", {}))
    durable_persisted_delta = progression_delta(
        pipeline.get("ingress_durably_persisted_total", {})
    )
    excluded_from_pending_delta = progression_delta(
        pipeline.get("ingress_excluded_from_pending_total", {})
    )
    admission_block = pipeline.get("admission_blocking_authority", {}).get("dominant")
    graph_block = pipeline.get("graph_blocking_authority", {}).get("dominant")
    vector_block = pipeline.get("vector_blocking_authority", {}).get("dominant")

    if (
        rates.get("buffered_to_persisted_per_min", 0.0) <= 0.0
        and flush_delta > 0.0
        and scan_delta < 0.0
        and persisted_delta <= 0.0
        and pending_delta <= 0.0
    ):
        return {
            "verdict": "persistence_limited",
            "reason": admission_block
            or "buffered_discovery_is_flushing_but_not_emerging_as_durable_pending_stock",
        }
    if durable_persisted_delta > 0.0 and pending_delta <= 0.0 and excluded_from_pending_delta > 0.0:
        return {
            "verdict": "persistence_limited",
            "reason": "durable_file_persistence_completed_but_rows_were_excluded_before_persisted_file_pending",
        }
    if rates.get("buffered_to_persisted_per_min", 0.0) <= 0.0 and scan_delta >= 0.0:
        return {
            "verdict": "admission_limited",
            "reason": admission_block or "buffered_discovery_is_not_converting_into_persisted_stock",
        }
    if rates.get("buffered_to_persisted_per_min", 0.0) > 0.0 and persisted_delta > 0.0 and graph_ready_delta <= 0.0:
        return {
            "verdict": "graph_production_limited",
            "reason": graph_block or "persisted_stock_is_accumulating_faster_than_graph_ready_progress",
        }
    if rates.get("persisted_to_graph_ready_per_min", 0.0) > 0.0 and vector_ready_delta <= 0.0:
        return {
            "verdict": "vector_downstream_limited",
            "reason": vector_block or "graph_ready_stock_is_advancing_but_vector_ready_is_not",
        }
    if (
        rates.get("buffered_to_persisted_per_min", 0.0) <= 0.0
        and rates.get("persisted_to_graph_ready_per_min", 0.0) <= 0.0
        and rates.get("graph_ready_to_vector_ready_per_min", 0.0) > 0.0
    ):
        return {
            "verdict": "vector_downstream_limited",
            "reason": "downstream_vector_stock_is_draining_without_new_upstream_conversion",
        }
    return {
        "verdict": "balanced_conversion",
        "reason": "canonical_boundaries_show_measurable_progress",
    }


def diagnose_resource_balance(summary: dict[str, Any]) -> dict[str, Any]:
    sample_count = int(summary.get("sample_count") or 0)
    pipeline_sample_count = int(summary.get("pipeline_buffer", {}).get("sample_count") or 0)
    cpu_avg = float(summary.get("cpu_usage_pct", {}).get("avg") or 0.0)
    ram_available_p50 = float(summary.get("ram_available_gb", {}).get("p50") or 0.0)
    gpu_avg = float(summary.get("gpu_util_pct", {}).get("avg") or 0.0)
    gpu_p95 = float(summary.get("gpu_util_pct", {}).get("p95") or 0.0)
    gpu_mem_used_p95 = float(summary.get("gpu_mem_used_mb", {}).get("p95") or 0.0)
    gpu_mem_total = float(summary.get("gpu_mem_used_mb", {}).get("max") or 0.0) + float(
        summary.get("gpu_mem_free_mb", {}).get("min") or 0.0
    )
    pipeline = summary.get("pipeline_buffer", {})
    ready_avg = float(
        pipeline.get("ready_queue_chunks_current", {}).get("avg")
        or pipeline.get("ready_queue_depth_current", {}).get("avg")
        or 0.0
    )
    prepare_avg = float(
        pipeline.get("prepare_inflight_chunks_current", {}).get("avg")
        or pipeline.get("prepare_inflight_current", {}).get("avg")
        or 0.0
    )
    ready_gap_avg = float(
        pipeline.get("ready_replenishment_deficit_current", {}).get("avg") or 0.0
    )
    target_chunks_p50 = float(pipeline.get("target_embed_batch_chunks", {}).get("p50") or 0.0)
    actual_chunks_avg = float(pipeline.get("avg_chunks_per_embed_call", {}).get("avg") or 0.0)
    sawtooth = summary.get("gpu_util_sawtooth", {})
    sign_changes = int(sawtooth.get("sign_changes") or 0)
    max_step = float(sawtooth.get("max_step_abs_pct") or 0.0)

    verdict = "balanced"
    reason = "resource_balance_looks_reasonable"
    signals: list[str] = []

    if sample_count < 3 or pipeline_sample_count < 2:
        verdict = "insufficient_observation_window"
        reason = "resource_sampling_window_is_too_short_to_classify_runtime_balance"
        signals.extend(["sample_count_too_low", "extend_runtime_observation"])
    elif gpu_mem_total > 0 and gpu_mem_used_p95 / gpu_mem_total >= 0.90:
        verdict = "vram_limited"
        reason = "gpu_memory_runs_close_to_capacity"
        signals.extend(["high_vram_p95", "prefer_single_gpu_worker_or_smaller_batches"])
    elif ready_gap_avg > 0:
        verdict = "vector_underfed"
        reason = "front_chunk_gap_persists_during_observation_window"
        signals.extend(["ready_gap_persisted", "increase_front_chunk_supply"])
    elif (
        gpu_avg < 35.0
        and cpu_avg < 50.0
        and ram_available_p50 > 8.0
        and ready_avg < 12.0
        and prepare_avg < 3.0
        and target_chunks_p50 > 0.0
        and actual_chunks_avg < target_chunks_p50 * 0.55
    ):
        verdict = "likely_underfed_by_cpu_prepare"
        reason = "gpu_oscillates_while_cpu_and_ram_have_headroom_and_pre_gpu_stock_stays_thin"
        signals.extend(
            [
                "gpu_avg_low",
                "cpu_headroom_available",
                "ram_headroom_available",
                "ready_buffer_thin",
                "prepare_pipeline_shallow",
                "actual_batch_density_below_target",
            ]
        )
        if sign_changes >= 40 or max_step >= 50.0:
            signals.append("gpu_util_sawtooth_high")
    elif gpu_p95 >= 80.0 and ready_avg >= 8.0:
        verdict = "gpu_compute_engaged"
        reason = "gpu_receives_enough_work_and_spends_time_at_high_utilization"
        signals.extend(["gpu_p95_high", "ready_buffer_present"])

    return {
        "verdict": verdict,
        "reason": reason,
        "signals": signals,
    }


def summarize_resource_samples(samples: list[dict[str, Any]], interval_ms: int) -> dict[str, Any]:
    gpu_samples = [sample for sample in samples if sample.get("gpu_available") is True]
    pipeline_samples = [sample for sample in samples if sample.get("pipeline_available") is True]
    summary = {
        "interval_ms": interval_ms,
        "sample_count": len(samples),
        "duration_ms": len(samples) * interval_ms,
        "cpu_usage_pct": summarize_numeric_series(samples, "cpu_usage_pct"),
        "ram_used_gb": summarize_numeric_series(samples, "ram_used_gb"),
        "ram_available_gb": summarize_numeric_series(samples, "ram_available_gb"),
        "gpu_util_pct": summarize_numeric_series(gpu_samples, "gpu_util_pct"),
        "gpu_mem_used_mb": summarize_numeric_series(gpu_samples, "gpu_mem_used_mb"),
        "gpu_mem_free_mb": summarize_numeric_series(gpu_samples, "gpu_mem_free_mb"),
        "gpu_temp_c": summarize_numeric_series(gpu_samples, "gpu_temp_c"),
        "gpu_power_w": summarize_numeric_series(gpu_samples, "gpu_power_w"),
        "gpu_util_sawtooth": gpu_sawtooth_summary(gpu_samples),
        "pipeline_buffer": {
            "sample_count": len(pipeline_samples),
            "watcher_buffered_current": summarize_progression_series(
                pipeline_samples, "watcher_buffered_current"
            ),
            "scan_buffered_current": summarize_progression_series(
                pipeline_samples, "scan_buffered_current"
            ),
            "persisted_file_current": summarize_progression_series(
                pipeline_samples, "persisted_file_current"
            ),
            "persisted_file_pending_current": summarize_progression_series(
                pipeline_samples, "persisted_file_pending_current"
            ),
            "graph_wip_current": summarize_progression_series(
                pipeline_samples, "graph_wip_current"
            ),
            "ingress_flush_count": summarize_progression_series(
                pipeline_samples, "ingress_flush_count"
            ),
            "ingress_last_flush_duration_ms": summarize_numeric_series(
                pipeline_samples, "ingress_last_flush_duration_ms"
            ),
            "ingress_last_promoted_count": summarize_progression_series(
                pipeline_samples, "ingress_last_promoted_count"
            ),
            "ingress_promoted_total": summarize_progression_series(
                pipeline_samples, "ingress_promoted_total"
            ),
            "ingress_last_durably_persisted_count": summarize_progression_series(
                pipeline_samples, "ingress_last_durably_persisted_count"
            ),
            "ingress_durably_persisted_total": summarize_progression_series(
                pipeline_samples, "ingress_durably_persisted_total"
            ),
            "ingress_last_excluded_from_pending_count": summarize_progression_series(
                pipeline_samples, "ingress_last_excluded_from_pending_count"
            ),
            "ingress_excluded_from_pending_total": summarize_progression_series(
                pipeline_samples, "ingress_excluded_from_pending_total"
            ),
            "structural_graph_backlog_current": summarize_progression_series(
                pipeline_samples, "structural_graph_backlog_current"
            ),
            "structural_graph_backlog_queued_current": summarize_progression_series(
                pipeline_samples, "structural_graph_backlog_queued_current"
            ),
            "structural_graph_backlog_inflight_current": summarize_progression_series(
                pipeline_samples, "structural_graph_backlog_inflight_current"
            ),
            "graph_projection_queue_current": summarize_progression_series(
                pipeline_samples, "graph_projection_queue_current"
            ),
            "graph_projection_queue_queued_current": summarize_progression_series(
                pipeline_samples, "graph_projection_queue_queued_current"
            ),
            "graph_projection_queue_inflight_current": summarize_progression_series(
                pipeline_samples, "graph_projection_queue_inflight_current"
            ),
            "graph_ready_current": summarize_progression_series(
                pipeline_samples, "graph_ready_current"
            ),
            "vector_queue_current": summarize_progression_series(
                pipeline_samples, "vector_queue_current"
            ),
            "vector_ready_current": summarize_progression_series(
                pipeline_samples, "vector_ready_current"
            ),
            "ready_queue_depth_current": summarize_numeric_series(
                pipeline_samples, "ready_queue_depth_current"
            ),
            "ready_queue_chunks_current": summarize_numeric_series(
                pipeline_samples, "ready_queue_chunks_current"
            ),
            "prepare_inflight_current": summarize_numeric_series(
                pipeline_samples, "prepare_inflight_current"
            ),
            "prepare_inflight_chunks_current": summarize_numeric_series(
                pipeline_samples, "prepare_inflight_chunks_current"
            ),
            "ready_replenishment_deficit_current": summarize_numeric_series(
                pipeline_samples, "ready_replenishment_deficit_current"
            ),
            "prepare_claimed_current": summarize_numeric_series(
                pipeline_samples, "prepare_claimed_current"
            ),
            "active_claimed_current": summarize_numeric_series(
                pipeline_samples, "active_claimed_current"
            ),
            "oldest_ready_batch_age_ms_current": summarize_numeric_series(
                pipeline_samples, "oldest_ready_batch_age_ms_current"
            ),
            "target_embed_batch_chunks": summarize_numeric_series(
                pipeline_samples, "target_embed_batch_chunks"
            ),
            "target_files_per_cycle": summarize_numeric_series(
                pipeline_samples, "target_files_per_cycle"
            ),
            "avg_chunks_per_embed_call": summarize_numeric_series(
                pipeline_samples, "avg_chunks_per_embed_call"
            ),
            "avg_files_per_embed_call": summarize_numeric_series(
                pipeline_samples, "avg_files_per_embed_call"
            ),
            "controller_state": summarize_controller_modes(pipeline_samples),
            "admission_blocking_authority": summarize_categorical_series(
                pipeline_samples, "admission_blocking_authority"
            ),
            "admission_wip_current": summarize_progression_series(
                pipeline_samples, "admission_wip_current"
            ),
            "graph_blocking_authority": summarize_categorical_series(
                pipeline_samples, "graph_blocking_authority"
            ),
            "vector_blocking_authority": summarize_categorical_series(
                pipeline_samples, "vector_blocking_authority"
            ),
        },
    }
    summary["diagnosis"] = diagnose_resource_balance(summary)
    summary["conversion_rates"] = summarize_conversion_rates(summary)
    summary["conversion_diagnosis"] = diagnose_conversion_pipeline(summary)
    return summary


class ResourceSampler:
    def __init__(self, run_dir: Path, interval_ms: int, mcp_url: str, mcp_timeout: int) -> None:
        self.run_dir = run_dir
        self.interval_ms = max(100, interval_ms)
        self.mcp_url = mcp_url
        self.mcp_timeout = mcp_timeout
        self.samples_path = run_dir / "runtime-resource-samples.jsonl"
        self.summary_path = run_dir / "runtime-resource-summary.json"
        self._stop = threading.Event()
        self._thread: threading.Thread | None = None
        self._samples: list[dict[str, Any]] = []
        self._prev_cpu: tuple[int, int] | None = None
        self._last_pipeline_sample_at = 0.0

    def _capture_pipeline_sample(self) -> dict[str, Any]:
        try:
            status_data = fetch_status_snapshot(self.mcp_url, timeout=self.mcp_timeout)
        except Exception:
            return {"pipeline_available": False}
        runtime_authority = status_data.get("runtime_authority", {})
        lane_parameters = (
            runtime_authority.get("lane_parameters", {})
            if isinstance(runtime_authority, dict)
            else {}
        )
        quiescent_state = (
            runtime_authority.get("quiescent_state", {})
            if isinstance(runtime_authority, dict)
            else {}
        )
        stage_model = (
            runtime_authority.get("canonical_ingestion_stage_model", {})
            if isinstance(runtime_authority, dict)
            else {}
        )
        canonical_edges = (
            runtime_authority.get("canonical_edges", {})
            if isinstance(runtime_authority, dict)
            else {}
        )
        observed_residual_work = (
            quiescent_state.get("observed_residual_work", {})
            if isinstance(quiescent_state, dict)
            else {}
        )
        backlog_drain = (
            quiescent_state.get("backlog_drain", {})
            if isinstance(quiescent_state, dict)
            else {}
        )
        if not isinstance(lane_parameters, dict):
            lane_parameters = {}
        if not isinstance(observed_residual_work, dict):
            observed_residual_work = {}
        if not isinstance(backlog_drain, dict):
            backlog_drain = {}
        if not isinstance(stage_model, dict):
            stage_model = {}
        if not isinstance(canonical_edges, dict):
            canonical_edges = {}

        vector_runtime = observed_residual_work
        vector_controller = {}
        semantic_cadence = lane_parameters.get("semantic_cadence", {})
        if isinstance(semantic_cadence, dict):
            vector_controller = {
                "state": semantic_cadence.get("controller_state"),
                "reason": semantic_cadence.get("controller_reason"),
            }

        # Compatibility fallback for older runtimes that still expose the richer
        # controller snapshot only under debug_snapshot.embedding_contract.
        if not vector_runtime or not vector_controller.get("state"):
            debug_snapshot = status_data.get("debug_snapshot", {})
            embedding_contract = (
                debug_snapshot.get("embedding_contract", {})
                if isinstance(debug_snapshot, dict)
                else {}
            )
            legacy_runtime = (
                embedding_contract.get("vector_runtime", {})
                if isinstance(embedding_contract, dict)
                else {}
            )
            legacy_controller = (
                embedding_contract.get("vector_batch_controller", {})
                if isinstance(embedding_contract, dict)
                else {}
            )
            if isinstance(legacy_runtime, dict) and legacy_runtime:
                vector_runtime = legacy_runtime
            if isinstance(legacy_controller, dict) and legacy_controller:
                vector_controller = legacy_controller
        else:
            debug_snapshot = status_data.get("debug_snapshot", {})
            embedding_contract = (
                debug_snapshot.get("embedding_contract", {})
                if isinstance(debug_snapshot, dict)
                else {}
            )
            legacy_runtime = (
                embedding_contract.get("vector_runtime", {})
                if isinstance(embedding_contract, dict)
                else {}
            )
            legacy_controller = (
                embedding_contract.get("vector_batch_controller", {})
                if isinstance(embedding_contract, dict)
                else {}
            )
            if isinstance(legacy_runtime, dict):
                for key in (
                    "prepare_inflight_current",
                    "prepare_claimed_current",
                    "active_claimed_current",
                    "oldest_ready_batch_age_ms_current",
                    "ready_queue_depth_current",
                ):
                    if vector_runtime.get(key) is None and legacy_runtime.get(key) is not None:
                        vector_runtime[key] = legacy_runtime.get(key)
            if isinstance(legacy_controller, dict):
                for key in (
                    "target_embed_batch_chunks",
                    "target_files_per_cycle",
                    "avg_chunks_per_embed_call",
                    "avg_files_per_embed_call",
                    "reason",
                    "state",
                ):
                    if vector_controller.get(key) is None and legacy_controller.get(key) is not None:
                        vector_controller[key] = legacy_controller.get(key)

        ready_lane = lane_parameters.get("vector_ready_queue_depth", {})
        if not isinstance(ready_lane, dict):
            ready_lane = {}

        def stage_count(name: str) -> Any:
            value = stage_model.get(name, {})
            if isinstance(value, dict):
                return value.get("current_count")
            return None

        structural_graph_breakdown = stage_model.get("structural_graph_backlog", {})
        if not isinstance(structural_graph_breakdown, dict):
            structural_graph_breakdown = {}
        structural_graph_counts = structural_graph_breakdown.get("queue_breakdown", {})
        if not isinstance(structural_graph_counts, dict):
            structural_graph_counts = {}
        graph_projection_breakdown = stage_model.get("graph_projection_queue_owned", {})
        if not isinstance(graph_projection_breakdown, dict):
            graph_projection_breakdown = {}
        graph_projection_counts = graph_projection_breakdown.get("queue_breakdown", {})
        if not isinstance(graph_projection_counts, dict):
            graph_projection_counts = {}
        ingress_promotion = stage_model.get("ingress_promotion", {})
        if not isinstance(ingress_promotion, dict):
            ingress_promotion = {}
        admission_edge = canonical_edges.get("admission_edge", {})
        if not isinstance(admission_edge, dict):
            admission_edge = {}
        admission_controller = runtime_authority.get("admission_controller", {})
        if not isinstance(admission_controller, dict):
            admission_controller = {}
        graph_edge = canonical_edges.get("graph_production_edge", {})
        if not isinstance(graph_edge, dict):
            graph_edge = {}
        vector_edge = canonical_edges.get("vector_downstream_edge", {})
        if not isinstance(vector_edge, dict):
            vector_edge = {}
        return {
            "pipeline_available": bool(vector_runtime) or bool(vector_controller) or bool(stage_model),
            "watcher_buffered_current": stage_count("watcher_buffered"),
            "scan_buffered_current": stage_count("scan_buffered"),
            "persisted_file_current": stage_count("persisted_file"),
            "persisted_file_pending_current": stage_count("persisted_file_pending"),
            "graph_wip_current": stage_count("graph_wip"),
            "ingress_flush_count": admission_controller.get("admission_flush_count", ingress_promotion.get("flush_count")),
            "ingress_last_flush_duration_ms": admission_controller.get("admission_last_flush_duration_ms", ingress_promotion.get("last_flush_duration_ms")),
            "ingress_last_promoted_count": admission_controller.get("admission_last_promoted_count", ingress_promotion.get("last_promoted_count")),
            "ingress_promoted_total": admission_controller.get("admission_promoted_total", ingress_promotion.get("promoted_total")),
            "ingress_last_durably_persisted_count": admission_controller.get("admission_last_durably_persisted_count"),
            "ingress_durably_persisted_total": admission_controller.get("admission_durably_persisted_total"),
            "ingress_last_excluded_from_pending_count": admission_controller.get("admission_last_excluded_from_pending_count"),
            "ingress_excluded_from_pending_total": admission_controller.get("admission_excluded_from_pending_total"),
            "structural_graph_backlog_current": stage_count("structural_graph_backlog"),
            "structural_graph_backlog_queued_current": structural_graph_counts.get("queued"),
            "structural_graph_backlog_inflight_current": structural_graph_counts.get("inflight"),
            "graph_projection_queue_current": stage_count("graph_projection_queue_owned"),
            "graph_projection_queue_queued_current": graph_projection_counts.get("queued"),
            "graph_projection_queue_inflight_current": graph_projection_counts.get("inflight"),
            "graph_ready_current": stage_count("graph_ready"),
            "vector_queue_current": stage_count("file_vectorization_queue_owned"),
            "vector_ready_current": stage_count("vector_ready"),
            "ready_queue_depth_current": vector_runtime.get("ready_queue_depth_current"),
            "ready_queue_chunks_current": vector_runtime.get("ready_queue_chunks_current"),
            "prepare_inflight_current": vector_runtime.get("prepare_inflight_current"),
            "prepare_inflight_chunks_current": vector_runtime.get(
                "prepare_inflight_chunks_current"
            ),
            "ready_replenishment_deficit_current": vector_runtime.get(
                "ready_replenishment_deficit_current"
            ),
            "prepare_claimed_current": vector_runtime.get("prepare_claimed_current"),
            "active_claimed_current": vector_runtime.get("active_claimed_current"),
            "oldest_ready_batch_age_ms_current": vector_runtime.get("oldest_ready_batch_age_ms_current"),
            "target_embed_batch_chunks": vector_controller.get("target_embed_batch_chunks"),
            "target_files_per_cycle": vector_controller.get("target_files_per_cycle"),
            "avg_chunks_per_embed_call": vector_controller.get("avg_chunks_per_embed_call"),
            "avg_files_per_embed_call": vector_controller.get("avg_files_per_embed_call"),
            "controller_reason": vector_controller.get("reason"),
            "controller_state": vector_controller.get("state"),
            "ready_queue_target": ready_lane.get("target"),
            "ready_queue_effective": ready_lane.get("effective"),
            "semantic_health": backlog_drain.get("semantic_health"),
            "provider_effective": backlog_drain.get("provider_effective"),
            "admission_blocking_authority": admission_edge.get("blocking_authority"),
            "admission_wip_current": admission_controller.get("admission_wip_current"),
            "graph_blocking_authority": graph_edge.get("blocking_authority"),
            "vector_blocking_authority": vector_edge.get("blocking_authority"),
        }

    def _capture_sample(self) -> dict[str, Any]:
        sample: dict[str, Any] = {
            "ts": time.time(),
            "ts_iso": utc_now_iso(),
        }
        current_cpu = read_proc_stat()
        if current_cpu is not None and self._prev_cpu is not None:
            total_delta = current_cpu[0] - self._prev_cpu[0]
            idle_delta = current_cpu[1] - self._prev_cpu[1]
            if total_delta > 0:
                sample["cpu_usage_pct"] = max(0.0, min(100.0, 100.0 * (1.0 - idle_delta / total_delta)))
        self._prev_cpu = current_cpu

        meminfo = read_meminfo()
        if meminfo:
            total = float(meminfo.get("MemTotal", 0))
            available = float(meminfo.get("MemAvailable", 0))
            used = max(0.0, total - available)
            sample["ram_total_gb"] = total / (1024**3)
            sample["ram_available_gb"] = available / (1024**3)
            sample["ram_used_gb"] = used / (1024**3)

        gpu = read_gpu_sample()
        sample["gpu_available"] = gpu.get("available", False)
        if sample["gpu_available"]:
            sample.update(
                {
                    "gpu_util_pct": gpu["gpu_util_pct"],
                    "gpu_mem_used_mb": gpu["gpu_mem_used_mb"],
                    "gpu_mem_free_mb": gpu["gpu_mem_free_mb"],
                    "gpu_mem_total_mb": gpu["gpu_mem_total_mb"],
                    "gpu_temp_c": gpu["gpu_temp_c"],
                    "gpu_power_w": gpu["gpu_power_w"],
                    "gpu_power_limit_w": gpu["gpu_power_limit_w"],
                }
            )
        else:
            sample["gpu_reason"] = gpu.get("reason")

        now = sample["ts"]
        if now - self._last_pipeline_sample_at >= 1.0:
            sample.update(self._capture_pipeline_sample())
            self._last_pipeline_sample_at = now
        return sample

    def _run(self) -> None:
        with self.samples_path.open("w", encoding="utf-8") as handle:
            while not self._stop.is_set():
                sample = self._capture_sample()
                self._samples.append(sample)
                handle.write(json.dumps(sample, ensure_ascii=False) + "\n")
                handle.flush()
                self._stop.wait(self.interval_ms / 1000.0)

    def start(self) -> None:
        self._thread = threading.Thread(target=self._run, name="qualify-resource-sampler", daemon=True)
        self._thread.start()

    def stop(self) -> dict[str, Any]:
        self._stop.set()
        if self._thread is not None:
            self._thread.join(timeout=5)
        summary = summarize_resource_samples(self._samples, self.interval_ms)
        write_json(self.summary_path, summary)
        return summary


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


def run_runtime_smoke(
    mode: str,
    run_dir: Path,
    url: str,
    instance: str,
    resource_sample_interval_ms: int,
    gpu_qualified_runtime: bool = False,
    reuse_runtime: bool = False,
) -> dict[str, Any]:
    t0 = time.time()
    resource_sampler: ResourceSampler | None = None
    env = command_env(mode, instance, url, gpu_qualified_runtime=gpu_qualified_runtime)
    smoke_budget_s = 180

    try:
        start_timed_out = False
        if reuse_runtime:
            # REQ-AXO-901838 — --reuse-runtime must NEVER stop/restart a
            # third-party runtime (dev started out-of-band). Observe the
            # existing runtime only; never own its lifecycle. Mirrors the
            # contract in qualify_ingestion_run.py (`if not args.reuse_runtime`).
            stop_log = "[qualify] stop skipped because --reuse-runtime was requested\n"
            start_log = "[qualify] start skipped because --reuse-runtime was requested\n"
            (run_dir / "runtime-stop.log").write_text(stop_log, encoding="utf-8")
            (run_dir / "runtime-start.log").write_text(start_log, encoding="utf-8")
        else:
            try:
                stop_proc = shell(["bash", "scripts/stop.sh"], timeout=30)
                stop_log = completed_output(stop_proc.stdout) + completed_output(stop_proc.stderr)
            except subprocess.TimeoutExpired as exc:
                stop_log = completed_output(exc.stdout) + completed_output(exc.stderr)
                stop_log += f"\n[qualify] stop.sh timeout after {exc.timeout}s\n"
            (run_dir / "runtime-stop.log").write_text(stop_log, encoding="utf-8")

            try:
                start_proc = shell(start_command_for_mode(mode), env=env, timeout=smoke_budget_s)
                start_log = completed_output(start_proc.stdout) + completed_output(start_proc.stderr)
            except subprocess.TimeoutExpired as exc:
                start_timed_out = True
                start_log = completed_output(exc.stdout) + completed_output(exc.stderr)
                start_log += (
                    f"\n[qualify] start timeout after {exc.timeout}s; checking runtime readiness anyway\n"
                )
            (run_dir / "runtime-start.log").write_text(start_log, encoding="utf-8")

        effective_url = resolve_effective_mcp_url(start_log, url)
        resource_sampler = ResourceSampler(
            run_dir,
            interval_ms=resource_sample_interval_ms,
            mcp_url=effective_url,
            mcp_timeout=10,
        )
        resource_sampler.start()

        wait_for_mcp_ready(effective_url, 120)
        note = "runtime ready"
        if start_timed_out:
            note = f"{note}; start wrapper exceeded {smoke_budget_s}s budget"
        status_data = fetch_status_snapshot(effective_url)
        runtime_truth_summary = summarize_runtime_truth(status_data)
        quiescent_summary = summarize_quiescent_status(status_data)
        burn_rate_probe = None
        if should_probe_semantic_burn_rate(status_data, quiescent_summary):
            burn_rate_probe = measure_semantic_backlog_burn_rate(effective_url, status_data)
            if (
                burn_rate_probe.get("state") == "still_warming_or_long_batch"
                and isinstance(burn_rate_probe.get("after"), dict)
                and burn_rate_probe["after"].get("semantic_health") == "healthy_draining"
                and burn_rate_probe["after"].get("vector_lane_state") == "healthy"
            ):
                extended_probe = measure_semantic_backlog_burn_rate(
                    effective_url,
                    burn_rate_probe.get("status_after_probe", status_data),
                    probe_window_sec=45,
                )
                burn_rate_probe["extended_probe"] = {
                    key: value
                    for key, value in extended_probe.items()
                    if key != "status_after_probe"
                }
                burn_rate_probe["status_after_probe"] = extended_probe.get(
                    "status_after_probe", burn_rate_probe.get("status_after_probe", status_data)
                )
                if extended_probe.get("state") == "measurable_progress":
                    burn_rate_probe["state"] = "measurable_progress_after_extended_probe"
                    burn_rate_probe["recommendation"] = extended_probe.get(
                        "recommendation", "track_burn_rate_until_backlog_turns_down"
                    )
            elif (
                burn_rate_probe.get("state") == "progress_uncertain"
                and isinstance(burn_rate_probe.get("after"), dict)
                and burn_rate_probe["after"].get("vector_lane_state") == "healthy"
                and burn_rate_probe["after"].get("effective_semantic_backlog_depth", 0) > 0
                and gpu_vector_lease_diagnostics(
                    burn_rate_probe.get("status_after_probe", status_data)
                ).get("owned_by_current_instance")
                is True
            ):
                extended_probe = measure_semantic_backlog_burn_rate(
                    effective_url,
                    burn_rate_probe.get("status_after_probe", status_data),
                    probe_window_sec=45,
                )
                burn_rate_probe["extended_probe"] = {
                    key: value
                    for key, value in extended_probe.items()
                    if key != "status_after_probe"
                }
                burn_rate_probe["status_after_probe"] = extended_probe.get(
                    "status_after_probe", burn_rate_probe.get("status_after_probe", status_data)
                )
                if extended_probe.get("state") == "measurable_progress":
                    burn_rate_probe["state"] = "measurable_progress_after_extended_probe"
                    burn_rate_probe["recommendation"] = extended_probe.get(
                        "recommendation", "track_burn_rate_until_backlog_turns_down"
                    )
            if burn_rate_probe.get("state") in {"measurable_progress", "measurable_progress_after_extended_probe"}:
                after_status = burn_rate_probe.get("status_after_probe", status_data)
                quiescent_summary = summarize_quiescent_status(after_status)
                quiescent_summary["throughput_observed"] = True
                quiescent_summary["throughput_recommendation"] = burn_rate_probe.get(
                    "recommendation", "track_burn_rate_until_backlog_turns_down"
                )
                quiescent_summary["throughput_probe"] = {
                    "state": burn_rate_probe.get("state"),
                    "measured_chunks_per_minute": burn_rate_probe.get("delta", {}).get(
                        "measured_chunks_per_minute"
                    ),
                    "measured_files_per_minute": burn_rate_probe.get("delta", {}).get(
                        "measured_files_per_minute"
                    ),
                    "effective_semantic_backlog_delta": burn_rate_probe.get("delta", {}).get(
                        "effective_semantic_backlog_depth"
                    ),
                }
            quiescent_summary["burn_rate_probe"] = {
                key: value
                for key, value in burn_rate_probe.items()
                if key != "status_after_probe"
            }
            status_data = burn_rate_probe.get("status_after_probe", status_data)
        else:
            time.sleep(MIN_RUNTIME_OBSERVATION_SEC)
            status_data = fetch_status_snapshot(effective_url)
            quiescent_summary = summarize_quiescent_status(status_data)
        runtime_truth_summary = summarize_runtime_truth(status_data)
        guardrails_summary = summarize_runtime_guardrails(mode, instance, runtime_truth_summary)
        (run_dir / "runtime-status.json").write_text(
            json.dumps(status_data, indent=2, ensure_ascii=False) + "\n",
            encoding="utf-8",
        )
        (run_dir / "runtime-quiescent-summary.json").write_text(
            json.dumps(quiescent_summary, indent=2, ensure_ascii=False) + "\n",
            encoding="utf-8",
        )
        if burn_rate_probe is not None:
            (run_dir / "runtime-burn-rate-probe.json").write_text(
                json.dumps(burn_rate_probe, indent=2, ensure_ascii=False) + "\n",
                encoding="utf-8",
            )
        quiescent_verdict = quiescent_summary.get("qualification_verdict")
        step_status = quiescent_step_status(quiescent_summary)
        if quiescent_verdict:
            note = f"{note}; quiescent={quiescent_verdict}"
        elif quiescent_summary.get("available") is False:
            note = f"{note}; quiescent_unavailable={quiescent_summary.get('reason', 'unknown')}"
        if runtime_truth_summary.get("available") is False:
            note = f"{note}; runtime_truth_unavailable={runtime_truth_summary.get('reason', 'unknown')}"
        else:
            note = (
                f"{note}; runtime_truth={runtime_truth_summary.get('truth_status', 'unknown')}"
                f"; process_role={runtime_truth_summary.get('process_role', 'unknown')}"
                f"; public_mcp_authority={runtime_truth_summary.get('public_mcp_authority', 'unknown')}"
                f"; soll_writer_authority={runtime_truth_summary.get('soll_writer_authority', 'unknown')}"
                f"; ist_writer_authority={runtime_truth_summary.get('ist_writer_authority', 'unknown')}"
                f"; brain_ready={runtime_truth_summary.get('brain_ready', 'unknown')}"
                f"; indexer_ready={runtime_truth_summary.get('indexer_ready', 'unknown')}"
                f"; system_converged={runtime_truth_summary.get('system_converged', 'unknown')}"
                f"; runtime_feed_state={runtime_truth_summary.get('indexer_feed_state', 'unknown')}"
                f"; stale_runtime_feed={runtime_truth_summary.get('indexer_feed_stale', 'unknown')}"
                f"; ist_snapshot={runtime_truth_summary.get('ist_snapshot_state', 'unknown')}"
                f"; stale_ist_snapshot={runtime_truth_summary.get('ist_snapshot_stale', 'unknown')}"
            )
        note = (
            f"{note}; canonical_truth_restored={guardrails_summary.get('canonical_truth_restored', 'unknown')}"
            f"; canonical_authority_restored={guardrails_summary.get('canonical_authority_restored', 'unknown')}"
            f"; rollback_path={guardrails_summary.get('rollback_path_state', 'unknown')}"
            f"; promotion_allowed={guardrails_summary.get('promotion_allowed', 'unknown')}"
            f"; cutover_blocked={guardrails_summary.get('cutover_blocked', 'unknown')}"
        )
        if isinstance(quiescent_summary.get("burn_rate_probe"), dict):
            note = (
                f"{note}; burn_rate_probe="
                f"{quiescent_summary['burn_rate_probe'].get('state', 'unknown')}"
            )
        resource_summary = resource_sampler.stop() if resource_sampler is not None else {}
        if runtime_truth_requires_warn(runtime_truth_summary, guardrails_summary):
            if step_status == "pass":
                step_status = "warn"
        return step_result(
            "runtime_smoke",
            step_status,
            int((time.time() - t0) * 1000),
            note,
            {
                "status": status_data,
                "quiescent": quiescent_summary,
                "runtime_truth": runtime_truth_summary,
                "guardrails": guardrails_summary,
                "resources": resource_summary,
            },
        )
    except Exception as exc:
        resource_summary = resource_sampler.stop() if resource_sampler is not None else {}
        return step_result(
            "runtime_smoke",
            "fail",
            int((time.time() - t0) * 1000),
            f"{type(exc).__name__}: {exc}",
            {"resources": resource_summary},
        )


def run_mcp_validate(
    args: argparse.Namespace, mode: str, run_dir: Path, instance: str, url: str
) -> dict[str, Any]:
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
    if args.allow_mutations:
        cmd.append("--allow-mutations")
    if args.symbol:
        cmd.extend(["--symbol", args.symbol])
    proc = shell(
        cmd,
        env=command_env(
            mode,
            instance,
            url,
            gpu_qualified_runtime=args.gpu_qualified_runtime,
        ),
    )
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


def run_mcp_robustness(args: argparse.Namespace, mode: str, run_dir: Path, instance: str, url: str) -> dict[str, Any]:
    t0 = time.time()
    output_root = run_dir / "robustness"
    output_root.mkdir(parents=True, exist_ok=True)
    supported_mode = mcp_robustness_supported_mode(mode)
    if supported_mode is None:
        note = (
            "skipped: split shadow mode is not yet modeled by qualify_mcp_robustness.py; "
            "shadow launch and runtime smoke remain covered"
        )
        return step_result("mcp_robustness", "warn", int((time.time() - t0) * 1000), note, {})
    cmd = [
        sys.executable,
        "scripts/qualify_mcp_robustness.py",
        "--modes",
        supported_mode,
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
    proc = shell(
        cmd,
        env=command_env(
            mode,
            instance,
            url,
            gpu_qualified_runtime=args.gpu_qualified_runtime,
        ),
    )
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


def run_retrieval_qualify(args: argparse.Namespace, mode: str, run_dir: Path, instance: str, url: str) -> dict[str, Any]:
    if mode != "indexer_full":
        return step_result(
            "retrieval_qualify",
            "warn",
            0,
            f"skipped because retrieve_context is only available in indexer_full autonomous mode (mode={mode})",
        )
    t0 = time.time()
    json_out = run_dir / "retrieval_qualify.json"
    cmd = [
        sys.executable,
        "scripts/qualify_retrieval_context.py",
        "--project",
        args.project,
        "--corpus",
        args.retrieval_corpus,
        "--timeout",
        str(args.timeout),
        "--json-out",
        str(json_out),
    ]
    proc = shell(
        cmd,
        env=command_env(
            mode,
            instance,
            url,
            gpu_qualified_runtime=args.gpu_qualified_runtime,
        ),
    )
    (run_dir / "retrieval_qualify.stdout.log").write_text((proc.stdout or "") + (proc.stderr or ""), encoding="utf-8")
    summary = {}
    if json_out.exists():
        summary = json.loads(json_out.read_text(encoding="utf-8"))
    summary_block = summary.get("summary", {}) if isinstance(summary, dict) else {}
    verdict = summary_block.get("verdict")
    step_status = "pass"
    if verdict == "fail":
        step_status = "fail"
    elif verdict == "warn":
        step_status = "warn"
    note = f"verdict={verdict or 'unknown'} exit={proc.returncode}"
    return step_result("retrieval_qualify", step_status, int((time.time() - t0) * 1000), note, summary)


def run_ingestion_qualify(args: argparse.Namespace, mode: str, run_dir: Path, instance: str, url: str) -> dict[str, Any]:
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
    if args.reuse_runtime:
        cmd.append("--reuse-runtime")
    proc = shell(
        cmd,
        env=command_env(
            mode,
            instance,
            url,
            gpu_qualified_runtime=args.gpu_qualified_runtime,
        ),
    )
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
    instance = normalize_instance(getattr(args, "instance", ""))
    mcp_url = os.environ.get("AXON_MCP_URL", "").strip() or default_mcp_url_for_instance(instance)

    for step_name in profile_steps(args.profile):
        if step_name == "runtime_smoke":
            result = run_runtime_smoke(
                mode,
                mode_run_dir,
                mcp_url,
                instance,
                args.resource_sample_interval_ms,
                gpu_qualified_runtime=args.gpu_qualified_runtime,
                reuse_runtime=args.reuse_runtime,
            )
        elif step_name == "mcp_validate":
            if steps.get("runtime_smoke", {}).get("status") == "fail":
                result = step_result("mcp_validate", "fail", 0, "skipped because runtime_smoke failed")
            else:
                result = run_mcp_validate(args, mode, mode_run_dir, instance, mcp_url)
        elif step_name == "mcp_robustness":
            if steps.get("runtime_smoke", {}).get("status") == "fail":
                result = step_result("mcp_robustness", "fail", 0, "skipped because runtime_smoke failed")
            else:
                result = run_mcp_robustness(args, mode, mode_run_dir, instance, mcp_url)
        elif step_name == "retrieval_qualify":
            if steps.get("runtime_smoke", {}).get("status") == "fail":
                result = step_result("retrieval_qualify", "fail", 0, "skipped because runtime_smoke failed")
            else:
                result = run_retrieval_qualify(args, mode, mode_run_dir, instance, mcp_url)
        elif step_name == "ingestion_qualify":
            result = run_ingestion_qualify(args, mode, mode_run_dir, instance, mcp_url)
        else:
            result = step_result(step_name, "fail", 0, "unsupported step")
        steps[step_name] = result
        ordered_steps.append(result)

    verdict = combine_step_statuses(ordered_steps)
    runtime_smoke_summary = steps.get("runtime_smoke", {}).get("summary", {})
    runtime_quiescent = {}
    if isinstance(runtime_smoke_summary, dict):
        runtime_quiescent = runtime_smoke_summary.get("quiescent", {})
    return {
        "mode": mode,
        "instance": instance,
        "mcp_url": mcp_url,
        "profile": args.profile,
        "verdict": verdict,
        "steps": steps,
        "step_order": [step["name"] for step in ordered_steps],
        "runtime_quiescent": runtime_quiescent,
    }


def main(argv: list[str]) -> int:
    args = parse_args(argv)
    args.instance = normalize_instance(args.instance)
    if args.cold:
        # REQ-AXO-113 — cold reset implies the runtime is freshly started by the
        # baseline orchestrator; downstream qualify steps must not stop/restart it.
        args.reuse_runtime = True
        perform_cold_reset(args)
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
        "instance": args.instance,
        "mcp_url": os.environ.get("AXON_MCP_URL", "").strip() or default_mcp_url_for_instance(args.instance),
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
        "gpu_qualified_runtime": args.gpu_qualified_runtime,
        "cold": args.cold,
        "tensorrt": args.tensorrt,
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
    runtime_quiescent_summary = summarize_runtime_quiescent(mode_reports)
    summary = {
        "created_at": utc_now_iso(),
        "run_dir": str(run_dir),
        "profile": args.profile,
        "modes": modes,
        "mode_reports": mode_reports,
        "comparison": comparison,
        "runtime_quiescent": runtime_quiescent_summary,
        "overall_verdict": overall_verdict,
    }
    write_json(run_dir / "summary.json", summary)

    print(f"[qualify] run_dir={run_dir}")
    print(f"[qualify] overall_verdict={overall_verdict}")
    for report in mode_reports:
        print(f"- mode={report['mode']} verdict={report['verdict']}")
        print(f"  - instance={report['instance']} mcp_url={report['mcp_url']}")
        mode_runtime_quiescent = report.get("runtime_quiescent", {})
        if isinstance(mode_runtime_quiescent, dict) and mode_runtime_quiescent:
            quiescent_note = mode_runtime_quiescent.get("qualification_verdict")
            if quiescent_note is None and mode_runtime_quiescent.get("available") is False:
                quiescent_note = f"unavailable:{mode_runtime_quiescent.get('reason', 'unknown')}"
            if quiescent_note is not None:
                print(f"  - runtime_quiescent: {quiescent_note}")
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
    if runtime_quiescent_summary:
        print(
            f"[qualify] runtime_quiescent_status={runtime_quiescent_summary.get('overall_status', 'unknown')}"
        )

    return exit_code_for_verdict(overall_verdict)


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
