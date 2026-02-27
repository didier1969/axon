"""KuzuDB storage backend for Axon.

Implements the :class:`StorageBackend` protocol using KuzuDB, an embedded
graph database that speaks Cypher.
"""

from __future__ import annotations

import logging
from collections import deque
from pathlib import Path
from typing import Any

import kuzu

from axon.core.graph.graph import KnowledgeGraph
from axon.core.graph.model import GraphNode, GraphRelationship, NodeLabel
from axon.core.storage.base import NodeEmbedding, SearchResult

logger = logging.getLogger(__name__)

# --- Shared constants (used by kuzu_schema, kuzu_search, kuzu_bulk) --------

_NODE_TABLE_NAMES: list[str] = [label.name.title().replace("_", "") for label in NodeLabel]
_LABEL_TO_TABLE: dict[str, str] = {
    label.value: label.name.title().replace("_", "") for label in NodeLabel
}
_LABEL_MAP: dict[str, NodeLabel] = {label.value: label for label in NodeLabel}
_SEARCHABLE_TABLES: list[str] = [
    t for t in _NODE_TABLE_NAMES if t not in ("Folder", "Community", "Process")
]

# --- Schema DDL constants (imported by kuzu_schema) -------------------------

_NODE_PROPERTIES = (
    "id STRING, name STRING, file_path STRING, start_line INT64, "
    "end_line INT64, content STRING, signature STRING, language STRING, "
    "class_name STRING, is_dead BOOL, is_entry_point BOOL, is_exported BOOL, "
    "PRIMARY KEY (id)"
)
_REL_PROPERTIES = (
    "rel_type STRING, confidence DOUBLE, role STRING, step_number INT64, "
    "strength DOUBLE, co_changes INT64, symbols STRING"
)
_EMBEDDING_PROPERTIES = "node_id STRING, vec DOUBLE[], PRIMARY KEY(node_id)"

def _escape(value: str) -> str:
    """Escape a string for safe inclusion in a Cypher literal."""
    return value.replace("\\", "\\\\").replace("'", "\\'")

def _table_for_id(node_id: str) -> str | None:
    """Extract the table name from a node ID by mapping its label prefix."""
    return _LABEL_TO_TABLE.get(node_id.split(":", 1)[0])

def _node_to_row(n: GraphNode) -> list:
    """Convert a GraphNode to a flat row for CSV COPY."""
    return [n.id, n.name, n.file_path, n.start_line, n.end_line, n.content,
            n.signature, n.language, n.class_name, n.is_dead, n.is_entry_point,
            n.is_exported]

def _rel_to_row(r: GraphRelationship) -> list:
    """Convert a GraphRelationship to a flat row for CSV COPY."""
    props = r.properties or {}
    return [r.source, r.target, r.type.value,
            float(props.get("confidence", 1.0)), str(props.get("role", "")),
            int(props.get("step_number", 0)), float(props.get("strength", 0.0)),
            int(props.get("co_changes", 0)), str(props.get("symbols", ""))]


class KuzuBackend:
    """StorageBackend implementation backed by KuzuDB."""

    def __init__(self) -> None:
        self._db: kuzu.Database | None = None
        self._conn: kuzu.Connection | None = None

    def initialize(self, path: Path, *, read_only: bool = False) -> None:
        """Open or create the KuzuDB database at *path* and set up the schema."""
        self._db = kuzu.Database(str(path), read_only=read_only)
        self._conn = kuzu.Connection(self._db)
        if not read_only:
            from axon.core.storage.kuzu_schema import create_schema
            create_schema(self._conn)

    def close(self) -> None:
        """Release the connection and database handles."""
        if self._conn is not None:
            try:
                del self._conn
            except (RuntimeError, OSError):
                pass
            self._conn = None
        if self._db is not None:
            try:
                del self._db
            except (RuntimeError, OSError):
                pass
            self._db = None

    def add_nodes(self, nodes: list[GraphNode]) -> None:
        """Insert nodes into their respective label tables."""
        if not nodes:
            return
        from axon.core.storage.kuzu_bulk import csv_copy
        by_table: dict[str, list[GraphNode]] = {}
        for node in nodes:
            table = _LABEL_TO_TABLE.get(node.label.value)
            if table:
                by_table.setdefault(table, []).append(node)
        try:
            for table, table_nodes in by_table.items():
                csv_copy(self._conn, table, [_node_to_row(n) for n in table_nodes])
        except (RuntimeError, OSError):
            logger.debug("Batch add_nodes via CSV failed, falling back", exc_info=True)
            for node in nodes:
                self._insert_node(node)

    def add_relationships(self, rels: list[GraphRelationship]) -> None:
        """Insert relationships by matching source and target nodes."""
        if not rels:
            return
        from axon.core.storage.kuzu_bulk import csv_copy
        by_pair: dict[tuple[str, str], list[GraphRelationship]] = {}
        for rel in rels:
            src_table = _table_for_id(rel.source)
            dst_table = _table_for_id(rel.target)
            if src_table and dst_table:
                by_pair.setdefault((src_table, dst_table), []).append(rel)
        try:
            for (src_table, dst_table), pair_rels in by_pair.items():
                csv_copy(self._conn, f"CodeRelation_{src_table}_{dst_table}",
                         [_rel_to_row(r) for r in pair_rels])
        except (RuntimeError, OSError):
            logger.debug("Batch add_relationships via CSV failed, falling back", exc_info=True)
            for rel in rels:
                self._insert_relationship(rel)

    def remove_nodes_by_file(self, file_path: str) -> int:
        """Delete all nodes whose ``file_path`` matches across every table."""
        assert self._conn is not None
        for table in _NODE_TABLE_NAMES:
            try:
                self._conn.execute(
                    f"MATCH (n:{table}) WHERE n.file_path = $fp DETACH DELETE n",
                    parameters={"fp": file_path},
                )
            except RuntimeError:
                logger.debug("Failed to remove nodes from table %s", table, exc_info=True)
        return 0

    def get_node(self, node_id: str) -> GraphNode | None:
        """Return a single node by ID, or ``None`` if not found."""
        assert self._conn is not None
        table = _table_for_id(node_id)
        if table is None:
            return None
        try:
            result = self._conn.execute(
                f"MATCH (n:{table}) WHERE n.id = $nid RETURN n.*", parameters={"nid": node_id})
            if result.has_next():
                return self._row_to_node(result.get_next(), node_id)
        except RuntimeError:
            logger.debug("get_node failed for %s", node_id, exc_info=True)
        return None

    def _rel_query(self, node_id: str, rel_type: str, direction: str) -> str | None:
        """Build a relationship Cypher query, returning None if node_id is invalid."""
        table = _table_for_id(node_id)
        if table is None:
            return None
        if direction == "incoming":
            return (f"MATCH (caller)-[r:CodeRelation]->(callee:{table}) "
                    f"WHERE callee.id = $nid AND r.rel_type = '{rel_type}' "
                    f"RETURN caller.*")
        if direction == "incoming_conf":
            return (f"MATCH (caller)-[r:CodeRelation]->(callee:{table}) "
                    f"WHERE callee.id = $nid AND r.rel_type = '{rel_type}' "
                    f"RETURN caller.*, r.confidence")
        if direction == "outgoing_conf":
            return (f"MATCH (caller:{table})-[r:CodeRelation]->(callee) "
                    f"WHERE caller.id = $nid AND r.rel_type = '{rel_type}' "
                    f"RETURN callee.*, r.confidence")
        return (f"MATCH (caller:{table})-[r:CodeRelation]->(callee) "
                f"WHERE caller.id = $nid AND r.rel_type = '{rel_type}' "
                f"RETURN callee.*")

    def get_callers(self, node_id: str) -> list[GraphNode]:
        """Return nodes that CALL the node identified by *node_id*."""
        assert self._conn is not None
        q = self._rel_query(node_id, "calls", "incoming")
        return [] if q is None else self._query_nodes(q, parameters={"nid": node_id})

    def get_callees(self, node_id: str) -> list[GraphNode]:
        """Return nodes called by the node identified by *node_id*."""
        assert self._conn is not None
        q = self._rel_query(node_id, "calls", "outgoing")
        return [] if q is None else self._query_nodes(q, parameters={"nid": node_id})

    def get_type_refs(self, node_id: str) -> list[GraphNode]:
        """Return nodes referenced via USES_TYPE from *node_id*."""
        assert self._conn is not None
        q = self._rel_query(node_id, "uses_type", "outgoing")
        return [] if q is None else self._query_nodes(q, parameters={"nid": node_id})

    def get_callers_with_confidence(self, node_id: str) -> list[tuple[GraphNode, float]]:
        """Return ``(node, confidence)`` for all callers of *node_id*."""
        assert self._conn is not None
        q = self._rel_query(node_id, "calls", "incoming_conf")
        return [] if q is None else self._query_nodes_with_confidence(q, parameters={"nid": node_id})

    def get_callees_with_confidence(self, node_id: str) -> list[tuple[GraphNode, float]]:
        """Return ``(node, confidence)`` for all callees of *node_id*."""
        assert self._conn is not None
        q = self._rel_query(node_id, "calls", "outgoing_conf")
        return [] if q is None else self._query_nodes_with_confidence(q, parameters={"nid": node_id})

    _MAX_BFS_DEPTH = 10

    def traverse(self, start_id: str, depth: int, direction: str = "callers") -> list[GraphNode]:
        """BFS traversal through CALLS edges -- flat result list (no depth info)."""
        return [node for node, _ in self.traverse_with_depth(start_id, depth, direction)]

    def traverse_with_depth(
        self, start_id: str, depth: int, direction: str = "callers"
    ) -> list[tuple[GraphNode, int]]:
        """BFS traversal returning ``(node, hop_depth)`` pairs."""
        assert self._conn is not None
        depth = min(depth, self._MAX_BFS_DEPTH)
        if _table_for_id(start_id) is None:
            return []
        visited: set[str] = set()
        result_list: list[tuple[GraphNode, int]] = []
        queue: deque[tuple[str, int]] = deque([(start_id, 0)])
        while queue:
            current_id, current_depth = queue.popleft()
            if current_id in visited:
                continue
            visited.add(current_id)
            if current_id != start_id:
                node = self.get_node(current_id)
                if node is not None:
                    result_list.append((node, current_depth))
            if current_depth < depth:
                fn = self.get_callers if direction == "callers" else self.get_callees
                for neighbor in fn(current_id):
                    if neighbor.id not in visited:
                        queue.append((neighbor.id, current_depth + 1))
        return result_list

    def get_process_memberships(self, node_ids: list[str]) -> dict[str, str]:
        """Return ``{node_id: process_name}`` for nodes in any Process."""
        assert self._conn is not None
        if not node_ids:
            return {}
        mapping: dict[str, str] = {}
        try:
            result = self._conn.execute(
                "MATCH (n)-[r:CodeRelation]->(p:Process) "
                "WHERE n.id IN $ids AND r.rel_type = 'step_in_process' "
                "RETURN n.id, p.name", parameters={"ids": node_ids})
            while result.has_next():
                row = result.get_next()
                nid, pname = (row[0] if row else ""), (row[1] if len(row) > 1 else "")
                if nid and pname and nid not in mapping:
                    mapping[nid] = pname
        except RuntimeError:
            logger.debug("get_process_memberships failed", exc_info=True)
        return mapping

    def execute_raw(self, query: str) -> list[list[Any]]:
        """Execute a raw Cypher query and return all result rows."""
        assert self._conn is not None
        result = self._conn.execute(query)
        rows: list[list[Any]] = []
        while result.has_next():
            rows.append(result.get_next())
        return rows

    def exact_name_search(self, name: str, limit: int = 5) -> list[SearchResult]:
        """Search for nodes with an exact name match across all searchable tables."""
        assert self._conn is not None
        from axon.core.storage.kuzu_search import exact_name_search
        return exact_name_search(self._conn, name, limit)

    def fts_search(self, query: str, limit: int) -> list[SearchResult]:
        """BM25 full-text search using KuzuDB's native FTS extension."""
        assert self._conn is not None
        from axon.core.storage.kuzu_search import fts_search
        return fts_search(self._conn, query, limit)

    def fuzzy_search(
        self, query: str, limit: int, max_distance: int = 2
    ) -> list[SearchResult]:
        """Fuzzy name search using Levenshtein edit distance."""
        assert self._conn is not None
        from axon.core.storage.kuzu_search import fuzzy_search
        return fuzzy_search(self._conn, query, limit, max_distance)

    def vector_search(self, vector: list[float], limit: int) -> list[SearchResult]:
        """Find the closest nodes to *vector* using cosine similarity."""
        assert self._conn is not None
        from axon.core.storage.kuzu_search import vector_search
        return vector_search(self._conn, vector, limit)

    def store_embeddings(self, embeddings: list[NodeEmbedding]) -> None:
        """Persist embedding vectors into the Embedding node table."""
        assert self._conn is not None
        if not embeddings:
            return

        from axon.core.storage.kuzu_bulk import bulk_store_embeddings_csv
        if bulk_store_embeddings_csv(self._conn, embeddings):
            return

        for emb in embeddings:
            try:
                self._conn.execute(
                    "MERGE (e:Embedding {node_id: $nid}) SET e.vec = $vec",
                    parameters={"nid": emb.node_id, "vec": emb.embedding},
                )
            except RuntimeError:
                logger.debug(
                    "store_embeddings failed for node %s", emb.node_id, exc_info=True
                )

    def get_indexed_files(self) -> dict[str, str]:
        """Return ``{file_path: sha256(content)}`` for all File nodes."""
        assert self._conn is not None
        from axon.core.storage.kuzu_bulk import get_indexed_files
        return get_indexed_files(self._conn)

    def bulk_load(self, graph: KnowledgeGraph) -> None:
        """Replace the entire store with the contents of *graph*."""
        assert self._conn is not None
        from axon.core.storage.kuzu_bulk import bulk_load
        bulk_load(self._conn, graph, self.add_nodes, self.add_relationships)

    def rebuild_fts_indexes(self) -> None:
        """Drop and recreate all FTS indexes."""
        assert self._conn is not None
        from axon.core.storage.kuzu_bulk import rebuild_fts_indexes
        rebuild_fts_indexes(self._conn)

    _INSERT_NODE_CYPHER = (
        "CREATE (:{table} {{id: $id, name: $name, file_path: $file_path, "
        "start_line: $start_line, end_line: $end_line, content: $content, "
        "signature: $signature, language: $language, class_name: $class_name, "
        "is_dead: $is_dead, is_entry_point: $is_entry_point, "
        "is_exported: $is_exported}})"
    )

    def _insert_node(self, node: GraphNode) -> None:
        """INSERT a single node into the appropriate label table."""
        assert self._conn is not None
        table = _LABEL_TO_TABLE.get(node.label.value)
        if table is None:
            logger.warning("Unknown label %s for node %s", node.label, node.id)
            return
        params = {"id": node.id, "name": node.name, "file_path": node.file_path,
                  "start_line": node.start_line, "end_line": node.end_line,
                  "content": node.content, "signature": node.signature,
                  "language": node.language, "class_name": node.class_name,
                  "is_dead": node.is_dead, "is_entry_point": node.is_entry_point,
                  "is_exported": node.is_exported}
        try:
            self._conn.execute(self._INSERT_NODE_CYPHER.format(table=table), parameters=params)
        except RuntimeError:
            logger.debug("Insert node failed for %s", node.id, exc_info=True)

    _INSERT_REL_CYPHER = (
        "MATCH (a:{src}), (b:{dst}) WHERE a.id = $src AND b.id = $tgt "
        "CREATE (a)-[:CodeRelation {{rel_type: $rel_type, confidence: $confidence, "
        "role: $role, step_number: $step_number, strength: $strength, "
        "co_changes: $co_changes, symbols: $symbols}}]->(b)"
    )

    def _insert_relationship(self, rel: GraphRelationship) -> None:
        """MATCH source and target, then CREATE the relationship."""
        assert self._conn is not None
        src_table = _table_for_id(rel.source)
        tgt_table = _table_for_id(rel.target)
        if src_table is None or tgt_table is None:
            logger.warning("Cannot resolve tables for rel %s -> %s", rel.source, rel.target)
            return
        props = rel.properties or {}
        params = {"src": rel.source, "tgt": rel.target, "rel_type": rel.type.value,
                  "confidence": float(props.get("confidence", 1.0)),
                  "role": str(props.get("role", "")),
                  "step_number": int(props.get("step_number", 0)),
                  "strength": float(props.get("strength", 0.0)),
                  "co_changes": int(props.get("co_changes", 0)),
                  "symbols": str(props.get("symbols", ""))}
        try:
            self._conn.execute(
                self._INSERT_REL_CYPHER.format(src=src_table, dst=tgt_table), parameters=params)
        except RuntimeError:
            logger.debug("Insert rel failed: %s -> %s", rel.source, rel.target, exc_info=True)

    def _query_nodes(
        self, query: str, parameters: dict[str, Any] | None = None
    ) -> list[GraphNode]:
        """Execute a query returning ``n.*`` columns and convert to GraphNode list."""
        assert self._conn is not None
        nodes: list[GraphNode] = []
        try:
            result = self._conn.execute(query, parameters=parameters or {})
            while result.has_next():
                row = result.get_next()
                node = self._row_to_node(row)
                if node is not None:
                    nodes.append(node)
        except RuntimeError:
            logger.debug("_query_nodes failed: %s", query, exc_info=True)
        return nodes

    def _query_nodes_with_confidence(
        self, query: str, parameters: dict[str, Any] | None = None
    ) -> list[tuple[GraphNode, float]]:
        """Execute a query returning ``n.*`` columns plus a trailing confidence column."""
        assert self._conn is not None
        pairs: list[tuple[GraphNode, float]] = []
        try:
            result = self._conn.execute(query, parameters=parameters or {})
            while result.has_next():
                row = result.get_next()
                node = self._row_to_node(row[:-1])
                confidence = float(row[-1]) if row[-1] is not None else 1.0
                if node is not None:
                    pairs.append((node, confidence))
        except RuntimeError:
            logger.debug("_query_nodes_with_confidence failed: %s", query, exc_info=True)
        return pairs

    @staticmethod
    def _row_to_node(row: list[Any], node_id: str | None = None) -> GraphNode | None:
        """Convert a result row from ``RETURN n.*`` into a GraphNode."""
        try:
            nid = node_id or row[0]
            label = _LABEL_MAP.get(nid.split(":", 1)[0], NodeLabel.FILE)
            return GraphNode(
                id=row[0], label=label, name=row[1] or "", file_path=row[2] or "",
                start_line=row[3] or 0, end_line=row[4] or 0,
                content=row[5] or "", signature=row[6] or "",
                language=row[7] or "", class_name=row[8] or "",
                is_dead=bool(row[9]), is_entry_point=bool(row[10]),
                is_exported=bool(row[11]))
        except (IndexError, KeyError):
            logger.debug("Failed to convert row to GraphNode: %s", row, exc_info=True)
            return None
