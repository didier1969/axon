#!/usr/bin/env python3
# Copyright (c) Didier Stadelmann. All rights reserved.

from __future__ import annotations

import argparse
import json
from pathlib import Path

import pyarrow.parquet as pq


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Validate an Axon Memgraph Parquet publication.")
    parser.add_argument("--publication-dir", required=True, type=Path)
    parser.add_argument("--require-import-file", action="store_true")
    return parser.parse_args()


def count_rows(path: Path) -> int:
    return pq.ParquetFile(path).metadata.num_rows


def main() -> int:
    args = parse_args()
    publication_dir = args.publication_dir.resolve()
    manifest_path = publication_dir / "manifest.json"
    nodes_path = publication_dir / "nodes.parquet"
    edges_path = publication_dir / "edges.parquet"
    import_path = publication_dir / "memgraph_import.cypherl"

    errors: list[str] = []
    for path in [manifest_path, nodes_path, edges_path]:
        if not path.exists():
            errors.append(f"missing artifact: {path}")
    if args.require_import_file and not import_path.exists():
        errors.append(f"missing import file: {import_path}")
    if errors:
        print(json.dumps({"status": "failed", "errors": errors}, indent=2))
        return 1

    manifest = json.loads(manifest_path.read_text())
    node_count = count_rows(nodes_path)
    edge_count = count_rows(edges_path)
    expected_nodes = int(manifest.get("row_counts", {}).get("nodes", -1))
    expected_edges = int(manifest.get("row_counts", {}).get("edges", -1))
    if node_count != expected_nodes:
        errors.append(f"node count mismatch: manifest={expected_nodes} parquet={node_count}")
    if edge_count != expected_edges:
        errors.append(f"edge count mismatch: manifest={expected_edges} parquet={edge_count}")
    if manifest.get("llm_contract") != "use_axon_mcp_not_memgraph":
        errors.append("llm_contract must be use_axon_mcp_not_memgraph")
    if manifest.get("human_only") is not True:
        errors.append("human_only must be true")
    if manifest.get("publication_kind") != "memgraph_human_ist_soll_projection":
        errors.append("publication_kind mismatch")

    summary = {
        "status": "failed" if errors else "ok",
        "publication_id": manifest.get("publication_id"),
        "publication_dir": str(publication_dir),
        "nodes": node_count,
        "edges": edge_count,
        "has_import_file": import_path.exists(),
        "import_file_size_bytes": import_path.stat().st_size if import_path.exists() else 0,
        "errors": errors,
    }
    print(json.dumps(summary, indent=2, sort_keys=True))
    return 1 if errors else 0


if __name__ == "__main__":
    raise SystemExit(main())
