#!/usr/bin/env python3
"""REQ-AXO-310 — unit tests for the incremental Memgraph projection builder.

No pytest dependency: run `python3 scripts/test_memgraph_incremental.py`.
Builds real parquet publications (no mocks) and asserts on the generated
cypherl deltas — MERGE for changed/new, DETACH DELETE for removed, skip
unchanged, and never a full wipe in incremental mode.
"""
from __future__ import annotations

import json
import tempfile
from pathlib import Path

import pyarrow as pa
import pyarrow.parquet as pq

import memgraph_build_cypherl as mb


def _write_pub(pub_dir: Path, nodes: list[dict], edges: list[dict]) -> None:
    pub_dir.mkdir(parents=True, exist_ok=True)

    def table(rows: list[dict], cols: list[str]) -> pa.Table:
        return pa.table({c: [str(r.get(c, "")) for r in rows] for c in cols})

    pq.write_table(table(nodes, ["id", "label", "title"]), pub_dir / "nodes.parquet")
    pq.write_table(
        table(edges, ["from_id", "to_id", "relation_type"]), pub_dir / "edges.parquet"
    )
    (pub_dir / "manifest.json").write_text(json.dumps({"publication_id": "pub-test"}))


def test_content_hash_excludes_injected_fields() -> None:
    base = {"id": "A", "title": "x"}
    injected = {"id": "A", "title": "x", "publication_id": "pub-999", "human_only": True}
    assert mb.row_content_hash(base) == mb.row_content_hash(injected), (
        "injected publication fields must not change the content hash"
    )
    assert mb.row_content_hash(base) != mb.row_content_hash({"id": "A", "title": "y"})


def test_incremental_emits_merge_skip_and_delete() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        prior, new = root / "prior", root / "new"
        # prior: A(x), B(y) ; edge A->B
        _write_pub(
            prior,
            nodes=[
                {"id": "A", "label": "Sym", "title": "x"},
                {"id": "B", "label": "Sym", "title": "y"},
            ],
            edges=[{"from_id": "A", "to_id": "B", "relation_type": "calls"}],
        )
        # new: A unchanged, B changed(z), C new ; edge A->B removed, B->C new
        _write_pub(
            new,
            nodes=[
                {"id": "A", "label": "Sym", "title": "x"},
                {"id": "B", "label": "Sym", "title": "z"},
                {"id": "C", "label": "Sym", "title": "w"},
            ],
            edges=[{"from_id": "B", "to_id": "C", "relation_type": "calls"}],
        )
        out = root / "out.cypherl"
        qdir = root / "queries"
        qdir.mkdir()
        summary = mb.build_import(
            new,
            out,
            batch_size=500,
            keep_existing=False,
            query_dir=qdir,
            incremental=True,
            prior_publication_dir=prior,
        )
        text = out.read_text()

        assert "MATCH (n) DETACH DELETE n;" not in text, "no full wipe in incremental"
        assert summary["nodes_skipped"] == 1, summary  # A unchanged
        assert summary["nodes_emitted"] == 2, summary  # B changed + C new
        assert summary["nodes_deleted"] == 0, summary  # A,B,C all present
        assert "MERGE (n:AxonNode {id: row.id})" in text
        assert "CREATE (n:AxonNode" not in text
        assert summary["edges_emitted"] == 1, summary  # B->C
        assert summary["edges_deleted"] == 1, summary  # A->B removed
        assert "WHERE type(r) = row.rel DELETE r;" in text


def test_node_deletion_when_removed() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        prior, new = root / "prior", root / "new"
        _write_pub(
            prior,
            nodes=[
                {"id": "A", "label": "Sym", "title": "x"},
                {"id": "GONE", "label": "Sym", "title": "g"},
            ],
            edges=[],
        )
        _write_pub(new, nodes=[{"id": "A", "label": "Sym", "title": "x"}], edges=[])
        out = root / "out.cypherl"
        qdir = root / "q"
        qdir.mkdir()
        summary = mb.build_import(
            new, out, 500, False, qdir, incremental=True, prior_publication_dir=prior
        )
        text = out.read_text()
        assert summary["nodes_skipped"] == 1, summary  # A unchanged
        assert summary["nodes_deleted"] == 1, summary  # GONE removed
        assert "MATCH (n:AxonNode {id: id}) DETACH DELETE n;" in text
        assert "GONE" in text


if __name__ == "__main__":
    test_content_hash_excludes_injected_fields()
    test_incremental_emits_merge_skip_and_delete()
    test_node_deletion_when_removed()
    print("OK — REQ-AXO-310 incremental builder: 3 tests passed")
