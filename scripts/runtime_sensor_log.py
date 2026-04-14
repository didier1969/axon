#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Any


PROJECT_ROOT = Path("/home/dstadel/projects/axon")
DEFAULT_OUTPUT_ROOT = PROJECT_ROOT / ".axon" / "runtime-sensor-runs"
DEFAULT_MCP_URL = os.environ.get("AXON_MCP_URL", "http://127.0.0.1:44129/mcp")
DEFAULT_GPU_QUERY = (
    "name,driver_version,memory.total,memory.used,memory.free,utilization.gpu,utilization.memory"
)


def utc_now_iso() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="seconds")


def shell(args: list[str], *, check: bool = True) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        args,
        cwd=PROJECT_ROOT,
        text=True,
        capture_output=True,
        check=check,
    )


def detect_axon_pid() -> int | None:
    try:
        proc = shell(["pgrep", "-af", "axon-core"], check=True)
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


def parse_proc_status(pid: int) -> dict[str, int]:
    path = Path("/proc") / str(pid) / "status"
    result = {
        "rss_bytes": 0,
        "rss_anon_bytes": 0,
        "rss_file_bytes": 0,
        "rss_shmem_bytes": 0,
    }
    try:
        text = path.read_text()
    except OSError:
        return result

    key_map = {
        "VmRSS:": "rss_bytes",
        "RssAnon:": "rss_anon_bytes",
        "RssFile:": "rss_file_bytes",
        "RssShmem:": "rss_shmem_bytes",
    }
    for line in text.splitlines():
        parts = line.split()
        if len(parts) >= 2 and parts[0] in key_map:
            try:
                result[key_map[parts[0]]] = int(parts[1]) * 1024
            except ValueError:
                pass
    return result


def ps_value(pid: int, field: str) -> str:
    try:
        return shell(["ps", "-o", f"{field}=", "-p", str(pid)]).stdout.strip()
    except subprocess.CalledProcessError:
        return ""


def mcp_status(mcp_url: str) -> dict[str, Any]:
    payload = {
        "jsonrpc": "2.0",
        "id": "runtime-sensor-log",
        "method": "tools/call",
        "params": {"name": "status", "arguments": {}},
    }
    try:
        proc = shell(
            [
                "curl",
                "-sS",
                mcp_url,
                "-H",
                "Content-Type: application/json",
                "-d",
                json.dumps(payload),
            ]
        )
        parsed = json.loads(proc.stdout)
        if isinstance(parsed, dict):
            return parsed
    except Exception as exc:
        return {"error": type(exc).__name__}
    return {"error": "invalid_mcp_response"}


def gpu_status() -> dict[str, Any]:
    try:
        proc = shell(
            [
                "/usr/lib/wsl/lib/nvidia-smi",
                f"--query-gpu={DEFAULT_GPU_QUERY}",
                "--format=csv,noheader,nounits",
            ]
        )
        first = next((line.strip() for line in proc.stdout.splitlines() if line.strip()), "")
        if not first:
            return {"available": False}
        parts = [part.strip() for part in first.split(",")]
        if len(parts) < 7:
            return {"available": False, "raw": first}
        return {
            "available": True,
            "name": parts[0],
            "driver_version": parts[1],
            "memory_total_mb": int(parts[2]),
            "memory_used_mb": int(parts[3]),
            "memory_free_mb": int(parts[4]),
            "utilization_gpu_percent": int(parts[5]),
            "utilization_memory_percent": int(parts[6]),
        }
    except Exception as exc:
        return {"available": False, "error": type(exc).__name__}


def status_script() -> dict[str, Any]:
    try:
        proc = shell(["bash", "scripts/status.sh"], check=False)
        return {
            "exit_code": proc.returncode,
            "output": (proc.stdout or "") + (proc.stderr or ""),
        }
    except Exception as exc:
        return {"exit_code": -1, "error": type(exc).__name__}


def tmux_tail(lines: int = 80) -> str:
    try:
        return shell(
            ["tmux", "capture-pane", "-pt", "axon:0.0", "-S", f"-{lines}"],
            check=False,
        ).stdout
    except Exception:
        return ""


def collect_sample(mcp_url: str, elapsed_seconds: int) -> dict[str, Any]:
    pid = detect_axon_pid()
    proc = (
        {
            **parse_proc_status(pid),
            "cpu_percent": ps_value(pid, "%cpu"),
            "mem_percent": ps_value(pid, "%mem"),
        }
        if pid is not None
        else {}
    )
    return {
        "timestamp": utc_now_iso(),
        "elapsed_seconds": elapsed_seconds,
        "axon_pid": pid,
        "proc": proc,
        "gpu": gpu_status(),
        "status_script": status_script(),
        "mcp_status": mcp_status(mcp_url),
    }


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Periodic Axon runtime sensor logger")
    parser.add_argument("--duration", type=int, default=1200)
    parser.add_argument("--interval", type=int, default=5)
    parser.add_argument("--label", default="runtime-sensors")
    parser.add_argument("--output-root", default=str(DEFAULT_OUTPUT_ROOT))
    parser.add_argument("--mcp-url", default=DEFAULT_MCP_URL)
    parser.add_argument("--tmux-tail-lines", type=int, default=200)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if args.duration <= 0 or args.interval <= 0:
        raise SystemExit("duration and interval must be > 0")

    started = datetime.now()
    run_dir = Path(args.output_root) / (
        f"{started.strftime('%Y-%m-%dT%H-%M-%S')}-{args.label.strip().replace(' ', '-')}"
    )
    run_dir.mkdir(parents=True, exist_ok=False)
    samples_path = run_dir / "samples.ndjson"
    meta_path = run_dir / "meta.json"
    tail_path = run_dir / "tmux-tail.log"

    meta = {
        "started_at": utc_now_iso(),
        "duration_seconds": args.duration,
        "interval_seconds": args.interval,
        "mcp_url": args.mcp_url,
        "project_root": str(PROJECT_ROOT),
        "run_dir": str(run_dir),
    }
    meta_path.write_text(json.dumps(meta, indent=2, ensure_ascii=True) + "\n")

    sample_count = args.duration // args.interval
    if args.duration % args.interval:
        sample_count += 1

    start_monotonic = time.time()
    with samples_path.open("a", encoding="utf-8") as handle:
        for _ in range(sample_count):
            elapsed = int(time.time() - start_monotonic)
            sample = collect_sample(args.mcp_url, elapsed)
            handle.write(json.dumps(sample, ensure_ascii=True) + "\n")
            handle.flush()

            gpu = sample.get("gpu", {})
            mcp = (
                sample.get("mcp_status", {})
                .get("result", {})
                .get("data", {})
                .get("debug_snapshot", {})
                .get("embedding_contract", {})
            )
            vector_runtime = mcp.get("vector_runtime", {})
            print(
                "[runtime-sensor] "
                f"t={elapsed:>4}s "
                f"gpu_used_mb={gpu.get('memory_used_mb', 'ERR')} "
                f"gpu_util={gpu.get('utilization_gpu_percent', 'ERR')} "
                f"provider={mcp.get('provider_effective', 'ERR')} "
                f"embed_calls={vector_runtime.get('embed_calls_total', 'ERR')} "
                f"chunks={vector_runtime.get('chunks_embedded_total', 'ERR')} "
                f"embed_ms_chunk={vector_runtime.get('embed_ms_per_chunk', 'ERR')}"
            )
            sys.stdout.flush()
            time.sleep(args.interval)

    tail_path.write_text(tmux_tail(args.tmux_tail_lines))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
