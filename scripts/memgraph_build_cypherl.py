#!/usr/bin/env python3
# Copyright (c) Didier Stadelmann. All rights reserved.

from __future__ import annotations

import argparse
import json
import math
import re
from pathlib import Path
from typing import Any, Iterable

import pyarrow.parquet as pq


IDENT_RE = re.compile(r"[^A-Za-z0-9_]")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Build a Memgraph Cypher import file from an Axon graph-shaped Parquet publication."
    )
    parser.add_argument("--publication-dir", required=True, type=Path)
    parser.add_argument("--out", type=Path)
    parser.add_argument("--batch-size", type=int, default=500)
    parser.add_argument("--keep-existing", action="store_true")
    return parser.parse_args()


def safe_ident(raw: str, fallback: str) -> str:
    value = IDENT_RE.sub("_", str(raw or "").strip())
    value = value.strip("_")
    if not value:
        value = fallback
    if value[0].isdigit():
        value = f"{fallback}_{value}"
    return value[:96]


def cypher_string(raw: str) -> str:
    return "'" + raw.replace("\\", "\\\\").replace("'", "\\'").replace("\n", "\\n").replace("\r", "\\r") + "'"


def cypher_value(value: Any) -> str:
    if value is None:
        return "null"
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, int):
        return str(value)
    if isinstance(value, float):
        if math.isnan(value) or math.isinf(value):
            return "null"
        return repr(value)
    return cypher_string(str(value))


def cypher_map(row: dict[str, Any]) -> str:
    parts = []
    for key, value in row.items():
        if value is None:
            continue
        parts.append(f"{safe_ident(key, 'prop')}: {cypher_value(value)}")
    return "{" + ", ".join(parts) + "}"


def iter_rows(path: Path) -> Iterable[dict[str, Any]]:
    table = pq.read_table(path)
    columns = table.column_names
    for batch in table.to_batches(max_chunksize=4096):
        values = {name: batch.column(idx).to_pylist() for idx, name in enumerate(columns)}
        for row_idx in range(batch.num_rows):
            yield {name: values[name][row_idx] for name in columns}


def write_batch(out, statement_prefix: str, rows: list[dict[str, Any]], statement_suffix: str) -> None:
    if not rows:
        return
    out.write(statement_prefix)
    out.write("[\n")
    out.write(",\n".join("  " + cypher_map(row) for row in rows))
    out.write("\n]\n")
    out.write(statement_suffix)
    out.write("\n\n")


def build_import(publication_dir: Path, out_path: Path, batch_size: int, keep_existing: bool) -> dict[str, Any]:
    manifest_path = publication_dir / "manifest.json"
    nodes_path = publication_dir / "nodes.parquet"
    edges_path = publication_dir / "edges.parquet"
    manifest = json.loads(manifest_path.read_text())

    labels: dict[str, int] = {}
    relations: dict[str, int] = {}
    total_nodes = 0
    total_edges = 0

    with out_path.open("w", encoding="utf-8") as out:
        if not keep_existing:
            out.write("MATCH (n) DETACH DELETE n;\n\n")
        out.write("CREATE INDEX ON :AxonNode(id);\n\n")

        node_batches: dict[str, list[dict[str, Any]]] = {}
        for row in iter_rows(nodes_path):
            label = safe_ident(str(row.get("label") or "AxonNode"), "AxonNode")
            row["publication_id"] = manifest["publication_id"]
            row["human_only"] = True
            node_batches.setdefault(label, []).append(row)
            labels[label] = labels.get(label, 0) + 1
            total_nodes += 1
            if len(node_batches[label]) >= batch_size:
                write_batch(
                    out,
                    "UNWIND ",
                    node_batches[label],
                    f"AS row CREATE (n:AxonNode:{label}) SET n += row;",
                )
                node_batches[label] = []

        for label, rows in node_batches.items():
            write_batch(out, "UNWIND ", rows, f"AS row CREATE (n:AxonNode:{label}) SET n += row;")

        edge_batches: dict[str, list[dict[str, Any]]] = {}
        for row in iter_rows(edges_path):
            relation = safe_ident(str(row.get("relation_type") or "RELATED_TO"), "RELATED_TO").upper()
            row["publication_id"] = manifest["publication_id"]
            row["human_only"] = True
            edge_batches.setdefault(relation, []).append(row)
            relations[relation] = relations.get(relation, 0) + 1
            total_edges += 1
            if len(edge_batches[relation]) >= batch_size:
                write_batch(
                    out,
                    "UNWIND ",
                    edge_batches[relation],
                    (
                        "AS row MATCH (a:AxonNode {id: row.from_id}), (b:AxonNode {id: row.to_id}) "
                        f"CREATE (a)-[r:{relation}]->(b) SET r += row;"
                    ),
                )
                edge_batches[relation] = []

        for relation, rows in edge_batches.items():
            write_batch(
                out,
                "UNWIND ",
                rows,
                (
                    "AS row MATCH (a:AxonNode {id: row.from_id}), (b:AxonNode {id: row.to_id}) "
                    f"CREATE (a)-[r:{relation}]->(b) SET r += row;"
                ),
            )

        out.write("MATCH (n) RETURN count(n) AS imported_nodes;\n")
        out.write("MATCH ()-[r]->() RETURN count(r) AS imported_edges;\n")

    summary = {
        "publication_id": manifest["publication_id"],
        "input_manifest": str(manifest_path),
        "output": str(out_path),
        "nodes": total_nodes,
        "edges": total_edges,
        "labels": labels,
        "relations": relations,
    }
    return summary


def main() -> int:
    args = parse_args()
    publication_dir = args.publication_dir.resolve()
    out_path = args.out or publication_dir / "memgraph_import.cypherl"
    if args.batch_size <= 0:
        raise SystemExit("--batch-size must be positive")
    for name in ["manifest.json", "nodes.parquet", "edges.parquet"]:
        path = publication_dir / name
        if not path.exists():
            raise SystemExit(f"missing publication artifact: {path}")
    summary = build_import(publication_dir, out_path, args.batch_size, args.keep_existing)
    print(json.dumps(summary, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
