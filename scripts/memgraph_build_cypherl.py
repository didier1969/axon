#!/usr/bin/env python3
# Copyright (c) Didier Stadelmann. All rights reserved.

from __future__ import annotations

import argparse
import hashlib
import json
import math
import re
from pathlib import Path
from typing import Any, Iterable

import pyarrow.parquet as pq


# REQ-AXO-310 — incremental projection (content_hash diff, soll_generate_docs_v3
# model). The injected publication fields change every run, so they are excluded
# from the hash — only the source row content drives the diff.
_INJECTED_FIELDS = ("publication_id", "human_only")


def row_content_hash(row: dict[str, Any]) -> str:
    """Stable short content hash of a source row, ignoring injected publication
    fields. Two rows with the same source content hash equal, so an unchanged
    node/edge is skipped on the next publication."""
    items = sorted((k, v) for k, v in row.items() if k not in _INJECTED_FIELDS)
    digest = hashlib.sha256()
    for key, value in items:
        digest.update(repr(key).encode("utf-8"))
        digest.update(b"=")
        digest.update(repr(value).encode("utf-8"))
        digest.update(b";")
    return digest.hexdigest()[:16]


def edge_diff_key(row: dict[str, Any]) -> str:
    """Identity of an edge for the incremental diff: (from, to, relation). The
    relation is normalised exactly like the emitted Memgraph type (safe_ident +
    upper) so the DELETE match on `type(r)` lines up."""
    relation = safe_ident(str(row.get("relation_type") or "RELATED_TO"), "RELATED_TO").upper()
    return f"{row.get('from_id')}\x1f{row.get('to_id')}\x1f{relation}"


def load_prior_hashes(prior_dir: Path | None) -> tuple[dict[str, str], dict[str, str]]:
    """Read a previous publication's parquet and return `(node id -> hash,
    edge key -> hash)` for the diff. Returns empty maps when no prior
    publication exists — the first incremental run then MERGEs everything."""
    node_hashes: dict[str, str] = {}
    edge_hashes: dict[str, str] = {}
    if prior_dir is None:
        return node_hashes, edge_hashes
    nodes_path = prior_dir / "nodes.parquet"
    edges_path = prior_dir / "edges.parquet"
    if nodes_path.exists():
        for row in iter_rows(nodes_path):
            node_hashes[str(row.get("id"))] = row_content_hash(row)
    if edges_path.exists():
        for row in iter_rows(edges_path):
            edge_hashes[edge_diff_key(row)] = row_content_hash(row)
    return node_hashes, edge_hashes


IDENT_RE = re.compile(r"[^A-Za-z0-9_]")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Build a Memgraph Cypher import file from an Axon graph-shaped Parquet publication."
    )
    parser.add_argument("--publication-dir", required=True, type=Path)
    parser.add_argument("--out", type=Path)
    parser.add_argument("--batch-size", type=int, default=500)
    parser.add_argument("--keep-existing", action="store_true")
    parser.add_argument(
        "--incremental",
        action="store_true",
        help="REQ-AXO-310: emit MERGE/DELETE deltas vs --prior-publication-dir "
        "(content_hash diff, skips unchanged rows) instead of a full wipe+rebuild.",
    )
    parser.add_argument(
        "--prior-publication-dir",
        type=Path,
        default=None,
        help="Previous publication dir to diff against in --incremental mode.",
    )
    parser.add_argument(
        "--query-dir",
        type=Path,
        default=Path(__file__).resolve().parents[1] / "queries" / "memgraph",
        help="Directory containing prepared .cypher queries to install in Memgraph.",
    )
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


QUERY_DESCRIPTIONS = {
    "calls_hotspots_with_file": "CALLS hotspots grouped by file for dependency and blast-radius inspection.",
    "cross_project_links": "Relationships crossing project boundaries.",
    "drift_readiness_signals": "Files that are graph/vector degraded and likely to distort visual analysis.",
    "evidence_inventory": "Evidence volume and linked intent coverage by project and evidence type.",
    "files_by_readiness": "File readiness distribution for graph and vector lanes.",
    "high_degree_nodes": "High-degree graph nodes that dominate topology or may indicate modeling noise.",
    "hot_files": "Largest files and graph/vector readiness by project.",
    "ist_soll_traceability": "SOLL intent to evidence traceability for human inspection.",
    "ist_hot_symbols": "Most connected IST symbols by incoming and outgoing graph degree.",
    "ist_orphan_symbols": "IST symbols with no graph relationships.",
    "orphan_soll_nodes": "SOLL requirements, decisions, and validations that are isolated from the intent graph.",
    "overview": "Node label and relationship inventory for the active projection.",
    "prepared_queries": "List the installed Axon prepared query catalog.",
    "project_code_inventory": "Available project codes with label-scoped node counts.",
    "project_dashboard": "Per-project node inventory across IST and SOLL labels.",
    "project_entry_points": "Files with many symbols and graph connections, useful as human entry points.",
    "project_health_scoreboard": "Project-level graph health scoreboard for files, symbols, intent, evidence, and unresolved endpoints.",
    "project_relationships": "Relationship inventory by project.",
    "requirement_coverage": "Requirement coverage through decisions, validations, and evidence.",
    "soll_decision_impact": "SOLL decisions and nearby affected intent nodes.",
    "soll_decisions": "Current SOLL decision map and outgoing intent links.",
    "soll_requirement_risk": "Requirements missing decisions, validations, or evidence.",
    "top_evidence_references": "Evidence nodes most referenced by SOLL intent.",
    "trace_target_context": "Targeted 1-hop/2-hop context around an id, path, title, or symbol fragment.",
    "traceability_gaps": "SOLL intent nodes without evidence traceability.",
    "unresolved_endpoints": "Referenced graph endpoints that were not materialized as canonical nodes.",
    "why_unresolved_endpoint": "Explain unresolved endpoints through connected source nodes and relation types.",
}


MEMGRAPH_INDEXES = [
    ("AxonNode", "id"),
    ("AxonNode", "project_code"),
    ("AxonNode", "path"),
    ("AxonNode", "title"),
    ("AxonNode", "name"),
    ("AxonNode", "symbol"),
    ("AxonNode", "kind"),
    ("AxonNode", "status"),
    ("File", "project_code"),
    ("File", "path"),
    ("File", "status"),
    ("Symbol", "project_code"),
    ("Symbol", "title"),
    ("Symbol", "kind"),
    ("Requirement", "project_code"),
    ("Decision", "project_code"),
    ("Validation", "project_code"),
    ("Evidence", "project_code"),
    ("UnresolvedEndpoint", "project_code"),
    ("PreparedQuery", "name"),
    ("PreparedQuery", "rank"),
]


def write_indexes(out) -> None:
    for label, property_name in MEMGRAPH_INDEXES:
        out.write(f"CREATE INDEX ON :{label}({property_name});\n")
    out.write("\n")


def write_drop_indexes(out) -> None:
    for label, property_name in MEMGRAPH_INDEXES:
        out.write(f"DROP INDEX ON :{label}({property_name});\n")
    out.write("\n")


def query_parameters(cypher: str) -> str:
    parameters: dict[str, str] = {}
    if "$project_code" in cypher:
        parameters["project_code"] = "optional string; empty or null means all projects"
    if "$limit" in cypher:
        parameters["limit"] = "optional integer; default is query-specific"
    if "$min_degree" in cypher:
        parameters["min_degree"] = "optional integer; default is query-specific"
    if "$target" in cypher:
        parameters["target"] = "required string; id, path, title, name, or symbol fragment"
    return json.dumps(parameters, sort_keys=True)


def direct_all_projects_cypher(cypher: str) -> str:
    direct = cypher
    direct = direct.replace("$project_code", "''")
    direct = direct.replace("$min_degree", "25")
    direct = direct.replace("$limit", "100")
    return direct


def query_usage(cypher: str) -> str:
    if "$project_code" in cypher:
        target_note = " Set target when required." if "$target" in cypher else ""
        target_fallback = (
            " If target is required and parameters are unavailable, replace $target manually."
            if "$target" in cypher
            else ""
        )
        return (
            "Memgraph Lab: set parameter project_code to a project code, or empty/null for all projects. "
            "If Lab parameters are unavailable, run cypher_all_projects directly or replace $project_code manually."
            f"{target_note}{target_fallback}"
        )
    if "$target" in cypher:
        return "Memgraph Lab: set target to an id, path, title, name, or symbol fragment before running."
    return "Memgraph Lab: run cypher directly."


def prepared_query_rows(query_dir: Path, publication_id: str) -> list[dict[str, Any]]:
    if not query_dir.exists():
        return []
    rows = []
    query_files = sorted(query_dir.glob("*.cypher")) + sorted((query_dir / "catalog").glob("*.cypher"))
    for rank, path in enumerate(query_files, start=1):
        name = path.stem
        cypher = path.read_text(encoding="utf-8").strip()
        source_path = path.relative_to(query_dir).as_posix()
        rows.append(
            {
                "id": f"prepared_query:{name}",
                "name": name,
                "title": name.replace("_", " ").title(),
                "description": QUERY_DESCRIPTIONS.get(name, "Prepared Axon Memgraph query."),
                "cypher": cypher,
                "cypher_all_projects": direct_all_projects_cypher(cypher),
                "parameters": query_parameters(cypher),
                "usage": query_usage(cypher),
                "path": source_path,
                "rank": rank,
                "publication_id": publication_id,
                "human_only": True,
                "llm_contract": "use_axon_mcp_not_memgraph",
            }
        )
    return rows


def build_import(
    publication_dir: Path,
    out_path: Path,
    batch_size: int,
    keep_existing: bool,
    query_dir: Path,
    incremental: bool = False,
    prior_publication_dir: Path | None = None,
) -> dict[str, Any]:
    manifest_path = publication_dir / "manifest.json"
    nodes_path = publication_dir / "nodes.parquet"
    edges_path = publication_dir / "edges.parquet"
    manifest = json.loads(manifest_path.read_text())

    # REQ-AXO-310 — incremental mode diffs against the previous publication and
    # emits MERGE deltas (changed/new) + DETACH DELETE (removed), skipping
    # unchanged rows; it never wipes the graph. With no prior publication it
    # degrades to a full MERGE (idempotent, no duplicates).
    prior_node_hashes, prior_edge_hashes = (
        load_prior_hashes(prior_publication_dir) if incremental else ({}, {})
    )

    labels: dict[str, int] = {}
    relations: dict[str, int] = {}
    total_nodes = 0
    total_edges = 0
    nodes_skipped = 0
    edges_skipped = 0
    nodes_deleted = 0
    edges_deleted = 0
    current_node_ids: set[str] = set()
    current_edge_keys: set[str] = set()
    query_rows = prepared_query_rows(query_dir, manifest["publication_id"])

    def node_suffix(label: str) -> str:
        if incremental:
            return f"AS row MERGE (n:AxonNode {{id: row.id}}) SET n:{label}, n += row;"
        return f"AS row CREATE (n:AxonNode:{label}) SET n += row;"

    def edge_suffix(relation: str) -> str:
        verb = "MERGE" if incremental else "CREATE"
        return (
            "AS row MATCH (a:AxonNode {id: row.from_id}), (b:AxonNode {id: row.to_id}) "
            f"{verb} (a)-[r:{relation}]->(b) SET r += row;"
        )

    with out_path.open("w", encoding="utf-8") as out:
        write_drop_indexes(out)
        if not keep_existing and not incremental:
            out.write("MATCH (n) DETACH DELETE n;\n\n")
        write_indexes(out)

        node_batches: dict[str, list[dict[str, Any]]] = {}
        for row in iter_rows(nodes_path):
            label = safe_ident(str(row.get("label") or "AxonNode"), "AxonNode")
            if incremental:
                node_id = str(row.get("id"))
                current_node_ids.add(node_id)
                if prior_node_hashes.get(node_id) == row_content_hash(row):
                    nodes_skipped += 1
                    continue
            row["publication_id"] = manifest["publication_id"]
            row["human_only"] = True
            node_batches.setdefault(label, []).append(row)
            labels[label] = labels.get(label, 0) + 1
            total_nodes += 1
            if len(node_batches[label]) >= batch_size:
                write_batch(out, "UNWIND ", node_batches[label], node_suffix(label))
                node_batches[label] = []

        for label, rows in node_batches.items():
            write_batch(out, "UNWIND ", rows, node_suffix(label))

        if incremental:
            deleted_nodes = [nid for nid in prior_node_hashes if nid not in current_node_ids]
            nodes_deleted = len(deleted_nodes)
            for start in range(0, len(deleted_nodes), batch_size):
                chunk = deleted_nodes[start : start + batch_size]
                out.write(
                    "UNWIND "
                    + json.dumps(chunk)
                    + " AS id MATCH (n:AxonNode {id: id}) DETACH DELETE n;\n\n"
                )

        edge_batches: dict[str, list[dict[str, Any]]] = {}
        for row in iter_rows(edges_path):
            relation = safe_ident(str(row.get("relation_type") or "RELATED_TO"), "RELATED_TO").upper()
            if incremental:
                key = edge_diff_key(row)
                current_edge_keys.add(key)
                if prior_edge_hashes.get(key) == row_content_hash(row):
                    edges_skipped += 1
                    continue
            row["publication_id"] = manifest["publication_id"]
            row["human_only"] = True
            edge_batches.setdefault(relation, []).append(row)
            relations[relation] = relations.get(relation, 0) + 1
            total_edges += 1
            if len(edge_batches[relation]) >= batch_size:
                write_batch(out, "UNWIND ", edge_batches[relation], edge_suffix(relation))
                edge_batches[relation] = []

        for relation, rows in edge_batches.items():
            write_batch(out, "UNWIND ", rows, edge_suffix(relation))

        if incremental:
            deleted_edges = [
                {
                    "from_id": key.split("\x1f")[0],
                    "to_id": key.split("\x1f")[1],
                    "rel": key.split("\x1f")[2],
                }
                for key in prior_edge_hashes
                if key not in current_edge_keys
            ]
            edges_deleted = len(deleted_edges)
            for start in range(0, len(deleted_edges), batch_size):
                chunk = deleted_edges[start : start + batch_size]
                write_batch(
                    out,
                    "UNWIND ",
                    chunk,
                    "AS row MATCH (a:AxonNode {id: row.from_id})-[r]->(b:AxonNode {id: row.to_id}) "
                    "WHERE type(r) = row.rel DELETE r;",
                )

        out.write("MATCH (q:PreparedQuery) DETACH DELETE q;\n\n")
        out.write("MATCH (p:PreparedQueryPack) DETACH DELETE p;\n\n")
        out.write(
            "CREATE (:PreparedQueryPack {id: 'axon_memgraph_query_pack', name: 'Axon Memgraph Query Pack', "
            f"publication_id: {cypher_string(manifest['publication_id'])}, human_only: true, "
            "llm_contract: 'use_axon_mcp_not_memgraph'});\n\n"
        )
        if query_rows:
            write_batch(
                out,
                "UNWIND ",
                query_rows,
                "AS row CREATE (q:PreparedQuery) SET q += row;",
            )
            out.write(
                "MATCH (p:PreparedQueryPack {id: 'axon_memgraph_query_pack'}), (q:PreparedQuery) "
                "CREATE (p)-[:HAS_PREPARED_QUERY]->(q);\n\n"
            )

        out.write("MATCH (n:AxonNode) RETURN count(n) AS imported_nodes;\n")
        out.write("MATCH (:AxonNode)-[r]->(:AxonNode) RETURN count(r) AS imported_edges;\n")
        out.write("MATCH (q:PreparedQuery) RETURN count(q) AS installed_prepared_queries;\n")

    summary = {
        "publication_id": manifest["publication_id"],
        "input_manifest": str(manifest_path),
        "output": str(out_path),
        "incremental": incremental,
        "nodes": total_nodes,
        "edges": total_edges,
        "nodes_emitted": total_nodes,
        "nodes_skipped": nodes_skipped,
        "nodes_deleted": nodes_deleted,
        "edges_emitted": total_edges,
        "edges_skipped": edges_skipped,
        "edges_deleted": edges_deleted,
        "prepared_queries": len(query_rows),
        "query_dir": str(query_dir),
        "labels": labels,
        "relations": relations,
    }
    return summary


def build_query_pack(query_dir: Path, out_path: Path, publication_id: str = "standalone") -> dict[str, Any]:
    query_rows = prepared_query_rows(query_dir, publication_id)
    out_path.parent.mkdir(parents=True, exist_ok=True)
    with out_path.open("w", encoding="utf-8") as out:
        out.write("// Copyright (c) Didier Stadelmann. All rights reserved.\n")
        out.write("// Generated Axon Memgraph human query pack. Do not edit by hand.\n")
        out.write("MATCH (q:PreparedQuery) DETACH DELETE q;\n\n")
        out.write("MATCH (p:PreparedQueryPack) DETACH DELETE p;\n\n")
        out.write(
            "CREATE (:PreparedQueryPack {id: 'axon_memgraph_query_pack', name: 'Axon Memgraph Query Pack', "
            f"publication_id: {cypher_string(publication_id)}, human_only: true, "
            "llm_contract: 'use_axon_mcp_not_memgraph'});\n\n"
        )
        if query_rows:
            write_batch(
                out,
                "UNWIND ",
                query_rows,
                "AS row CREATE (q:PreparedQuery) SET q += row;",
            )
            out.write(
                "MATCH (p:PreparedQueryPack {id: 'axon_memgraph_query_pack'}), (q:PreparedQuery) "
                "CREATE (p)-[:HAS_PREPARED_QUERY]->(q);\n\n"
            )
        out.write("MATCH (q:PreparedQuery) RETURN count(q) AS installed_prepared_queries;\n")

    return {
        "output": str(out_path),
        "prepared_queries": len(query_rows),
        "query_dir": str(query_dir),
        "publication_id": publication_id,
    }


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
    prior_dir = (
        args.prior_publication_dir.resolve()
        if args.incremental and args.prior_publication_dir is not None
        else None
    )
    summary = build_import(
        publication_dir,
        out_path,
        args.batch_size,
        args.keep_existing,
        args.query_dir.resolve(),
        incremental=args.incremental,
        prior_publication_dir=prior_dir,
    )
    print(json.dumps(summary, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
