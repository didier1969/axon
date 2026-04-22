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
RUNS_ROOT = PROJECT_ROOT / ".axon" / "qualification-runs"
QUALIFY_PROJECT = os.environ.get("AXON_QUALIFY_PROJECT", "BookingSystem")


def current_instance_kind() -> str:
    raw = os.environ.get("AXON_INSTANCE_KIND", "").strip().lower()
    if raw in {"dev", "live"}:
        return raw
    return "dev"


def current_graph_root() -> Path:
    configured = os.environ.get("AXON_DB_ROOT", "").strip()
    if configured:
        return Path(configured)
    if current_instance_kind() == "dev":
        return PROJECT_ROOT / ".axon-dev" / "graph_v2"
    return PROJECT_ROOT / ".axon" / "graph_v2"


def current_ist_db() -> Path:
    return current_graph_root() / "ist.db"


def current_ist_wal() -> Path:
    return current_graph_root() / "ist.db.wal"


def current_soll_db() -> Path:
    graph_root = current_graph_root()
    dev_soll = graph_root / "soll.db"
    if dev_soll.exists():
        return dev_soll
    return graph_root / "sanctuary" / "soll.db"


def current_sql_url() -> str:
    configured = os.environ.get("AXON_SQL_URL", "").strip() or os.environ.get("SQL_URL", "").strip()
    if configured:
        return configured
    if current_instance_kind() == "dev":
        return "http://127.0.0.1:44139/sql"
    return "http://127.0.0.1:44129/sql"


def current_cockpit_url() -> str:
    configured = os.environ.get("AXON_DASHBOARD_URL", "").strip()
    if configured:
        return configured.rstrip("/") + "/cockpit"
    if current_instance_kind() == "dev":
        return "http://127.0.0.1:44137/cockpit"
    return "http://127.0.0.1:44127/cockpit"


def current_mcp_url() -> str:
    configured = os.environ.get("AXON_MCP_URL", "").strip()
    if configured:
        return configured
    if current_instance_kind() == "dev":
        return "http://127.0.0.1:44139/mcp"
    return "http://127.0.0.1:44129/mcp"


def current_run_root(role: str) -> Path:
    instance_root = PROJECT_ROOT / (".axon-dev" if current_instance_kind() == "dev" else ".axon")
    return instance_root / ("run-indexer" if role == "indexer" else "run-brain")


def shadow_role_for_mode(mode: str) -> str:
    if mode == "brain_shadow":
        return "brain"
    if mode == "indexer_shadow":
        return "indexer"
    return "legacy_monolith"


def expected_runtime_mode_for_mode(mode: str) -> str:
    if mode == "brain_shadow":
        return "mcp_only"
    if mode == "indexer_shadow":
        return "graph_only"
    return mode


def start_command_for_mode(mode: str) -> list[str]:
    if mode == "brain_shadow":
        return ["bash", "scripts/start-brain.sh"]
    if mode == "indexer_shadow":
        return ["bash", "scripts/start-indexer.sh"]
    return ["bash", "scripts/start.sh", f"--{mode.replace('_', '-')}", "--skip-mcp-tests"]


def status_data_from_mcp() -> dict[str, Any]:
    response = mcp_call(
        "status",
        {
            "mode": "json",
        },
    )
    result = response.get("result", {})
    if isinstance(result, dict):
        data = result.get("data")
        if isinstance(data, dict):
            return data
        content = result.get("content")
        if isinstance(content, list) and content:
            first = content[0]
            if isinstance(first, dict):
                text = first.get("text")
                if isinstance(text, str) and text.strip():
                    try:
                        parsed = json.loads(text)
                        if isinstance(parsed, dict):
                            return parsed
                    except json.JSONDecodeError:
                        return {}
    return {}


def _read_json_file(path: Path) -> dict[str, Any]:
    try:
        payload = json.loads(path.read_text())
    except Exception:
        return {}
    return payload if isinstance(payload, dict) else {}


def status_data_from_local_indexer_runtime() -> dict[str, Any]:
    run_root = current_run_root("indexer")
    heartbeat = _read_json_file(run_root / "runtime-heartbeat.json")
    telemetry = heartbeat.get("runtime_telemetry")
    if not isinstance(telemetry, dict):
        telemetry = {}
    feed = heartbeat.get("runtime_truth_feed")
    if not isinstance(feed, dict):
        feed = {}

    pid_live = False
    pid_file = run_root / "axon-indexer.pid"
    if pid_file.exists():
        try:
            pid_live = (Path("/proc") / pid_file.read_text().strip()).exists()
        except Exception:
            pid_live = False

    ist_reader = current_graph_root() / "ist-reader.db"
    feed_state = str(feed.get("state") or ("fresh" if pid_live else "stale"))
    feed_stale = feed.get("stale")
    if not isinstance(feed_stale, bool):
        feed_stale = not pid_live

    graph_projection_queue = telemetry.get("graph_projection_queue")
    if not isinstance(graph_projection_queue, dict):
        graph_projection_queue = {}

    data = {
        "runtime_mode": "graph_only",
        "truth_status": "canonical" if pid_live and not feed_stale else "degraded",
        "runtime_authority": {
            "runtime_topology": {
                "topology": "brain_indexer_split",
                "process_role": "indexer",
                "public_mcp_authority": "brain",
                "soll_writer_authority": "brain",
                "ist_writer_authority": "indexer",
                "brain_ready": False,
                "indexer_ready": pid_live,
                "indexer_feed": {
                    "state": feed_state,
                    "stale": feed_stale,
                },
                "ist_snapshot": {
                    "state": "fresh" if ist_reader.exists() else "stale",
                    "stale": not ist_reader.exists(),
                },
            },
            "canonical_ingestion_stage_model": {
                "ingress_buffered": {
                    "current_count": telemetry.get("ingress_buffered_entries", 0)
                },
                "scan_buffered": {
                    "current_count": telemetry.get("ingress_scan_entries", 0)
                },
                "watcher_buffered": {
                    "current_count": telemetry.get("ingress_hot_entries", 0)
                },
                "graph_projection_queue_owned": {
                    "current_count": graph_projection_queue.get("total", 0),
                    "queue_breakdown": {
                        "queued": graph_projection_queue.get("queued", 0),
                        "inflight": graph_projection_queue.get("inflight", 0),
                    },
                },
            },
            "admission_controller": {
                "admission_flush_count": telemetry.get("ingress_flush_count", 0),
                "admission_last_promoted_count": telemetry.get(
                    "ingress_last_promoted_count", 0
                ),
                "admission_last_durably_persisted_count": 0,
                "admission_last_excluded_from_pending_count": 0,
                "admission_wip_current": graph_projection_queue.get("total", 0),
                "blocking_authority": telemetry.get(
                    "utility_first_scheduler_reason", "unknown"
                ),
                "target_band": 0,
                "reorder_point": 0,
                "max_wip": 0,
                "forced_bulk_fill_threshold": 0,
                "bulk_fill_preferred": False,
            },
            "quiescent_state": {
                "service_pressure": telemetry.get("service_pressure", "unknown")
            },
            "utility_first_scheduler": {
                "state": telemetry.get("utility_first_scheduler_state", ""),
                "reason": telemetry.get("utility_first_scheduler_reason", ""),
            },
        },
    }
    return data


def status_data_for_mode(mode: str) -> dict[str, Any]:
    try:
        status = status_data_from_mcp()
    except Exception:
        status = {}
    if status:
        return status
    if mode == "indexer_shadow":
        return status_data_from_local_indexer_runtime()
    return {}


def runtime_status_matches_mode(status_data: dict[str, Any], mode: str) -> tuple[bool, str]:
    if not isinstance(status_data, dict) or not status_data:
        return False, "missing_status_data"

    runtime_authority = status_data.get("runtime_authority", {})
    if not isinstance(runtime_authority, dict):
        return False, "missing_runtime_authority"

    topology = runtime_authority.get("runtime_topology", {})
    if not isinstance(topology, dict):
        return False, "missing_runtime_topology"

    if status_data.get("truth_status") is None:
        return False, "missing_truth_status"

    topology_name = topology.get("topology")
    if mode == "graph_only" and topology_name == "brain_indexer_split":
        if topology.get("indexer_ready") is not True:
            return False, "split_graph_only_not_indexer_ready"
        if topology.get("ist_writer_authority") != "indexer":
            return False, "split_graph_only_wrong_ist_writer_authority"
        indexer_feed = topology.get("indexer_feed", {})
        if isinstance(indexer_feed, dict) and indexer_feed.get("state") == "fresh":
            return True, "ready_via_brain_indexer_feed"
        return False, "split_graph_only_indexer_feed_not_fresh"

    expected_runtime_mode = expected_runtime_mode_for_mode(mode)
    runtime_mode = status_data.get("runtime_mode")
    if runtime_mode != expected_runtime_mode:
        return False, f"runtime_mode_mismatch expected={expected_runtime_mode} got={runtime_mode!r}"

    if mode == "brain_shadow":
        if topology.get("process_role") != "brain":
            return False, "brain_shadow_wrong_process_role"
        if topology.get("brain_ready") is not True:
            return False, "brain_shadow_not_brain_ready"
        if topology.get("public_mcp_authority") != "brain":
            return False, "brain_shadow_wrong_public_mcp_authority"
        if topology.get("soll_writer_authority") != "brain":
            return False, "brain_shadow_wrong_soll_writer_authority"
    elif mode == "indexer_shadow":
        if topology.get("process_role") != "indexer":
            return False, "indexer_shadow_wrong_process_role"
        if topology.get("indexer_ready") is not True:
            return False, "indexer_shadow_not_indexer_ready"
        if topology.get("ist_writer_authority") != "indexer":
            return False, "indexer_shadow_wrong_ist_writer_authority"

    return True, "ready"


def wait_for_runtime_contract(mode: str, timeout_s: int = 180) -> tuple[int | None, dict[str, Any]]:
    deadline = time.time() + timeout_s
    last_pid: int | None = None
    last_reason = "unknown"
    while time.time() < deadline:
        last_pid = detect_axon_pid()
        if last_pid is None:
            time.sleep(1)
            continue
        status_data = status_data_for_mode(mode)
        if not status_data:
            last_reason = "missing_status_data"
            time.sleep(1)
            continue
        ready, reason = runtime_status_matches_mode(status_data, mode)
        last_reason = reason
        if ready:
            return last_pid, status_data
        time.sleep(1)
    raise RuntimeError(
        f"Axon runtime not ready after {timeout_s}s for mode={mode} "
        f"(last pid={last_pid}, reason={last_reason})"
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
            current_sql_url(),
            "-H",
            "Content-Type: application/json",
            "-d",
            payload,
        ],
        capture=True,
    )
    return parse_json_payload(proc.stdout)


def cockpit_html() -> str:
    proc = shell(["curl", "-sS", current_cockpit_url()], capture=True)
    return proc.stdout


def nested_value(payload: dict[str, Any], *path: str) -> Any:
    current: Any = payload
    for key in path:
        if not isinstance(current, dict):
            return None
        current = current.get(key)
    return current


def runtime_metrics_from_status(status_data: dict[str, Any]) -> dict[str, Any]:
    if not isinstance(status_data, dict) or not status_data:
        return {}

    runtime_authority = status_data.get("runtime_authority")
    if not isinstance(runtime_authority, dict):
        return {}

    stage_model = runtime_authority.get("canonical_ingestion_stage_model")
    if not isinstance(stage_model, dict):
        stage_model = {}

    admission = runtime_authority.get("admission_controller")
    if not isinstance(admission, dict):
        admission = {}

    graph_projection_queue = nested_value(
        stage_model, "graph_projection_queue_owned", "queue_breakdown"
    )
    if not isinstance(graph_projection_queue, dict):
        graph_projection_queue = {}

    quiescent_state = runtime_authority.get("quiescent_state")
    if not isinstance(quiescent_state, dict):
        quiescent_state = {}

    utility_scheduler = runtime_authority.get("utility_first_scheduler")
    if not isinstance(utility_scheduler, dict):
        utility_scheduler = {}

    runtime_topology = runtime_authority.get("runtime_topology")
    if not isinstance(runtime_topology, dict):
        runtime_topology = {}
    process_role = str(runtime_topology.get("process_role", ""))
    peer_runtime = runtime_topology.get("indexer_runtime")
    if not isinstance(peer_runtime, dict):
        peer_runtime = {}
    peer_telemetry = peer_runtime.get("telemetry")
    if not isinstance(peer_telemetry, dict):
        peer_telemetry = {}

    telemetry_source = "status_json"
    telemetry = {
        "buffered_entries": parse_int(
            nested_value(stage_model, "ingress_buffered", "current_count")
        ),
        "scan_buffered_entries": parse_int(
            nested_value(stage_model, "scan_buffered", "current_count")
        ),
        "watcher_buffered_entries": parse_int(
            nested_value(stage_model, "watcher_buffered", "current_count")
        ),
        "subtree_hints": 0,
        "subtree_hint_in_flight": 0,
        "subtree_hint_accepted_total": 0,
        "subtree_hint_blocked_total": 0,
        "subtree_hint_suppressed_total": 0,
        "flush_count": parse_int(admission.get("admission_flush_count")),
        "last_promoted_count": parse_int(admission.get("admission_last_promoted_count")),
        "graph_projection_queue": {
            "queued": parse_int(graph_projection_queue.get("queued")),
            "inflight": parse_int(graph_projection_queue.get("inflight")),
            "total": parse_int(
                nested_value(stage_model, "graph_projection_queue_owned", "current_count")
            ),
        },
        "claim_mode": str(utility_scheduler.get("state", "")),
        "service_pressure": str(quiescent_state.get("service_pressure", "")),
        "bridge": "",
        "sql_snapshot": "status_json",
        "last_durably_persisted_count": parse_int(
            admission.get("admission_last_durably_persisted_count")
        ),
        "last_excluded_from_pending_count": parse_int(
            admission.get("admission_last_excluded_from_pending_count")
        ),
        "admission_wip_current": parse_int(admission.get("admission_wip_current")),
        "admission_blocking_authority": str(
            admission.get("blocking_authority", "unknown")
        ),
        "admission_target_band": parse_int(admission.get("target_band")),
        "admission_reorder_point": parse_int(admission.get("reorder_point")),
        "admission_max_wip": parse_int(admission.get("max_wip")),
        "forced_bulk_fill_threshold": parse_int(
            admission.get("forced_bulk_fill_threshold")
        ),
        "bulk_fill_preferred": bool(admission.get("bulk_fill_preferred", False)),
    }

    if process_role == "brain" and peer_runtime.get("available") is True and peer_telemetry:
        telemetry_source = "indexer_peer_status_json"
        telemetry = {
            "buffered_entries": parse_int(peer_telemetry.get("ingress_buffered_entries")),
            "scan_buffered_entries": parse_int(peer_telemetry.get("ingress_scan_entries")),
            "watcher_buffered_entries": parse_int(peer_telemetry.get("ingress_hot_entries")),
            "subtree_hints": parse_int(peer_telemetry.get("ingress_subtree_hints")),
            "subtree_hint_in_flight": parse_int(
                peer_telemetry.get("ingress_subtree_hint_in_flight")
            ),
            "subtree_hint_accepted_total": parse_int(
                peer_telemetry.get("ingress_subtree_hint_accepted_total")
            ),
            "subtree_hint_blocked_total": parse_int(
                peer_telemetry.get("ingress_subtree_hint_blocked_total")
            ),
            "subtree_hint_suppressed_total": parse_int(
                peer_telemetry.get("ingress_subtree_hint_suppressed_total")
            ),
            "flush_count": parse_int(peer_telemetry.get("ingress_flush_count")),
            "last_promoted_count": parse_int(
                peer_telemetry.get("ingress_last_promoted_count")
            ),
            "graph_projection_queue": {
                "queued": parse_int(
                    nested_value(peer_telemetry, "graph_projection_queue", "queued")
                ),
                "inflight": parse_int(
                    nested_value(peer_telemetry, "graph_projection_queue", "inflight")
                ),
                "total": parse_int(
                    nested_value(peer_telemetry, "graph_projection_queue", "total")
                ),
            },
            "claim_mode": str(peer_telemetry.get("claim_mode", "")),
            "service_pressure": str(peer_telemetry.get("service_pressure", "")),
            "bridge": "",
            "sql_snapshot": "indexer_peer_status_json",
            "last_durably_persisted_count": 0,
            "last_excluded_from_pending_count": 0,
            "admission_wip_current": 0,
            "admission_blocking_authority": "indexer_peer_telemetry",
            "admission_target_band": 0,
            "admission_reorder_point": 0,
            "admission_max_wip": 0,
            "forced_bulk_fill_threshold": 0,
            "bulk_fill_preferred": False,
        }

    metrics = {
        "source": telemetry_source,
        "known": parse_int(nested_value(stage_model, "persisted_file", "current_count")),
        "completed": 0,
        "graph_ready": parse_int(nested_value(stage_model, "graph_ready", "current_count")),
        "vector_ready": parse_int(nested_value(stage_model, "vector_ready", "current_count")),
        "vector_ready_graph": 0,
        "indexing": parse_int(admission.get("graph_wip_current")),
        "pending": parse_int(
            nested_value(stage_model, "persisted_file_pending", "current_count")
        ),
        "degraded": 0,
        "skipped": parse_int(
            nested_value(stage_model, "explicitly_excluded_from_vectorization", "current_count")
        ),
        **telemetry,
    }
    metrics["completed"] = max(
        0,
        metrics["known"]
        - metrics["pending"]
        - metrics["indexing"],
    )
    return metrics


def runtime_sql_ready() -> bool:
    try:
        sql_query("SELECT 1 AS ok")
        return True
    except subprocess.CalledProcessError:
        return False
    except json.JSONDecodeError:
        return False


def runtime_mcp_ready() -> bool:
    payload = {
        "jsonrpc": "2.0",
        "id": "qualify-ready",
        "method": "tools/call",
        "params": {"name": "status", "arguments": {"surface": "summary"}},
    }
    try:
        proc = shell(
            [
                "curl",
                "-sS",
                "-X",
                "POST",
                current_mcp_url(),
                "-H",
                "Content-Type: application/json",
                "-d",
                json.dumps(payload),
            ],
            capture=True,
        )
        parsed = parse_json_payload(proc.stdout)
        return isinstance(parsed, dict) and "result" in parsed
    except subprocess.CalledProcessError:
        return False
    except json.JSONDecodeError:
        return False


def detect_axon_pid() -> int | None:
    try:
        proc = shell(["pgrep", "-af", "axon-core|axon-indexer|axon-brain"], capture=True)
    except subprocess.CalledProcessError:
        return None

    for line in proc.stdout.splitlines():
        parts = line.split(maxsplit=1)
        if len(parts) != 2:
            continue
        cmdline = parts[1]
        if (
            "bin/axon-core" in cmdline
            or "axon-indexer" in cmdline
            or "axon-brain" in cmdline
        ):
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
    db_file_bytes = file_size(current_ist_db())
    db_wal_bytes = file_size(current_ist_wal())
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
        numeric = re.search(r"-?\d+", value.replace(",", ""))
        if numeric:
            try:
                return int(numeric.group(0))
            except ValueError:
                pass
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
            extract_html_value(html, r"Vector Ready File</p>\s*<p[^>]*class=\"metric-value\">([^<]+)")
        ),
        "vector_ready_graph": parse_int(
            extract_html_value(html, r"Vector Ready Graph</p>\s*<p[^>]*class=\"metric-value\">([^<]+)")
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
            if runtime_sql_ready() or runtime_mcp_ready():
                return pid
        time.sleep(1)
    raise RuntimeError(f"Axon runtime not ready after {timeout_s}s (last pid={last_pid})")


def mcp_call(tool_name: str, arguments: dict[str, Any] | None = None) -> dict[str, Any]:
    payload = {
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {"name": tool_name, "arguments": arguments or {}},
    }
    proc = shell(
        [
            "curl",
            "-sS",
            "--max-time",
            "5",
            "-X",
            "POST",
            current_mcp_url(),
            "-H",
            "Content-Type: application/json",
            "-d",
            json.dumps(payload),
        ],
        capture=True,
    )
    parsed = parse_json_payload(proc.stdout)
    if isinstance(parsed, dict):
        return parsed
    return {"error": "invalid_mcp_response", "raw": proc.stdout}


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
    enforce_gate: bool
    reuse_runtime: bool
    include_rich_mcp_diagnostics: bool
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
        choices=["full", "graph_only", "read_only", "mcp_only", "brain_shadow", "indexer_shadow"],
        default="full",
        help="Runtime mode passed to the start wrapper. Default: full",
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
    parser.add_argument(
        "--enforce-gate",
        action="store_true",
        help="Fail with exit code 2 if MCP truth_check reports drift_detected.",
    )
    parser.add_argument(
        "--reuse-runtime",
        action="store_true",
        help="Do not stop/start the runtime. Attach to the currently running instance and only sample it.",
    )
    parser.add_argument(
        "--include-rich-mcp-diagnostics",
        action="store_true",
        help="Include expensive MCP diagnostics like truth_check and diagnose_indexing in the final summary.",
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
        enforce_gate=ns.enforce_gate,
        reuse_runtime=ns.reuse_runtime,
        include_rich_mcp_diagnostics=ns.include_rich_mcp_diagnostics,
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
        "shadow_role": shadow_role_for_mode(args.mode),
        "shadow_only": args.mode in {"brain_shadow", "indexer_shadow"},
        "duration_seconds": args.duration,
        "interval_seconds": args.interval,
        "reset_ist": args.reset_ist,
        "keep_running": args.keep_running,
        "reuse_runtime": args.reuse_runtime,
        "paths": {
            "project_root": str(PROJECT_ROOT),
                "graph_root": str(current_graph_root()),
                "ist_db": str(current_ist_db()),
                "ist_wal": str(current_ist_wal()),
                "soll_db": str(current_soll_db()),
                "run_dir": str(run_dir),
        },
        "git": git_context(),
        "commands": {
            "stop": "bash scripts/stop.sh",
            "start": " ".join(start_command_for_mode(args.mode)),
        },
    }
    write_json(lock_path, lock)

    print(f"[qualify] run_dir={run_dir}")
    print(f"[qualify] reset_ist={args.reset_ist} mode={args.mode} duration={args.duration}s interval={args.interval}s")

    stop_code = 0
    start_code = 0
    stop_output = "[qualify] stop skipped because --reuse-runtime was requested\n"
    start_output = "[qualify] start skipped because --reuse-runtime was requested\n"

    if not args.reuse_runtime:
        stop_code, stop_output = run_script("scripts/stop.sh", check=False)
        stop_log_path.write_text(stop_output)
        if stop_code != 0 and runtime_is_up():
            raise RuntimeError(
                f"stop.sh returned {stop_code} and axon-core is still running; see {stop_log_path}"
            )

        if args.reset_ist:
            for path in [current_ist_db(), current_ist_wal()]:
                try:
                    path.unlink()
                except FileNotFoundError:
                    pass

        start_command = start_command_for_mode(args.mode)
        start_env = os.environ.copy()
        start_env["AXON_RUNTIME_SHADOW_ROLE"] = shadow_role_for_mode(args.mode)
        start_env["AXON_SPLIT_SHADOW_ONLY"] = "1" if args.mode in {"brain_shadow", "indexer_shadow"} else "0"
        start_proc = shell(start_command, env=start_env, check=False)
        start_code = start_proc.returncode
        start_output = (start_proc.stdout or "") + (start_proc.stderr or "")
        start_log_path.write_text(start_output)
        if start_code != 0 and not runtime_is_up():
            raise RuntimeError(
                f"start wrapper returned {start_code} and runtime is not up; see {start_log_path}"
            )
    else:
        stop_log_path.write_text(stop_output)
        start_log_path.write_text(start_output)

    pid, runtime_status = wait_for_runtime_contract(args.mode)
    runtime_authority = runtime_status.get("runtime_authority", {})
    runtime_topology = (
        runtime_authority.get("runtime_topology", {})
        if isinstance(runtime_authority, dict)
        else {}
    )
    lock["runtime"] = {
        "pid": pid,
        "started_at": utc_now_iso(),
        "start_exit_code": start_code,
        "stop_exit_code": stop_code,
        "runtime_mode": runtime_status.get("runtime_mode"),
        "shadow_role": shadow_role_for_mode(args.mode),
        "shadow_only": args.mode in {"brain_shadow", "indexer_shadow"},
        "runtime_contract": {
            "topology": runtime_topology,
            "truth_status": runtime_status.get("truth_status"),
        },
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

            sample["runtime_status"] = status_data_for_mode(args.mode)
            if not sample["runtime_status"]:
                sample["runtime_status_error"] = "missing_status_data"

            try:
                runtime_metrics = runtime_metrics_from_status(sample["runtime_status"])
                if runtime_metrics:
                    sample["cockpit"] = runtime_metrics
                else:
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
                f"scan_buffered={cockpit.get('scan_buffered_entries', '')} "
                f"watcher_buffered={cockpit.get('watcher_buffered_entries', '')} "
                f"hints={cockpit.get('subtree_hints', '')} "
                f"hint_in_flight={cockpit.get('subtree_hint_in_flight', '')} "
                f"hint_blocked={cockpit.get('subtree_hint_blocked_total', '')} "
                f"hint_suppressed={cockpit.get('subtree_hint_suppressed_total', '')} "
                f"flushes={cockpit.get('flush_count', '')} "
                f"last_promoted={cockpit.get('last_promoted_count', '')} "
                f"admission_wip={cockpit.get('admission_wip_current', '')} "
                f"admission_block={cockpit.get('admission_blocking_authority', '')} "
                f"bulk_fill={cockpit.get('bulk_fill_preferred', '')} "
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
    max_scan_buffered = max(
        int(sample.get("cockpit", {}).get("scan_buffered_entries", 0)) for sample in samples
    ) if samples else 0
    max_watcher_buffered = max(
        int(sample.get("cockpit", {}).get("watcher_buffered_entries", 0)) for sample in samples
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
    max_flush_count = max(
        int(sample.get("cockpit", {}).get("flush_count", 0)) for sample in samples
    ) if samples else 0
    max_last_promoted_count = max(
        int(sample.get("cockpit", {}).get("last_promoted_count", 0)) for sample in samples
    ) if samples else 0
    max_admission_wip = max(
        int(sample.get("cockpit", {}).get("admission_wip_current", 0)) for sample in samples
    ) if samples else 0
    admission_blocking_authorities = sorted(
        {
            str(sample.get("cockpit", {}).get("admission_blocking_authority", "")).strip()
            for sample in samples
            if str(sample.get("cockpit", {}).get("admission_blocking_authority", "")).strip()
        }
    )
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
        "max_scan_buffered_entries": max_scan_buffered,
        "max_watcher_buffered_entries": max_watcher_buffered,
        "max_subtree_hints": max_hints,
        "max_subtree_hint_in_flight": max_hints_in_flight,
        "max_subtree_hint_blocked_total": max_hint_blocked_total,
        "max_subtree_hint_suppressed_total": max_hint_suppressed_total,
        "max_admission_flush_count": max_flush_count,
        "max_admission_last_promoted_count": max_last_promoted_count,
        "max_admission_wip_current": max_admission_wip,
        "admission_blocking_authorities_seen": admission_blocking_authorities,
        "sql_known_first": sql_known_values[0] if sql_known_values else 0,
        "sql_known_last": sql_known_values[-1] if sql_known_values else 0,
        "cockpit_known_first": cockpit_known_values[0] if cockpit_known_values else 0,
        "cockpit_known_last": cockpit_known_values[-1] if cockpit_known_values else 0,
        "known_divergence_samples": divergence_samples,
        "final_sample": final_sample,
    }
    summary["mcp_truth_check"] = None
    summary["mcp_diagnose_indexing"] = None
    summary["truth_drift_detected"] = None
    truth_drift_detected = False
    if args.include_rich_mcp_diagnostics:
        mcp_truth_check = mcp_call("truth_check", {})
        mcp_indexing_diagnosis = mcp_call("diagnose_indexing", {"project": QUALIFY_PROJECT})
        summary["mcp_truth_check"] = mcp_truth_check
        summary["mcp_diagnose_indexing"] = mcp_indexing_diagnosis

        truth_text = ""
        if isinstance(mcp_truth_check.get("result"), dict):
            content = mcp_truth_check["result"].get("content", [])
            if isinstance(content, list) and content:
                first = content[0]
                if isinstance(first, dict):
                    truth_text = str(first.get("text", ""))
        truth_drift_detected = "drift_detected" in truth_text.lower()
        summary["truth_drift_detected"] = truth_drift_detected
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
        f"Max Scan Buffered Entries: {max_scan_buffered}",
        f"Max Watcher Buffered Entries: {max_watcher_buffered}",
        f"Max Subtree Hints: {max_hints}",
        f"Max Subtree Hint In Flight: {max_hints_in_flight}",
        f"Max Subtree Hint Blocked Total: {max_hint_blocked_total}",
        f"Max Subtree Hint Suppressed Total: {max_hint_suppressed_total}",
        f"Max Admission Flush Count: {max_flush_count}",
        f"Max Admission Last Promoted Count: {max_last_promoted_count}",
        f"Max Admission WIP Current: {max_admission_wip}",
        f"Admission Blocking Authorities Seen: {', '.join(admission_blocking_authorities) if admission_blocking_authorities else 'none'}",
        f"SQL/Cockpit known divergence samples: {divergence_samples}",
        f"Final SQL known: {final_sample.get('sql', {}).get('known', 'ERR')}",
        f"Final SQL completed: {final_sample.get('sql', {}).get('completed', 'ERR')}",
        f"Final SQL graph projection queued: {final_sample.get('sql', {}).get('graph_projection_queue', {}).get('queued', 'ERR')}",
        f"Final SQL graph projection inflight: {final_sample.get('sql', {}).get('graph_projection_queue', {}).get('inflight', 'ERR')}",
        f"Final SQL graph projection total: {final_sample.get('sql', {}).get('graph_projection_queue', {}).get('total', 'ERR')}",
        f"Final cockpit buffered: {final_sample.get('cockpit', {}).get('buffered_entries', 'ERR')}",
        f"Final cockpit scan buffered: {final_sample.get('cockpit', {}).get('scan_buffered_entries', 'ERR')}",
        f"Final cockpit watcher buffered: {final_sample.get('cockpit', {}).get('watcher_buffered_entries', 'ERR')}",
        f"Final cockpit graph projection queued: {final_sample.get('cockpit', {}).get('graph_projection_queue', {}).get('queued', 'ERR')}",
        f"Final cockpit graph projection inflight: {final_sample.get('cockpit', {}).get('graph_projection_queue', {}).get('inflight', 'ERR')}",
        f"Final admission flush count: {final_sample.get('cockpit', {}).get('flush_count', 'ERR')}",
        f"Final admission last promoted count: {final_sample.get('cockpit', {}).get('last_promoted_count', 'ERR')}",
        f"Final admission blocking authority: {final_sample.get('cockpit', {}).get('admission_blocking_authority', 'ERR')}",
        f"MCP truth drift detected: {truth_drift_detected}",
        f"FileIndexed events parsed from runtime log: {file_indexed_stats['parsed_file_indexed_events']}",
        f"Max FileIndexed queue_wait_us: {file_indexed_stats['max_queue_wait_us']}",
        f"Max FileIndexed parse_us: {file_indexed_stats['max_parse_us']}",
        f"Max FileIndexed commit_us: {file_indexed_stats['max_commit_us']}",
    ]
    notes_path.write_text("\n".join(notes) + "\n")

    if args.enforce_gate and truth_drift_detected:
        print("[qualify] release gate failed: truth drift detected")
        return 2

    if not args.keep_running and not args.reuse_runtime:
        stop_after_code, stop_after_output = run_script(
            "scripts/stop.sh", check=False
        )
        stop_log_path.write_text(
            stop_log_path.read_text()
            + "\n\n--- stop-after ---\n"
            + stop_after_output
        )
        if stop_after_code != 0 and runtime_is_up():
            raise RuntimeError(
                f"stop.sh --stop-after returned {stop_after_code} and runtime is still up; see {stop_log_path}"
            )

    print(f"[qualify] summary={summary_path}")
    print(f"[qualify] samples={samples_path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
