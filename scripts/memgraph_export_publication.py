#!/usr/bin/env python3
"""REQ-AXO-902052 #6-B — PG → Memgraph publication-dir exporter.

Rebuilds the publication exporter that was purged with the DuckDB plugin
(`src/axon-plugin-duckdb/src/bin_memgraph_publication.rs`). Produces the
`nodes.parquet` + `edges.parquet` + `manifest.json` triple that
`scripts/memgraph_build_cypherl.py` consumes and `scripts/memgraph_validate_publication.py`
validates — the human-only, non-canonical IST+SOLL graph projection
(PIL-AXO-009; LLM clients use Axon MCP, never Memgraph).

Dependency-light by design: reads PG via `psql … COPY … TO STDOUT (FORMAT csv)`
(no Python PG driver needed) and writes Parquet via pyarrow (already a dep of
the downstream scripts). Docker is NOT required to run this — only the final
`memgraph-projection.sh load` step needs it.

Node schema : id, label, project_code, name, title, kind, status
Edge schema : from_id, to_id, relation_type, project_code
"""

from __future__ import annotations

import argparse
import io
import json
import subprocess
import sys
import time
from pathlib import Path

import pyarrow as pa
import pyarrow.csv as pacsv
import pyarrow.parquet as pq

# All columns are exported as text so node ids stay strings (Cypher matches on
# `{id: row.from_id}` — a numeric-looking id must not become an int).
NODE_COLUMNS = ["id", "label", "project_code", "name", "title", "kind", "status"]
EDGE_COLUMNS = ["from_id", "to_id", "relation_type", "project_code"]

# IST + SOLL node union. Every branch projects the 7 NODE_COLUMNS in order.
NODE_QUERY = """
COPY (
    SELECT id, type AS label, project_code, NULL::text AS name, title, NULL::text AS kind, status
      FROM soll.Node
    UNION ALL
    SELECT id, 'Symbol' AS label, project_code, name, NULL::text AS title, kind, NULL::text AS status
      FROM ist.Symbol
    UNION ALL
    SELECT path AS id, 'IndexedFile' AS label, project_code, path AS name,
           NULL::text AS title, NULL::text AS kind, NULL::text AS status
      FROM ist.IndexedFile
) TO STDOUT WITH (FORMAT csv, HEADER true)
"""

# IST + SOLL edge union (4 EDGE_COLUMNS in order).
EDGE_QUERY = """
COPY (
    SELECT source_id AS from_id, target_id AS to_id, relation_type, project_code FROM soll.Edge
    UNION ALL
    SELECT source_id AS from_id, target_id AS to_id, relation_type, project_code FROM ist.edge
) TO STDOUT WITH (FORMAT csv, HEADER true)
"""


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--db-url", required=True, help="PostgreSQL connection URL.")
    parser.add_argument("--out-dir", required=True, type=Path, help="Publication directory to write.")
    parser.add_argument(
        "--publication-id",
        default=None,
        help="Stable id for this publication (default: pub-<unix_ms>).",
    )
    parser.add_argument("--source-commit", default="", help="Optional git sha for provenance.")
    return parser.parse_args()


def copy_to_table(db_url: str, query: str, columns: list[str]) -> pa.Table:
    """Run a `COPY … TO STDOUT (CSV)` and parse it into an all-string table."""
    proc = subprocess.run(
        ["psql", db_url, "--no-psqlrc", "--quiet", "-c", query],
        capture_output=True,
        check=False,
    )
    if proc.returncode != 0:
        sys.stderr.write(proc.stderr.decode("utf-8", "replace"))
        raise SystemExit(f"psql COPY failed (rc={proc.returncode})")
    convert = pacsv.ConvertOptions(column_types={c: pa.string() for c in columns})
    table = pacsv.read_csv(io.BytesIO(proc.stdout), convert_options=convert)
    # Preserve a deterministic column order for the downstream cypher builder.
    return table.select(columns)


def main() -> int:
    args = parse_args()
    out_dir: Path = args.out_dir
    out_dir.mkdir(parents=True, exist_ok=True)
    publication_id = args.publication_id or f"pub-{int(time.time() * 1000)}"

    nodes = copy_to_table(args.db_url, NODE_QUERY, NODE_COLUMNS)
    edges = copy_to_table(args.db_url, EDGE_QUERY, EDGE_COLUMNS)

    pq.write_table(nodes, out_dir / "nodes.parquet")
    pq.write_table(edges, out_dir / "edges.parquet")

    manifest = {
        "publication_id": publication_id,
        "publication_kind": "memgraph_human_ist_soll_projection",
        "human_only": True,
        "llm_contract": "use_axon_mcp_not_memgraph",
        "row_counts": {"nodes": nodes.num_rows, "edges": edges.num_rows},
        "generated_at_ms": int(time.time() * 1000),
        "source_commit": args.source_commit,
    }
    (out_dir / "manifest.json").write_text(json.dumps(manifest, indent=2))

    print(
        json.dumps(
            {
                "status": "ok",
                "publication_id": publication_id,
                "out_dir": str(out_dir),
                "nodes": nodes.num_rows,
                "edges": edges.num_rows,
            },
            indent=2,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
