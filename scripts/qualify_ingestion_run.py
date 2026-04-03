#!/usr/bin/env python3
"""Qualify a full Axon ingestion run with structured monitoring.

This tool exists to make runtime qualification repeatable:
- optional IST reset (enabled by default)
- clean restart in a chosen runtime mode
- structured sampling every N seconds for T seconds
- durable run folder with a lock file and logs for later analysis
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import time
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any


PROJECT_ROOT = Path("/home/dstadel/projects/axon")
GRAPH_ROOT = PROJECT_ROOT / ".axon" / "graph_v2"
IST_DB = GRAPH_ROOT / "ist.db"
IST_WAL = GRAPH_ROOT / "ist.db.wal"
SOLL_DB = GRAPH_ROOT / "sanctuary" / "soll.db"
RUNS_ROOT = PROJECT_ROOT / ".axon" / "qualification-runs"
SQL_URL = os.environ.get(
    "AXON_SQL_URL",
    os.environ.get("SQL_URL", "http://127.0.0.1:44129/sql"),
)
COCKPIT_URL = os.environ.get(
    "AXON_DASHBOARD_URL",
    "http://127.0.0.1:44127/cockpit",
)

SQL_OVERVIEW = """
SELECT
  count(*) AS known,
  COALESCE(SUM(CASE WHEN status IN ('indexed','indexed_degraded','skipped','deleted') THEN 1 ELSE 0 END), 0) AS completed,
  COALESCE(SUM(CASE WHEN status = 'pending' THEN 1 ELSE 0 END), 0) AS pending,
  COALESCE(SUM(CASE WHEN status = 'indexing' THEN 1 ELSE 0 END), 0) AS indexing,
  COALESCE(SUM(CASE WHEN status = 'indexed_degraded' THEN 1 ELSE 0 END), 0) AS degraded,
  COALESCE(SUM(CASE WHEN status = 'skipped' THEN 1 ELSE 0 END), 0) AS skipped,
  COALESCE(SUM(CASE WHEN status = 'oversized_for_current_budget' THEN 1 ELSE 0 END), 0) AS oversized,
  COALESCE(SUM(CASE WHEN graph_ready THEN 1 ELSE 0 END), 0) AS graph_ready,
  COALESCE(SUM(CASE WHEN vector_ready THEN 1 ELSE 0 END), 0) AS vector_ready
FROM File;
""".strip()

SQL_GRAPH_PROJECTION_QUEUE = """
SELECT
  COALESCE(SUM(CASE WHEN status = 'queued' THEN 1 ELSE 0 END), 0) AS queued,
  COALESCE(SUM(CASE WHEN status = 'inflight' THEN 1 ELSE 0 END), 0) AS inflight,
  COALESCE(COUNT(*), 0) AS total
FROM GraphProjectionQueue;
""".strip()

SQL_STAGE_COUNTS = """
SELECT COALESCE(file_stage, 'unknown'), count(*) AS c
FROM File
GROUP BY 1
ORDER BY c DESC, 1 ASC;
""".strip()

SQL_TOP_REASONS = """
SELECT COALESCE(status_reason, 'unknown'), count(*) AS c
FROM File
WHERE status IN ('pending', 'indexing')
GROUP BY 1
ORDER BY c DESC, 1 ASC
LIMIT 5;
""".strip()


def utc_now_iso() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="seconds")


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


def run_script(
    script: str,
    extra_args: list[str] | None = None,
    env: dict[str, str] | None = None,
    *,
    check: bool = True,
) -> tuple[int, str]:
    args = ["bash", script]
    if extra_args:
        args.extend(extra_args)
    proc = shell(args, env=env, check=check)
    return proc.returncode, (proc.stdout or "") + (proc.stderr or "")


def parse_json_payload(text: str) -> Any:
    text = text.strip()
    if not text:
        return None
    return json.loads(text)


def sql_query(query: str) -> Any:
    payload = json.dumps({"query": query})
    proc = shell(
        [
            "curl",
            "-sS",
            "-X",
            "POST",
            SQL_URL,
            "-H",
            "Content-Type: application/json",
            "-d",
            payload,
        ],
        capture=True,
    )
    return parse_json_payload(proc.stdout)


def cockpit_html() -> str:
    proc = shell(["curl", "-sS", COCKPIT_URL], capture=True)
    return proc.stdout


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


def git_context() -> dict[str, str]:
    def out(args: list[str]) -> str:
        try:
            return shell(args, capture=True).stdout.strip()
        except subprocess.CalledProcessError:
            return ""

    return {
        "branch": out(["git", "rev-parse", "--abbrev-ref", "HEAD"]),
        "commit": out(["git", "rev-parse", "HEAD"]),
        "dirty": "true" if out(["git", "status", "--short"]) else "false",
    }


def parse_proc_status(pid: int) -> dict[str, int]:
    status_path = Path("/proc") / str(pid) / "status"
    result = {
        "rss_bytes": 0,
        "rss_anon_bytes": 0,
        "rss_file_bytes": 0,
        "rss_shmem_bytes": 0,
    }

    try:
        text = status_path.read_text()
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
        return shell(["ps", "-o", f"{field}=", "-p", str(pid)], capture=True).stdout.strip()
    except subprocess.CalledProcessError:
        return ""


def file_size(path: Path) -> int:
    try:
        return path.stat().st_size
    except FileNotFoundError:
        return 0


def db_sizes() -> dict[str, int]:
    db_file_bytes = file_size(IST_DB)
    db_wal_bytes = file_size(IST_WAL)
    return {
        "db_file_bytes": db_file_bytes,
        "db_wal_bytes": db_wal_bytes,
        "db_total_bytes": db_file_bytes + db_wal_bytes,
    }


def parse_int(value: Any) -> int:
    if value is None:
        return 0
    if isinstance(value, bool):
        return int(value)
    if isinstance(value, int):
        return value
    if isinstance(value, float):
        return int(round(value))
    if isinstance(value, str):
        try:
            return int(value.replace(",", "").strip())
        except ValueError:
            try:
                return int(round(float(value.replace(",", "").strip())))
            except ValueError:
                return 0
    return 0


def parse_file_indexed_stats(tail: str) -> dict[str, int]:
    queue_wait_max = 0
    parse_us_max = 0
    commit_us_max = 0
    parsed_events = 0

    for line in tail.splitlines():
        if '"FileIndexed"' not in line:
            continue
        start = line.find('"FileIndexed"')
        if start == -1:
            continue
        obj_start = line.rfind("{", 0, start)
        if obj_start == -1:
            continue

        depth = 0
        obj_end = None
        for idx in range(obj_start, len(line)):
            if line[idx] == "{":
                depth += 1
            elif line[idx] == "}":
                depth -= 1
                if depth == 0:
                    obj_end = idx + 1
                    break

        if obj_end is None:
            continue

        try:
            payload = json.loads(line[obj_start:obj_end])
        except (json.JSONDecodeError, ValueError):
            continue

        file_indexed = payload.get("FileIndexed")
        if not isinstance(file_indexed, dict):
            continue

        parsed_events += 1
        queue_wait_max = max(queue_wait_max, parse_int(file_indexed.get("queue_wait_us")))
        parse_us_max = max(parse_us_max, parse_int(file_indexed.get("parse_us")))
        commit_us_max = max(commit_us_max, parse_int(file_indexed.get("commit_us")))

    return {
        "parsed_file_indexed_events": parsed_events,
        "max_queue_wait_us": queue_wait_max,
        "max_parse_us": parse_us_max,
        "max_commit_us": commit_us_max,
    }


def sql_overview() -> dict[str, int]:
    rows = sql_query(SQL_OVERVIEW)
    if not isinstance(rows, list) or not rows:
        return {
            "known": 0,
            "completed": 0,
            "pending": 0,
            "indexing": 0,
            "degraded": 0,
            "skipped": 0,
            "oversized": 0,
            "graph_ready": 0,
            "vector_ready": 0,
        }
    row = rows[0]
    if not isinstance(row, list):
        return {
            "known": 0,
            "completed": 0,
            "pending": 0,
            "indexing": 0,
            "degraded": 0,
            "skipped": 0,
            "oversized": 0,
            "graph_ready": 0,
            "vector_ready": 0,
        }
    padded = row + [0] * (9 - len(row))
    return {
        "known": parse_int(padded[0]),
        "completed": parse_int(padded[1]),
        "pending": parse_int(padded[2]),
        "indexing": parse_int(padded[3]),
        "degraded": parse_int(padded[4]),
        "skipped": parse_int(padded[5]),
        "oversized": parse_int(padded[6]),
        "graph_ready": parse_int(padded[7]),
        "vector_ready": parse_int(padded[8]),
    }


def sql_top_reasons() -> list[dict[str, Any]]:
    rows = sql_query(SQL_TOP_REASONS)
    if not isinstance(rows, list):
        return []
    reasons = []
    for row in rows:
        if isinstance(row, list) and len(row) >= 2:
            reasons.append({"reason": str(row[0]), "count": parse_int(row[1])})
    return reasons


def sql_stage_counts() -> list[dict[str, Any]]:
    rows = sql_query(SQL_STAGE_COUNTS)
    if not isinstance(rows, list):
        return []
    stages = []
    for row in rows:
        if isinstance(row, list) and len(row) >= 2:
            stages.append({"stage": str(row[0]), "count": parse_int(row[1])})
    return stages


def sql_graph_projection_queue() -> dict[str, int]:
    rows = sql_query(SQL_GRAPH_PROJECTION_QUEUE)
    if not isinstance(rows, list) or not rows:
        return {
            "queued": 0,
            "inflight": 0,
            "total": 0,
        }

    row = rows[0]
    if not isinstance(row, list) or len(row) < 3:
        return {
            "queued": 0,
            "inflight": 0,
            "total": 0,
        }

    return {
        "queued": parse_int(row[0]),
        "inflight": parse_int(row[1]),
        "total": parse_int(row[2]),
    }


def extract_html_value(html: str, pattern: str) -> str:
    match = re.search(pattern, html, re.S)
    return match.group(1).strip() if match else ""


def cockpit_metrics(html: str) -> dict[str, Any]:
    return {
        "known": parse_int(
            extract_html_value(html, r"Known Files</p>\s*<p[^>]*class=\"metric-value\">([^<]+)")
        ),
        "completed": parse_int(
            extract_html_value(html, r"Completed</p>\s*<p[^>]*class=\"metric-value\">([^<]+)")
        ),
        "graph_ready": parse_int(
            extract_html_value(html, r"Graph Ready</p>\s*<p[^>]*class=\"metric-value\">([^<]+)")
        ),
        "vector_ready": parse_int(
            extract_html_value(html, r"Vector Ready</p>\s*<p[^>]*class=\"metric-value\">([^<]+)")
        ),
        "indexing": parse_int(
            extract_html_value(html, r"Indexing</p>\s*<p[^>]*class=\"metric-value\">([^<]+)")
        ),
        "pending": parse_int(
            extract_html_value(html, r"Pending</p>\s*<p[^>]*class=\"metric-value\">([^<]+)")
        ),
        "degraded": parse_int(
            extract_html_value(html, r"Degraded</p>\s*<p[^>]*class=\"metric-value\">([^<]+)")
        ),
        "skipped": parse_int(
            extract_html_value(html, r"Skipped</p>\s*<p[^>]*class=\"metric-value\">([^<]+)")
        ),
        "buffered_entries": parse_int(
            extract_html_value(
                html,
                r"Buffered Entries</span>\s*<strong[^>]*class=\"signal-stat-value\">([^<]+)",
            )
        ),
        "subtree_hints": parse_int(
            extract_html_value(
                html,
                r"Subtree Hints</span>\s*<strong[^>]*class=\"signal-stat-value\">([^<]+)",
            )
        ),
        "subtree_hint_in_flight": parse_int(
            extract_html_value(
                html,
                r"Hint In Flight</span>\s*<strong[^>]*class=\"signal-stat-value\">([^<]+)",
            )
        ),
        "subtree_hint_accepted_total": parse_int(
            extract_html_value(
                html,
                r"Hint Accepted</span>\s*<strong[^>]*class=\"signal-stat-value\">([^<]+)",
            )
        ),
        "subtree_hint_blocked_total": parse_int(
            extract_html_value(
                html,
                r"Hint Blocked</span>\s*<strong[^>]*class=\"signal-stat-value\">([^<]+)",
            )
        ),
        "subtree_hint_suppressed_total": parse_int(
            extract_html_value(
                html,
                r"Hint Suppressed</span>\s*<strong[^>]*class=\"signal-stat-value\">([^<]+)",
            )
        ),
        "flush_count": parse_int(
            extract_html_value(
                html,
                r"Flush Count</span>\s*<strong[^>]*class=\"signal-stat-value\">([^<]+)",
            )
        ),
        "graph_projection_queue": {
            "queued": parse_int(
                extract_html_value(
                    html,
                    r"Graph Projection Queued</span>\s*<strong[^>]*class=\"signal-stat-value\">([^<]+)",
                )
            ),
            "inflight": parse_int(
                extract_html_value(
                    html,
                    r"Graph Projection In-Flight</span>\s*<strong[^>]*class=\"signal-stat-value\">([^<]+)",
                )
            ),
            "total": parse_int(
                extract_html_value(
                    html,
                    r"Graph Projection Pending</span>\s*<strong[^>]*class=\"signal-stat-value\">([^<]+)",
                )
            ),
        },
        "last_promoted_count": parse_int(
            extract_html_value(
                html,
                r"Last Promoted</span>\s*<strong[^>]*class=\"signal-stat-value\">([^<]+)",
            )
        ),
        "claim_mode": extract_html_value(
            html, r"Claim Mode</span>\s*<strong[^>]*class=\"signal-stat-value\">([^<]+)"
        ),
        "service_pressure": extract_html_value(
            html, r"Service Pressure</span>\s*<strong[^>]*class=\"signal-stat-value\">([^<]+)"
        ),
        "bridge": extract_html_value(
            html, r"Bridge</span>\s*<strong[^>]*class=\"signal-stat-value\">([^<]+)"
        ),
        "sql_snapshot": extract_html_value(
            html, r"SQL Snapshot</span>\s*<strong[^>]*class=\"signal-stat-value\">([^<]+)"
        ),
    }


def capture_tmux_tail(lines: int = 400) -> str:
    try:
        return shell(
            ["tmux", "capture-pane", "-pt", "axon:core", "-S", f"-{lines}"],
            capture=True,
        ).stdout
    except subprocess.CalledProcessError:
        return ""


def wait_for_runtime(timeout_s: int = 180) -> int:
    deadline = time.time() + timeout_s
    last_pid = None
    while time.time() < deadline:
        pid = detect_axon_pid()
        if pid is not None:
            last_pid = pid
            try:
                shell(["curl", "-sS", "-i", COCKPIT_URL], capture=True)
                return pid
            except subprocess.CalledProcessError:
                pass
        time.sleep(1)
    raise RuntimeError(f"Axon runtime not ready after {timeout_s}s (last pid={last_pid})")


def runtime_is_up() -> bool:
    return detect_axon_pid() is not None


def sanitize_label(value: str) -> str:
    cleaned = re.sub(r"[^a-zA-Z0-9._-]+", "-", value.strip()).strip("-")
    return cleaned or "run"


@dataclass
class Args:
    duration: int
    interval: int
    mode: str
    reset_ist: bool
    keep_running: bool
    label: str
    output_root: Path


def build_arg_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Run a full Axon ingestion qualification with reset, restart, monitoring, and durable logs."
    )
    parser.add_argument("--duration", type=int, default=300, help="Monitoring duration in seconds. Default: 300")
    parser.add_argument("--interval", type=int, default=5, help="Sampling interval in seconds. Default: 5")
    parser.add_argument(
        "--mode",
        choices=["full", "read_only", "mcp_only"],
        default="full",
        help="Runtime mode passed to start-v2.sh. Default: full",
    )
    parser.add_argument(
        "--label",
        default="qualify-ingestion",
        help="Short label included in the run directory name.",
    )
    parser.add_argument(
        "--output-root",
        default=str(RUNS_ROOT),
        help=f"Directory where run artifacts are stored. Default: {RUNS_ROOT}",
    )
    parser.add_argument(
        "--no-reset-ist",
        action="store_true",
        help="Do not delete ist.db / ist.db.wal before restart. By default they are reset.",
    )
    parser.add_argument(
        "--stop-after",
        action="store_true",
        help="Stop Axon again after the monitoring window completes.",
    )
    return parser


def parse_args() -> Args:
    ns = build_arg_parser().parse_args()
    if ns.duration <= 0:
        raise SystemExit("--duration must be > 0")
    if ns.interval <= 0:
        raise SystemExit("--interval must be > 0")
    if ns.interval > ns.duration:
        raise SystemExit("--interval must be <= --duration")
    return Args(
        duration=ns.duration,
        interval=ns.interval,
        mode=ns.mode,
        reset_ist=not ns.no_reset_ist,
        keep_running=not ns.stop_after,
        label=sanitize_label(ns.label),
        output_root=Path(ns.output_root),
    )


def write_json(path: Path, payload: Any) -> None:
    path.write_text(json.dumps(payload, indent=2, ensure_ascii=True) + "\n")


def main() -> int:
    args = parse_args()
    started_at = datetime.now()
    run_name = f"{started_at.strftime('%Y-%m-%dT%H-%M-%S')}-{args.mode}-{args.label}"
    run_dir = args.output_root / run_name
    run_dir.mkdir(parents=True, exist_ok=False)

    lock_path = run_dir / "run.lock.json"
    samples_path = run_dir / "samples.ndjson"
    summary_path = run_dir / "summary.json"
    notes_path = run_dir / "notes.txt"
    tmux_tail_path = run_dir / "tmux-tail.log"
    start_log_path = run_dir / "start.log"
    stop_log_path = run_dir / "stop.log"

    lock = {
        "schema_version": 1,
        "created_at": utc_now_iso(),
        "label": args.label,
        "mode": args.mode,
        "duration_seconds": args.duration,
        "interval_seconds": args.interval,
        "reset_ist": args.reset_ist,
        "keep_running": args.keep_running,
        "paths": {
            "project_root": str(PROJECT_ROOT),
            "graph_root": str(GRAPH_ROOT),
            "ist_db": str(IST_DB),
            "ist_wal": str(IST_WAL),
            "soll_db": str(SOLL_DB),
            "run_dir": str(run_dir),
        },
        "git": git_context(),
        "commands": {
            "stop": "bash scripts/stop-v2.sh",
            "start": f"bash scripts/start-v2.sh --{args.mode.replace('_', '-')}",
        },
    }
    write_json(lock_path, lock)

    print(f"[qualify] run_dir={run_dir}")
    print(f"[qualify] reset_ist={args.reset_ist} mode={args.mode} duration={args.duration}s interval={args.interval}s")

    stop_code, stop_output = run_script("scripts/stop-v2.sh", check=False)
    stop_log_path.write_text(stop_output)
    if stop_code != 0 and runtime_is_up():
        raise RuntimeError(
            f"stop-v2.sh returned {stop_code} and axon-core is still running; see {stop_log_path}"
        )

    if args.reset_ist:
        for path in [IST_DB, IST_WAL]:
            try:
                path.unlink()
            except FileNotFoundError:
                pass

    start_mode_arg = {
        "full": "--full",
        "read_only": "--read-only",
        "mcp_only": "--mcp-only",
    }[args.mode]
    start_code, start_output = run_script(
        "scripts/start-v2.sh", [start_mode_arg], check=False
    )
    start_log_path.write_text(start_output)
    if start_code != 0 and not runtime_is_up():
        raise RuntimeError(
            f"start-v2.sh returned {start_code} and runtime is not up; see {start_log_path}"
        )
    pid = wait_for_runtime()
    lock["runtime"] = {
        "pid": pid,
        "started_at": utc_now_iso(),
        "start_exit_code": start_code,
        "stop_exit_code": stop_code,
    }
    write_json(lock_path, lock)

    samples: list[dict[str, Any]] = []
    samples_path.touch()
    started_monotonic = time.time()
    sample_count = args.duration // args.interval
    if args.duration % args.interval:
        sample_count += 1

    with samples_path.open("a", encoding="utf-8") as handle:
        for _ in range(sample_count):
            ts = utc_now_iso()
            current_pid = detect_axon_pid()
            sample: dict[str, Any] = {
                "timestamp": ts,
                "elapsed_seconds": int(time.time() - started_monotonic),
                "pid": current_pid,
            }
            if current_pid is not None:
                sample["proc"] = {
                    **parse_proc_status(current_pid),
                    "cpu_percent": ps_value(current_pid, "%cpu"),
                }
            else:
                sample["proc"] = {
                    "rss_bytes": 0,
                    "rss_anon_bytes": 0,
                    "rss_file_bytes": 0,
                    "rss_shmem_bytes": 0,
                    "cpu_percent": "",
                }

            sample["db"] = db_sizes()

            try:
                sample["sql"] = sql_overview()
                sample["sql"]["top_reasons"] = sql_top_reasons()
                sample["sql"]["stages"] = sql_stage_counts()
                sample["sql"]["graph_projection_queue"] = sql_graph_projection_queue()
            except Exception as exc:
                sample["sql_error"] = type(exc).__name__
                sample["sql"] = {}

            try:
                html = cockpit_html()
                sample["cockpit"] = cockpit_metrics(html)
            except Exception as exc:
                sample["cockpit_error"] = type(exc).__name__
                sample["cockpit"] = {}

            handle.write(json.dumps(sample, ensure_ascii=True) + "\n")
            handle.flush()
            samples.append(sample)

            sql = sample.get("sql", {})
            cockpit = sample.get("cockpit", {})
            graph_projection_queue = sample.get("cockpit", {}).get("graph_projection_queue", {})
            sql_gpq = sample.get("sql", {}).get("graph_projection_queue", {})
            proc = sample.get("proc", {})
            print(
                "[sample] "
                f"t={sample['elapsed_seconds']:>4}s "
                f"sql_known={sql.get('known', 'ERR')} "
                f"sql_done={sql.get('completed', 'ERR')} "
                f"sql_pending={sql.get('pending', 'ERR')} "
                f"sql_indexing={sql.get('indexing', 'ERR')} "
                f"graph_ready={sql.get('graph_ready', 'ERR')} "
                f"vector_ready={sql.get('vector_ready', 'ERR')} "
                f"gpq_total={sql_gpq.get('total', 'ERR')} "
                f"gpq_queued={sql_gpq.get('queued', 'ERR')} "
                f"gpq_inflight={sql_gpq.get('inflight', 'ERR')} "
                f"gpq_runtime_queued={graph_projection_queue.get('queued', 'ERR')} "
                f"gpq_runtime_inflight={graph_projection_queue.get('inflight', 'ERR')} "
                f"cockpit_known={cockpit.get('known', '')} "
                f"buffered={cockpit.get('buffered_entries', '')} "
                f"hints={cockpit.get('subtree_hints', '')} "
                f"hint_in_flight={cockpit.get('subtree_hint_in_flight', '')} "
                f"hint_blocked={cockpit.get('subtree_hint_blocked_total', '')} "
                f"hint_suppressed={cockpit.get('subtree_hint_suppressed_total', '')} "
                f"rss_anon_mb={int(proc.get('rss_anon_bytes', 0) / (1024 * 1024))}"
            )
            sys.stdout.flush()
            time.sleep(args.interval)

    tail = capture_tmux_tail()
    tmux_tail_path.write_text(tail)
    file_indexed_stats = parse_file_indexed_stats(tail)

    max_rss_anon = max(
        int(sample.get("proc", {}).get("rss_anon_bytes", 0)) for sample in samples
    ) if samples else 0
    max_buffered = max(
        int(sample.get("cockpit", {}).get("buffered_entries", 0)) for sample in samples
    ) if samples else 0
    max_hints = max(
        int(sample.get("cockpit", {}).get("subtree_hints", 0)) for sample in samples
    ) if samples else 0
    max_hints_in_flight = max(
        int(sample.get("cockpit", {}).get("subtree_hint_in_flight", 0)) for sample in samples
    ) if samples else 0
    max_hint_blocked_total = max(
        int(sample.get("cockpit", {}).get("subtree_hint_blocked_total", 0)) for sample in samples
    ) if samples else 0
    max_hint_suppressed_total = max(
        int(sample.get("cockpit", {}).get("subtree_hint_suppressed_total", 0)) for sample in samples
    ) if samples else 0
    max_graph_projection_queue_total = max(
        int(sample.get("sql", {}).get("graph_projection_queue", {}).get("total", 0))
        for sample in samples
    ) if samples else 0
    max_graph_projection_queue_queued = max(
        int(sample.get("sql", {}).get("graph_projection_queue", {}).get("queued", 0))
        for sample in samples
    ) if samples else 0
    max_graph_projection_queue_inflight = max(
        int(sample.get("sql", {}).get("graph_projection_queue", {}).get("inflight", 0))
        for sample in samples
    ) if samples else 0
    max_graph_projection_queue_runtime_queued = max(
        int(sample.get("cockpit", {}).get("graph_projection_queue", {}).get("queued", 0))
        for sample in samples
    ) if samples else 0
    max_graph_projection_queue_runtime_inflight = max(
        int(sample.get("cockpit", {}).get("graph_projection_queue", {}).get("inflight", 0))
        for sample in samples
    ) if samples else 0
    sql_known_values = [int(s.get("sql", {}).get("known", 0)) for s in samples if s.get("sql")]
    cockpit_known_values = [int(s.get("cockpit", {}).get("known", 0)) for s in samples if s.get("cockpit")]
    divergence_samples = 0
    for sample in samples:
        sql_known = sample.get("sql", {}).get("known")
        cockpit_known = sample.get("cockpit", {}).get("known")
        if isinstance(sql_known, int) and isinstance(cockpit_known, int) and sql_known != cockpit_known:
            divergence_samples += 1

    final_sample = samples[-1] if samples else {}
    summary = {
        "created_at": utc_now_iso(),
        "run_dir": str(run_dir),
        "sample_count": len(samples),
        "parsed_file_indexed_events": file_indexed_stats["parsed_file_indexed_events"],
        "max_file_queue_wait_us": file_indexed_stats["max_queue_wait_us"],
        "max_file_parse_us": file_indexed_stats["max_parse_us"],
        "max_file_commit_us": file_indexed_stats["max_commit_us"],
        "max_graph_projection_queue_total": max_graph_projection_queue_total,
        "max_graph_projection_queue_queued": max_graph_projection_queue_queued,
        "max_graph_projection_queue_inflight": max_graph_projection_queue_inflight,
        "max_graph_projection_queue_runtime_queued": max_graph_projection_queue_runtime_queued,
        "max_graph_projection_queue_runtime_inflight": max_graph_projection_queue_runtime_inflight,
        "max_rss_anon_bytes": max_rss_anon,
        "max_buffered_entries": max_buffered,
        "max_subtree_hints": max_hints,
        "max_subtree_hint_in_flight": max_hints_in_flight,
        "max_subtree_hint_blocked_total": max_hint_blocked_total,
        "max_subtree_hint_suppressed_total": max_hint_suppressed_total,
        "sql_known_first": sql_known_values[0] if sql_known_values else 0,
        "sql_known_last": sql_known_values[-1] if sql_known_values else 0,
        "cockpit_known_first": cockpit_known_values[0] if cockpit_known_values else 0,
        "cockpit_known_last": cockpit_known_values[-1] if cockpit_known_values else 0,
        "known_divergence_samples": divergence_samples,
        "final_sample": final_sample,
    }
    write_json(summary_path, summary)

    notes = [
        f"Run directory: {run_dir}",
        f"Mode: {args.mode}",
        f"Reset IST: {args.reset_ist}",
        f"Duration: {args.duration}s",
        f"Interval: {args.interval}s",
        f"Samples: {len(samples)}",
        f"Max RssAnon MB: {int(max_rss_anon / (1024 * 1024))}",
        f"Max Buffered Entries: {max_buffered}",
        f"Max Subtree Hints: {max_hints}",
        f"Max Subtree Hint In Flight: {max_hints_in_flight}",
        f"Max Subtree Hint Blocked Total: {max_hint_blocked_total}",
        f"Max Subtree Hint Suppressed Total: {max_hint_suppressed_total}",
        f"SQL/Cockpit known divergence samples: {divergence_samples}",
        f"Final SQL known: {final_sample.get('sql', {}).get('known', 'ERR')}",
        f"Final SQL completed: {final_sample.get('sql', {}).get('completed', 'ERR')}",
        f"Final SQL graph projection queued: {final_sample.get('sql', {}).get('graph_projection_queue', {}).get('queued', 'ERR')}",
        f"Final SQL graph projection inflight: {final_sample.get('sql', {}).get('graph_projection_queue', {}).get('inflight', 'ERR')}",
        f"Final SQL graph projection total: {final_sample.get('sql', {}).get('graph_projection_queue', {}).get('total', 'ERR')}",
        f"Final cockpit buffered: {final_sample.get('cockpit', {}).get('buffered_entries', 'ERR')}",
        f"Final cockpit graph projection queued: {final_sample.get('cockpit', {}).get('graph_projection_queue', {}).get('queued', 'ERR')}",
        f"Final cockpit graph projection inflight: {final_sample.get('cockpit', {}).get('graph_projection_queue', {}).get('inflight', 'ERR')}",
        f"FileIndexed events parsed from runtime log: {file_indexed_stats['parsed_file_indexed_events']}",
        f"Max FileIndexed queue_wait_us: {file_indexed_stats['max_queue_wait_us']}",
        f"Max FileIndexed parse_us: {file_indexed_stats['max_parse_us']}",
        f"Max FileIndexed commit_us: {file_indexed_stats['max_commit_us']}",
    ]
    notes_path.write_text("\n".join(notes) + "\n")

    if not args.keep_running:
        stop_after_code, stop_after_output = run_script(
            "scripts/stop-v2.sh", check=False
        )
        stop_log_path.write_text(
            stop_log_path.read_text()
            + "\n\n--- stop-after ---\n"
            + stop_after_output
        )
        if stop_after_code != 0 and runtime_is_up():
            raise RuntimeError(
                f"stop-v2.sh --stop-after returned {stop_after_code} and runtime is still up; see {stop_log_path}"
            )

    print(f"[qualify] summary={summary_path}")
    print(f"[qualify] samples={samples_path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
