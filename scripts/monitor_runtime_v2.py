#!/usr/bin/env python3
import argparse
import csv
import json
import os
import subprocess
import sys
import time
import urllib.error
import urllib.request
from datetime import datetime
from pathlib import Path


PROJECT_ROOT = Path("/home/dstadel/projects/axon")
DEFAULT_SQL_URL = "http://127.0.0.1:44129/sql"
DEFAULT_DB_PATH = PROJECT_ROOT / ".axon" / "graph_v2" / "ist.db"


def post_json(url: str, payload: dict) -> object:
    request = urllib.request.Request(
        url,
        data=json.dumps(payload).encode("utf-8"),
        headers={"content-type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(request, timeout=5.0) as response:
        return json.loads(response.read().decode("utf-8"))


def sql_query(sql_url: str, query: str) -> object:
    return post_json(sql_url, {"query": query})


def detect_axon_pid() -> int | None:
    try:
        output = subprocess.check_output(
            ["pgrep", "-f", "bin/axon-core"], text=True, stderr=subprocess.DEVNULL
        )
    except subprocess.CalledProcessError:
        return None

    for line in output.splitlines():
        line = line.strip()
        if line.isdigit():
            return int(line)
    return None


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


def file_size(path: Path) -> int:
    try:
        return path.stat().st_size
    except FileNotFoundError:
        return 0


def db_sizes(db_path: Path) -> dict[str, int]:
    wal_path = Path(f"{db_path}.wal")
    db_file_bytes = file_size(db_path)
    db_wal_bytes = file_size(wal_path)
    return {
        "db_file_bytes": db_file_bytes,
        "db_wal_bytes": db_wal_bytes,
        "db_total_bytes": db_file_bytes + db_wal_bytes,
    }


def parse_int(value: object) -> int:
    if isinstance(value, bool):
        return int(value)
    if isinstance(value, int):
        return value
    if isinstance(value, float):
        return round(value)
    if isinstance(value, str):
        try:
            return int(value)
        except ValueError:
            try:
                return round(float(value))
            except ValueError:
                return 0
    return 0


def fetch_status_counts(sql_url: str) -> dict[str, int]:
    rows = sql_query(
        sql_url,
        "SELECT status, count(*) AS count FROM File GROUP BY status ORDER BY status",
    )
    stats: dict[str, int] = {}
    if isinstance(rows, list):
        for row in rows:
            if isinstance(row, list) and len(row) >= 2:
                status = str(row[0])
                stats[status] = parse_int(row[1])
    return stats


def fetch_backlog_reasons(sql_url: str) -> list[tuple[str, int]]:
    rows = sql_query(
        sql_url,
        "SELECT COALESCE(status_reason, 'unknown'), count(*) "
        "FROM File "
        "WHERE status IN ('pending', 'indexing') "
        "GROUP BY 1 "
        "ORDER BY count(*) DESC, 1 ASC "
        "LIMIT 5",
    )
    reasons: list[tuple[str, int]] = []
    if isinstance(rows, list):
        for row in rows:
            if isinstance(row, list) and len(row) >= 2:
                reasons.append((str(row[0]), parse_int(row[1])))
    return reasons


def format_mb(value: int) -> str:
    return f"{value / (1024 * 1024):.0f}MB"


def monitor(sql_url: str, db_path: Path, duration_s: int, interval_s: int, csv_path: Path) -> int:
    pid = detect_axon_pid()
    if pid is None:
        print("❌ Impossible de trouver le PID de bin/axon-core. Axon doit être démarré.")
        return 1

    csv_path.parent.mkdir(parents=True, exist_ok=True)
    fieldnames = [
        "timestamp",
        "elapsed_s",
        "pid",
        "known_files",
        "completed_files",
        "pending_files",
        "indexing_files",
        "degraded_files",
        "oversized_files",
        "skipped_files",
        "rss_bytes",
        "rss_anon_bytes",
        "rss_file_bytes",
        "rss_shmem_bytes",
        "db_file_bytes",
        "db_wal_bytes",
        "db_total_bytes",
        "top_backlog_reasons",
    ]

    print(f"[{datetime.now().strftime('%H:%M:%S')}] Monitoring Axon V2 démarré")
    print(f"PID runtime: {pid}")
    print(f"SQL: {sql_url}")
    print(f"DB: {db_path}")
    print(f"CSV: {csv_path}")
    print("-" * 100)

    start = time.time()
    sample_index = 0

    with csv_path.open("w", newline="", encoding="utf-8") as handle:
        writer = csv.DictWriter(handle, fieldnames=fieldnames)
        writer.writeheader()

        while True:
            now = time.time()
            elapsed_s = int(now - start)
            if elapsed_s > duration_s:
                break

            stats = fetch_status_counts(sql_url)
            backlog_reasons = fetch_backlog_reasons(sql_url)
            memory = parse_proc_status(pid)
            storage = db_sizes(db_path)

            pending_files = stats.get("pending", 0)
            indexing_files = stats.get("indexing", 0)
            degraded_files = stats.get("indexed_degraded", 0)
            oversized_files = stats.get("oversized_for_current_budget", 0)
            skipped_files = stats.get("skipped", 0)
            known_files = sum(stats.values())
            completed_files = max(0, known_files - pending_files - indexing_files)

            top_reasons = ", ".join(
                f"{reason}:{count}" for reason, count in backlog_reasons
            ) or "none"

            row = {
                "timestamp": datetime.now().isoformat(timespec="seconds"),
                "elapsed_s": elapsed_s,
                "pid": pid,
                "known_files": known_files,
                "completed_files": completed_files,
                "pending_files": pending_files,
                "indexing_files": indexing_files,
                "degraded_files": degraded_files,
                "oversized_files": oversized_files,
                "skipped_files": skipped_files,
                "rss_bytes": memory["rss_bytes"],
                "rss_anon_bytes": memory["rss_anon_bytes"],
                "rss_file_bytes": memory["rss_file_bytes"],
                "rss_shmem_bytes": memory["rss_shmem_bytes"],
                "db_file_bytes": storage["db_file_bytes"],
                "db_wal_bytes": storage["db_wal_bytes"],
                "db_total_bytes": storage["db_total_bytes"],
                "top_backlog_reasons": top_reasons,
            }
            writer.writerow(row)
            handle.flush()

            sample_index += 1
            print(
                f"[{datetime.now().strftime('%H:%M:%S')}] "
                f"T+{elapsed_s:>4}s "
                f"known={known_files:<6} done={completed_files:<6} "
                f"pending={pending_files:<6} indexing={indexing_files:<6} "
                f"rss={format_mb(memory['rss_bytes']):>6} "
                f"anon={format_mb(memory['rss_anon_bytes']):>6} "
                f"file={format_mb(memory['rss_file_bytes']):>6} "
                f"db={format_mb(storage['db_total_bytes']):>6} "
                f"reasons={top_reasons}"
            )
            sys.stdout.flush()
            time.sleep(interval_s)

    print("-" * 100)
    print(
        f"[{datetime.now().strftime('%H:%M:%S')}] Monitoring terminé. "
        f"{sample_index} échantillons écrits dans {csv_path}"
    )
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description="Monitor Axon V2 runtime/backlog/memory.")
    parser.add_argument("--duration", type=int, default=180, help="Durée totale en secondes.")
    parser.add_argument("--interval", type=int, default=15, help="Intervalle d'échantillonnage en secondes.")
    parser.add_argument("--sql-url", default=DEFAULT_SQL_URL, help="URL du SQL gateway.")
    parser.add_argument(
        "--db-path",
        default=str(DEFAULT_DB_PATH),
        help="Chemin du fichier DuckDB principal.",
    )
    parser.add_argument(
        "--csv",
        default=str(PROJECT_ROOT / ".axon" / "observability" / "runtime_monitor.csv"),
        help="Chemin du CSV de sortie.",
    )
    args = parser.parse_args()

    try:
        return monitor(
            sql_url=args.sql_url,
            db_path=Path(args.db_path),
            duration_s=args.duration,
            interval_s=args.interval,
            csv_path=Path(args.csv),
        )
    except urllib.error.URLError as exc:
        print(f"❌ SQL gateway indisponible: {exc}")
        return 1
    except KeyboardInterrupt:
        print("\n⏹️ Monitoring interrompu manuellement.")
        return 130


if __name__ == "__main__":
    raise SystemExit(main())
