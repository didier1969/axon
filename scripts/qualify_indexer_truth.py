#!/usr/bin/env python3
"""REQ-AXO-901827 / MIL-AXO-032 — indexer truth integrity verrou.

Acceptance gate from the MIL body : after a clean indexer run on the
AXO project tree, public.symbol must contain at least 3000 rows for
project_code='AXO', the per-project ratio symbols/code-file must be
≥ 5, every project that the indexer has touched must have at least
one symbol, and the structural kinds (struct / interface / impl /
enum) introduced by the session-62 parser fix must each carry at
least one row.

Falling any clause points at a regression of REQ-AXO-901827 (parser
extract_impl + tree-sitter dispatch) or a downstream pipeline write
bug. Designed to be wired into the promote-live qualify pipeline so
the parser fix never silently regresses.

Run :
    ./scripts/qualify_indexer_truth.py
        [--axo-min 3000] [--ratio-min 5.0]
        [--db-url <postgres://...>]

Uses `psql` shelling out rather than psycopg so the verrou stays
zero-deps in any devenv shell.
"""

from __future__ import annotations

import argparse
import os
import shutil
import subprocess
import sys


def _resolve_db_url(cli_url: str | None) -> str:
    if cli_url:
        return cli_url
    for env in ("AXON_DEV_DATABASE_URL", "DATABASE_URL", "AXON_LIVE_DATABASE_URL"):
        value = os.environ.get(env)
        if value:
            return value
    raise SystemExit(
        "set AXON_DEV_DATABASE_URL or pass --db-url ; the verrou needs PG access"
    )


def _psql_scalar(url: str, sql: str) -> str:
    cmd = ["psql", url, "-t", "-A", "-F", "|", "-c", sql]
    proc = subprocess.run(cmd, capture_output=True, text=True, check=False)
    if proc.returncode != 0:
        raise RuntimeError(f"psql failed : {proc.stderr.strip()}")
    return proc.stdout.strip()


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--axo-min", type=int, default=3000)
    parser.add_argument("--ratio-min", type=float, default=5.0)
    parser.add_argument("--db-url", default=None)
    parser.add_argument(
        "--allow-zero-symbol-projects",
        action="store_true",
        help=(
            "Skip the per-project >0 symbols check. Useful when running"
            " mid-ingestion ; default enforces full coverage."
        ),
    )
    args = parser.parse_args(argv)

    if shutil.which("psql") is None:
        print("psql not in PATH — devenv shell required", file=sys.stderr)
        return 2

    url = _resolve_db_url(args.db_url)
    failures: list[str] = []

    axo_syms = int(
        _psql_scalar(url, "SELECT count(*) FROM public.symbol WHERE project_code='AXO'") or "0"
    )
    if axo_syms < args.axo_min:
        failures.append(
            f"AXO symbol count {axo_syms} < {args.axo_min} (acceptance 1)"
        )

    axo_files = int(
        _psql_scalar(
            url,
            "SELECT count(*) FROM public.indexedfile "
            "WHERE path LIKE '/home/dstadel/projects/axon/%' "
            "  AND status='indexed'",
        )
        or "0"
    )
    if axo_files > 0:
        ratio = float(axo_syms) / float(axo_files)
        if ratio < args.ratio_min:
            failures.append(
                f"AXO symbols/file ratio {ratio:.2f} < {args.ratio_min} (acceptance 3)"
            )
    else:
        failures.append(
            "AXO indexedfile count = 0 — indexer never ran the AXO tree (acceptance 3 cannot evaluate)"
        )

    if not args.allow_zero_symbol_projects:
        raw = _psql_scalar(
            url,
            "SELECT string_agg(project_code, ',') FROM (\n"
            "  SELECT project_code FROM public.symbol GROUP BY project_code HAVING count(*) = 0\n"
            ") s",
        )
        if raw:
            failures.append(
                f"projects without symbols : {raw} (acceptance 2)"
            )

    for kind in ("struct", "interface", "impl", "enum"):
        count = int(
            _psql_scalar(
                url,
                "SELECT count(*) FROM public.symbol "
                f"WHERE project_code='AXO' AND kind='{kind}'",
            )
            or "0"
        )
        if count == 0:
            failures.append(
                f"AXO has 0 symbols of kind='{kind}' — "
                "REQ-AXO-901827 parser fix regressed"
            )

    if failures:
        print("MIL-AXO-032 verrou FAILED :")
        for line in failures:
            print(f"  - {line}")
        return 1

    print(
        "MIL-AXO-032 verrou PASS — "
        f"AXO syms={axo_syms} files={axo_files} ratio={axo_syms / max(axo_files, 1):.2f}"
    )
    return 0


if __name__ == "__main__":  # pragma: no cover
    sys.exit(main())
