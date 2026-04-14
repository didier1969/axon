#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import subprocess
import sys
import time
from pathlib import Path
from typing import Any


DEFAULT_SQL_URL = "http://127.0.0.1:44129/sql"
NOISE_PREDICATE = (
    "path LIKE '%/.worktrees/%' OR "
    "path LIKE '%/.devbox/%' OR "
    "path LIKE '%/priv/static/%' OR "
    "path LIKE '%/_build/%' OR "
    "path LIKE '%/dist/%' OR "
    "path LIKE '%/node_modules/%' OR "
    "path LIKE '%.min.js' OR "
    "path LIKE '%.min.css'"
)


def q(sql_url: str, query: str) -> list[list[Any]]:
    payload = json.dumps({"query": query})
    proc = subprocess.run(
        [
            "curl",
            "-sS",
            "-X",
            "POST",
            sql_url,
            "-H",
            "Content-Type: application/json",
            "-d",
            payload,
        ],
        text=True,
        capture_output=True,
        check=True,
    )
    data = json.loads(proc.stdout or "[]")
    if isinstance(data, list):
        return data
    return []


def scalar(sql_url: str, query: str) -> int:
    rows = q(sql_url, query)
    if not rows or not rows[0]:
        return 0
    v = rows[0][0]
    try:
        return int(v)
    except Exception:
        try:
            return int(float(v))
        except Exception:
            return 0


def esc(s: str) -> str:
    return s.replace("'", "''")


def metrics(sql_url: str, project: str) -> dict[str, int]:
    p = esc(project)
    return {
        "known": scalar(sql_url, f"SELECT count(*) FROM File WHERE project_code = '{p}'"),
        "completed": scalar(
            sql_url,
            f"SELECT count(*) FROM File WHERE project_code = '{p}' "
            "AND status IN ('indexed','indexed_degraded','skipped','deleted')",
        ),
        "pending": scalar(
            sql_url, f"SELECT count(*) FROM File WHERE project_code = '{p}' AND status = 'pending'"
        ),
        "indexing": scalar(
            sql_url, f"SELECT count(*) FROM File WHERE project_code = '{p}' AND status = 'indexing'"
        ),
        "symbols": scalar(sql_url, f"SELECT count(*) FROM Symbol WHERE project_code = '{p}'"),
        "noise_files": scalar(
            sql_url,
            f"SELECT count(*) FROM File WHERE project_code = '{p}' AND ({NOISE_PREDICATE}) "
            "AND status <> 'deleted'",
        ),
        "noise_symbols": scalar(
            sql_url,
            f"SELECT count(DISTINCT c.target_id) "
            "FROM CONTAINS c JOIN File f ON f.path = c.source_id "
            f"WHERE f.project_code = '{p}' AND ({NOISE_PREDICATE}) AND f.status <> 'deleted'",
        ),
    }


def print_metrics(title: str, m: dict[str, int]) -> None:
    print(title)
    print(
        f"  known={m['known']} completed={m['completed']} pending={m['pending']} indexing={m['indexing']} "
        f"symbols={m['symbols']} noise_files={m['noise_files']} noise_symbols={m['noise_symbols']}"
    )


def execute(sql_url: str, query: str) -> None:
    q(sql_url, query)


def is_git_ignored(path: str, project_root: str) -> bool:
    try:
        candidate = Path(path)
        root = Path(project_root)
        if candidate.is_absolute():
            candidate = candidate.resolve().relative_to(root.resolve())
        path_arg = str(candidate)
    except Exception:
        path_arg = path

    proc = subprocess.run(
        ["git", "-C", project_root, "check-ignore", "-q", "--", path_arg],
        text=True,
        capture_output=True,
    )
    return proc.returncode == 0


def clean_rebuild(sql_url: str, project: str, respect_ignore: bool, project_root: str) -> None:
    p = esc(project)
    print(f"Running clean rebuild for project '{project}'...")
    # 1) Remove all derived graph/vector artifacts tied to this project.
    execute(
        sql_url,
        f"DELETE FROM CALLS WHERE source_id IN (SELECT id FROM Symbol WHERE project_code = '{p}') "
        f"OR target_id IN (SELECT id FROM Symbol WHERE project_code = '{p}');",
    )
    execute(
        sql_url,
        f"DELETE FROM CALLS_NIF WHERE source_id IN (SELECT id FROM Symbol WHERE project_code = '{p}') "
        f"OR target_id IN (SELECT id FROM Symbol WHERE project_code = '{p}');",
    )
    execute(
        sql_url,
        f"DELETE FROM ChunkEmbedding WHERE chunk_id IN (SELECT id FROM Chunk WHERE project_code = '{p}');",
    )
    execute(sql_url, f"DELETE FROM Chunk WHERE project_code = '{p}';")
    execute(
        sql_url,
        f"DELETE FROM CONTAINS WHERE source_id IN (SELECT path FROM File WHERE project_code = '{p}') "
        f"OR target_id IN (SELECT id FROM Symbol WHERE project_code = '{p}');",
    )
    execute(sql_url, f"DELETE FROM Symbol WHERE project_code = '{p}';")
    execute(
        sql_url,
        f"DELETE FROM FileVectorizationQueue WHERE file_path IN (SELECT path FROM File WHERE project_code = '{p}');",
    )
    # 2) Base cleanup of known generated/minified assets.
    execute(
        sql_url,
        f"""
UPDATE File
SET status = 'deleted',
    worker_id = NULL,
    needs_reindex = FALSE,
    file_stage = 'deleted',
    graph_ready = FALSE,
    vector_ready = FALSE,
    status_reason = 'ignored_generated_asset'
WHERE project_code = '{p}' AND ({NOISE_PREDICATE});
""".strip(),
    )
    # 3) Optionally enforce current gitignore as source of truth.
    if respect_ignore:
        rows = q(
            sql_url,
            f"SELECT path FROM File WHERE project_code = '{p}' AND status <> 'deleted' ORDER BY path;",
        )
        ignored_paths: list[str] = []
        for row in rows:
            if not row:
                continue
            path = str(row[0])
            if is_git_ignored(path, project_root):
                ignored_paths.append(path)
        if ignored_paths:
            for i in range(0, len(ignored_paths), 300):
                chunk = ignored_paths[i : i + 300]
                selector = ",".join(f"'{esc(x)}'" for x in chunk)
                execute(
                    sql_url,
                    f"""
UPDATE File
SET status = 'deleted',
    worker_id = NULL,
    needs_reindex = FALSE,
    file_stage = 'deleted',
    graph_ready = FALSE,
    vector_ready = FALSE,
    status_reason = 'ignored_by_gitignore_rebuild'
WHERE path IN ({selector});
""".strip(),
                )
            print(f"Applied gitignore exclusion to {len(ignored_paths)} path(s).")


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description="Clean rebuild of a single project with noise evaluation")
    ap.add_argument("--project", default="BookingSystem", help="Project code")
    ap.add_argument("--sql-url", default=DEFAULT_SQL_URL, help="SQL gateway URL")
    ap.add_argument("--wait-seconds", type=int, default=120, help="Wait window for indexing progress")
    args = ap.parse_args(argv)

    project = args.project
    sql_url = args.sql_url
    p = esc(project)
    project_root = str(Path("/home/dstadel/projects") / project)

    before = metrics(sql_url, project)
    print_metrics("Before:", before)

    clean_rebuild(sql_url, project, True, project_root)

    print(f"Requeueing project '{project}' for targeted reindex...")
    execute(
        sql_url,
        f"""
UPDATE File
SET status = 'pending',
    worker_id = NULL,
    priority = 900,
    status_reason = 'manual_project_clean_reindex',
    file_stage = 'promoted',
    graph_ready = FALSE,
    vector_ready = FALSE
WHERE project_code = '{p}' AND status <> 'deleted';
""".strip(),
    )

    deadline = time.time() + max(0, args.wait_seconds)
    while time.time() < deadline:
        m = metrics(sql_url, project)
        print_metrics("Progress:", m)
        if m["pending"] == 0 and m["indexing"] == 0:
            break
        time.sleep(5)

    after = metrics(sql_url, project)
    print_metrics("After:", after)

    delta_noise_files = after["noise_files"] - before["noise_files"]
    delta_noise_symbols = after["noise_symbols"] - before["noise_symbols"]
    print("Summary:")
    print(f"  noise_files_delta={delta_noise_files}")
    print(f"  noise_symbols_delta={delta_noise_symbols}")
    print(f"  completed={after['completed']}/{after['known']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
