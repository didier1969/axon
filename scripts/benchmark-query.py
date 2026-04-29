#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import os
import sqlite3
import sys
from pathlib import Path


PROJECT_ROOT = Path(__file__).resolve().parents[1]


def default_benchmark_db() -> Path:
    configured = os.environ.get("AXON_BENCHMARK_DB_PATH", "").strip()
    if configured:
        return Path(configured)
    instance = os.environ.get("AXON_INSTANCE_KIND", "").strip().lower()
    if instance == "dev":
        return PROJECT_ROOT / ".axon-dev" / "run" / "benchmark.sqlite3"
    return PROJECT_ROOT / ".axon" / "run" / "benchmark.sqlite3"


PRESETS = {
    "recent-vector-batches": """
        SELECT run_id,
               wall_ms,
               batch_wait_for_ready_ms,
               prepare_started_at_ms,
               prepare_finished_at_ms,
               ready_enqueued_at_ms,
               gpu_started_at_ms,
               gpu_finished_at_ms,
               chunk_count,
               total_tokens,
               max_item_tokens,
               micro_batch_count,
               effective_vector_workers_admitted,
               vector_worker_admission_reason,
               allowed_gpu_workers,
               ready_queue_depth_at_gpu_start,
               prepare_inflight_at_gpu_start,
               ready_queue_chunks_at_gpu_start,
               prepare_inflight_chunks_at_gpu_start,
               file_count,
               input_bytes,
               fetch_ms,
               embed_ms,
               db_write_ms,
               mark_done_ms,
               success
        FROM vector_batch_run
        ORDER BY finished_at_ms DESC
        LIMIT 50
    """,
    "slow-vector-batches": """
        SELECT run_id,
               wall_ms,
               batch_wait_for_ready_ms,
               prepare_started_at_ms,
               prepare_finished_at_ms,
               ready_enqueued_at_ms,
               gpu_started_at_ms,
               gpu_finished_at_ms,
               chunk_count,
               total_tokens,
               max_item_tokens,
               micro_batch_count,
               effective_vector_workers_admitted,
               vector_worker_admission_reason,
               allowed_gpu_workers,
               ready_queue_depth_at_gpu_start,
               prepare_inflight_at_gpu_start,
               ready_queue_chunks_at_gpu_start,
               prepare_inflight_chunks_at_gpu_start,
               file_count,
               fetch_ms,
               embed_ms,
               db_write_ms,
               mark_done_ms,
               success
        FROM vector_batch_run
        ORDER BY embed_ms DESC, wall_ms DESC
        LIMIT 50
    """,
}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("sql", nargs="?", help="SQL query to execute")
    parser.add_argument("--db", type=Path, default=default_benchmark_db())
    parser.add_argument("--format", choices=("json", "csv", "tsv"), default="json")
    parser.add_argument("--preset", choices=tuple(PRESETS.keys()))
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    sql = PRESETS.get(args.preset) if args.preset else args.sql
    if not sql:
        raise SystemExit("provide SQL or --preset")
    connection = sqlite3.connect(args.db)
    connection.row_factory = sqlite3.Row
    try:
        rows = connection.execute(sql).fetchall()
    except sqlite3.OperationalError as exc:
        raise SystemExit(
            f"query failed against {args.db}: {exc}. "
            "If this is a fresh mirror, run the vector pipeline once with the new code first."
        )
    finally:
        connection.close()
    columns = rows[0].keys() if rows else []

    if args.format == "json":
        print(
            json.dumps(
                {
                    "db_path": str(args.db),
                    "columns": list(columns),
                    "rows": [dict(row) for row in rows],
                },
                indent=2,
            )
        )
        return 0

    delimiter = "," if args.format == "csv" else "\t"
    if columns:
        print(delimiter.join(columns))
    for row in rows:
        values = []
        for column in columns:
            text = "" if row[column] is None else str(row[column])
            if delimiter == "," and any(ch in text for ch in [",", "\"", "\n"]):
                text = '"' + text.replace('"', '""') + '"'
            elif delimiter == "\t":
                text = text.replace("\t", " ").replace("\n", "\\n")
            values.append(text)
        print(delimiter.join(values))
    return 0


if __name__ == "__main__":
    sys.exit(main())
