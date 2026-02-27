"""Search operations for KuzuDB.

Contains module-level functions that receive a ``kuzu.Connection`` and
perform various search queries (exact name, FTS, fuzzy, vector).
These are called by :class:`~axon.core.storage.kuzu_backend.KuzuBackend`
search methods.
"""

from __future__ import annotations

import logging
from typing import Any

import kuzu

from axon.core.graph.model import GraphNode, NodeLabel
from axon.core.storage.base import NodeEmbedding, SearchResult
from axon.core.storage.kuzu_backend import (
    _LABEL_MAP,
    _LABEL_TO_TABLE,
    _SEARCHABLE_TABLES,
    _escape,
    _table_for_id,
)

logger = logging.getLogger(__name__)


def _row_to_node(row: list[Any], node_id: str | None = None) -> GraphNode | None:
    """Convert a result row from ``RETURN n.*`` into a GraphNode.

    Column order matches the property definition:
    0=id, 1=name, 2=file_path, 3=start_line, 4=end_line,
    5=content, 6=signature, 7=language, 8=class_name,
    9=is_dead, 10=is_entry_point, 11=is_exported
    """
    try:
        nid = node_id or row[0]
        prefix = nid.split(":", 1)[0]
        label = _LABEL_MAP.get(prefix, NodeLabel.FILE)

        return GraphNode(
            id=row[0],
            label=label,
            name=row[1] or "",
            file_path=row[2] or "",
            start_line=row[3] or 0,
            end_line=row[4] or 0,
            content=row[5] or "",
            signature=row[6] or "",
            language=row[7] or "",
            class_name=row[8] or "",
            is_dead=bool(row[9]),
            is_entry_point=bool(row[10]),
            is_exported=bool(row[11]),
        )
    except (IndexError, KeyError):
        logger.debug("Failed to convert row to GraphNode: %s", row, exc_info=True)
        return None


def exact_name_search(
    conn: kuzu.Connection, name: str, limit: int = 5
) -> list[SearchResult]:
    """Search for nodes with an exact name match across all searchable tables.

    Returns results sorted by label priority (functions/methods first),
    preferring source files over test files.
    """
    candidates: list[SearchResult] = []

    for table in _SEARCHABLE_TABLES:
        cypher = (
            f"MATCH (n:{table}) WHERE n.name = $name "
            f"RETURN n.id, n.name, n.file_path, n.content, n.signature "
            f"LIMIT {limit}"
        )
        try:
            result = conn.execute(cypher, parameters={"name": name})
            while result.has_next():
                row = result.get_next()
                node_id = row[0] or ""
                node_name = row[1] or ""
                file_path = row[2] or ""
                content = row[3] or ""
                signature = row[4] or ""
                label_prefix = node_id.split(":", 1)[0] if node_id else ""
                snippet = content[:200] if content else signature[:200]
                score = 2.0 if "/tests/" not in file_path else 1.0
                candidates.append(
                    SearchResult(
                        node_id=node_id,
                        score=score,
                        node_name=node_name,
                        file_path=file_path,
                        label=label_prefix,
                        snippet=snippet,
                    )
                )
        except RuntimeError:
            logger.debug("exact_name_search failed on table %s", table, exc_info=True)

    candidates.sort(key=lambda r: (-r.score, r.node_id))
    return candidates[:limit]


def fts_search(
    conn: kuzu.Connection, query: str, limit: int
) -> list[SearchResult]:
    """BM25 full-text search using KuzuDB's native FTS extension.

    Searches across all node tables using pre-built FTS indexes on
    ``name``, ``content``, and ``signature`` fields.  Results are
    ranked by BM25 relevance score.

    Returns the top *limit* results sorted by score descending.
    """
    escaped_q = _escape(query)
    candidates: list[SearchResult] = []

    for table in _SEARCHABLE_TABLES:
        idx_name = f"{table.lower()}_fts"
        cypher = (
            f"CALL QUERY_FTS_INDEX('{table}', '{idx_name}', '{escaped_q}') "
            f"RETURN node.id, node.name, node.file_path, node.content, "
            f"node.signature, node.language, score "
            f"ORDER BY score DESC LIMIT {limit}"
        )
        try:
            result = conn.execute(cypher)
            while result.has_next():
                row = result.get_next()
                node_id = row[0] or ""
                name = row[1] or ""
                file_path = row[2] or ""
                content = row[3] or ""
                signature = row[4] or ""
                language = row[5] or ""
                bm25_score = float(row[6]) if row[6] is not None else 0.0

                # Demote test file results — mirrors exact_name_search penalty.
                if "/tests/" in file_path or "/test_" in file_path:
                    bm25_score *= 0.5

                label_prefix = node_id.split(":", 1)[0] if node_id else ""

                # Boost top-level definitions in source files.
                if label_prefix in ("function", "class") and "/tests/" not in file_path:
                    bm25_score *= 1.2

                snippet = content[:200] if content else signature[:200]

                candidates.append(
                    SearchResult(
                        node_id=node_id,
                        score=bm25_score,
                        node_name=name,
                        file_path=file_path,
                        label=label_prefix,
                        snippet=snippet,
                        language=language,
                    )
                )
        except RuntimeError:
            logger.debug("fts_search failed on table %s", table, exc_info=True)

    candidates.sort(key=lambda r: (-r.score, r.node_id))
    return candidates[:limit]


def fuzzy_search(
    conn: kuzu.Connection, query: str, limit: int, max_distance: int = 2
) -> list[SearchResult]:
    """Fuzzy name search using Levenshtein edit distance.

    Scans all node tables for symbols whose name is within
    *max_distance* edits of *query*.  Converts edit distance to a
    score (0 edits = 1.0, *max_distance* edits = 0.3).
    """
    escaped_q = _escape(query.lower())
    candidates: list[SearchResult] = []

    for table in _SEARCHABLE_TABLES:
        cypher = (
            f"MATCH (n:{table}) "
            f"WHERE levenshtein(lower(n.name), '{escaped_q}') <= {max_distance} "
            f"RETURN n.id, n.name, n.file_path, n.content, "
            f"levenshtein(lower(n.name), '{escaped_q}') AS dist "
            f"ORDER BY dist LIMIT {limit}"
        )
        try:
            result = conn.execute(cypher)
            while result.has_next():
                row = result.get_next()
                node_id = row[0] or ""
                name = row[1] or ""
                file_path = row[2] or ""
                content = row[3] or ""
                dist = int(row[4]) if row[4] is not None else max_distance

                score = max(0.3, 1.0 - (dist * 0.3))
                label_prefix = node_id.split(":", 1)[0] if node_id else ""

                candidates.append(
                    SearchResult(
                        node_id=node_id,
                        score=score,
                        node_name=name,
                        file_path=file_path,
                        label=label_prefix,
                        snippet=content[:200] if content else "",
                    )
                )
        except RuntimeError:
            logger.debug("fuzzy_search failed on table %s", table, exc_info=True)

    candidates.sort(key=lambda r: (-r.score, r.node_id))
    return candidates[:limit]


def vector_search(
    conn: kuzu.Connection, vector: list[float], limit: int
) -> list[SearchResult]:
    """Find the closest nodes to *vector* using native ``array_cosine_similarity``.

    Computes cosine similarity directly in KuzuDB's Cypher engine ---
    no Python-side computation or full-table load required.  Joins with
    node tables to fetch metadata in a single query.
    """
    # Vector literals must be inlined — KuzuDB parameterized queries
    # cannot distinguish DOUBLE[] from LIST for array_cosine_similarity.
    vec_literal = "[" + ", ".join(str(v) for v in vector) + "]"

    try:
        result = conn.execute(
            f"MATCH (e:Embedding) "
            f"RETURN e.node_id, "
            f"array_cosine_similarity(e.vec, {vec_literal}) AS sim "
            f"ORDER BY sim DESC LIMIT {limit}"
        )
    except RuntimeError:
        logger.debug("vector_search failed", exc_info=True)
        return []

    emb_rows: list[tuple[str, float]] = []
    while result.has_next():
        row = result.get_next()
        emb_rows.append((row[0] or "", float(row[1]) if row[1] is not None else 0.0))

    if not emb_rows:
        return []

    node_cache: dict[str, GraphNode] = {}
    node_ids = [r[0] for r in emb_rows]
    ids_by_table: dict[str, list[str]] = {}
    for nid in node_ids:
        table = _table_for_id(nid)
        if table:
            ids_by_table.setdefault(table, []).append(nid)

    for table, ids in ids_by_table.items():
        try:
            q = f"MATCH (n:{table}) WHERE n.id IN $ids RETURN n.*"
            res = conn.execute(q, parameters={"ids": ids})
            while res.has_next():
                row = res.get_next()
                node = _row_to_node(row)
                if node:
                    node_cache[node.id] = node
        except RuntimeError:
            logger.debug("Batch node fetch failed for table %s", table, exc_info=True)

    results: list[SearchResult] = []
    for node_id, sim in emb_rows:
        node = node_cache.get(node_id)
        label_prefix = node_id.split(":", 1)[0] if node_id else ""
        results.append(
            SearchResult(
                node_id=node_id,
                score=sim,
                node_name=node.name if node else "",
                file_path=node.file_path if node else "",
                label=label_prefix,
                snippet=(node.content[:200] if node and node.content else ""),
                language=node.language if node else "",
            )
        )
    return results
