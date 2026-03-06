"""Bulk loading and CSV operations for KuzuDB.

Contains module-level functions that receive a ``kuzu.Connection`` and
perform bulk data loading, CSV COPY FROM operations, FTS index rebuilding,
and indexed file queries.  These are called by
:class:`~axon.core.storage.kuzu_backend.KuzuBackend` bulk methods.
"""

from __future__ import annotations

import csv
import logging
import tempfile
from pathlib import Path
from typing import Any, Callable, Iterable

import kuzu

from axon.core.graph.model import GraphNode, GraphRelationship
from axon.core.storage.base import NodeEmbedding
from axon.core.storage.kuzu_constants import (
    _LABEL_TO_TABLE,
    _NODE_TABLE_NAMES,
    get_table_for_id as _table_for_id,
    node_to_row as _node_to_row,
    rel_to_row as _rel_to_row,
)

logger = logging.getLogger(__name__)


def csv_copy(conn: kuzu.Connection, table: str, rows: Iterable[list[Any]]) -> None:
    """Write *rows* to a temporary CSV and COPY FROM into *table*.

    Always cleans up the temp file, even on failure.
    """
    csv_path: str | None = None
    try:
        with tempfile.NamedTemporaryFile(
            mode="w", suffix=".csv", delete=False, newline="", encoding="utf-8"
        ) as f:
            writer = csv.writer(f, quoting=csv.QUOTE_NONNUMERIC, quotechar='"')
            writer.writerows(rows)
            csv_path = f.name
        conn.execute(f'COPY {table} FROM "{csv_path}" (HEADER=false, PARALLEL=FALSE)')
    finally:
        if csv_path:
            Path(csv_path).unlink(missing_ok=True)


def bulk_load_nodes_csv(conn: kuzu.Connection, nodes: Iterable[GraphNode]) -> bool:
    """Load all nodes via temporary CSV files + COPY FROM.

    Returns True on success, False if COPY FROM is not available.
    """
    by_table: dict[str, list[GraphNode]] = {}
    for node in nodes:
        table = _LABEL_TO_TABLE.get(node.label.value)
        if table:
            by_table.setdefault(table, []).append(node)

    try:
        for table, table_nodes in by_table.items():
            csv_copy(conn, table, [_node_to_row(node) for node in table_nodes])
        return True
    except (RuntimeError, OSError):
        logger.error("CSV bulk_load_nodes failed, falling back", exc_info=True)
        return False


def bulk_load_rels_csv(conn: kuzu.Connection, rels: Iterable[GraphRelationship]) -> bool:
    """Load all relationships via temporary CSV files + COPY FROM.

    Returns True on success, False if COPY FROM is not available.
    """
    by_pair: dict[tuple[str, str], list[GraphRelationship]] = {}
    for rel in rels:
        src_table = _table_for_id(rel.source)
        dst_table = _table_for_id(rel.target)
        if src_table and dst_table:
            by_pair.setdefault((src_table, dst_table), []).append(rel)

    try:
        for (src_table, dst_table), pair_rels in by_pair.items():
            csv_copy(conn, f"CodeRelation_{src_table}_{dst_table}",
                     [_rel_to_row(rel) for rel in pair_rels])
        return True
    except (RuntimeError, OSError):
        logger.error("CSV bulk_load_rels failed, falling back", exc_info=True)
        return False


def bulk_store_embeddings_csv(
    conn: kuzu.Connection, embeddings: Iterable[NodeEmbedding]
) -> bool:
    """Store embeddings via temporary CSV + COPY FROM.

    Returns True on success, False if COPY FROM is not available.
    """
    try:
        try:
            conn.execute("MATCH (e:Embedding) DETACH DELETE e")
        except RuntimeError:
            pass

        csv_copy(conn, "Embedding", [
            [emb.node_id,
             "[" + ",".join(str(v) for v in emb.embedding) + "]"]
            for emb in embeddings
        ])
        return True
    except (RuntimeError, OSError):
        logger.error("CSV bulk_store_embeddings failed, falling back", exc_info=True)
        return False


def rebuild_fts_indexes(conn: kuzu.Connection) -> None:
    """Drop and recreate all FTS indexes.

    Must be called after any bulk data change so the BM25 indexes
    reflect the current node contents.
    """
    for table in _NODE_TABLE_NAMES:
        idx_name = f"{table.lower()}_fts"
        try:
            conn.execute(f"CALL DROP_FTS_INDEX('{table}', '{idx_name}')")
        except RuntimeError:
            pass
        try:
            conn.execute(
                f"CALL CREATE_FTS_INDEX('{table}', '{idx_name}', "
                f"['name', 'content', 'signature'])"
            )
        except RuntimeError:
            logger.error("FTS index rebuild failed for %s", table, exc_info=True)


def get_indexed_files(conn: kuzu.Connection) -> dict[str, str]:
    """Return ``{file_path: sha256(content)}`` for all File nodes.
    
    Uses KuzuDB's native sha256 function to avoid memory issues on large codebases.
    """
    mapping: dict[str, str] = {}
    try:
        result = conn.execute(
            "MATCH (n:File) RETURN n.file_path, sha256(n.content)"
        )
        while result.has_next():
            row = result.get_next()
            fp = row[0] or ""
            content_hash = row[1] or ""
            mapping[fp] = content_hash
    except RuntimeError:
        logger.error("get_indexed_files failed", exc_info=True)
    return mapping


def bulk_load(
    conn: kuzu.Connection,
    nodes: Iterable[GraphNode],
    rels: Iterable[GraphRelationship],
    add_nodes_fallback: Callable[[list[GraphNode]], None],
    add_relationships_fallback: Callable[[list[GraphRelationship]], None],
) -> None:
    """Replace the entire store with the provided nodes and relationships."""
    for table in _NODE_TABLE_NAMES:
        try:
            conn.execute(f"MATCH (n:{table}) DETACH DELETE n")
        except RuntimeError:
            pass

    # Note: We convert to list here to support the fallback mechanism
    # which expects a list. In a future iteration, the fallbacks themselves
    # should be chunked.
    node_list = list(nodes)
    if not bulk_load_nodes_csv(conn, node_list):
        add_nodes_fallback(node_list)

    rel_list = list(rels)
    if not bulk_load_rels_csv(conn, rel_list):
        add_relationships_fallback(rel_list)

    rebuild_fts_indexes(conn)
