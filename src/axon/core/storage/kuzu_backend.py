"""KuzuDB storage backend for Axon.

Implements the :class:`StorageBackend` protocol using KuzuDB, an embedded
graph database that speaks Cypher.
"""

from __future__ import annotations

import logging
from collections import deque
from pathlib import Path
from typing import Any, Iterable

import kuzu

from axon.core.graph.graph import KnowledgeGraph
from axon.core.graph.model import GraphNode, GraphRelationship, NodeLabel
from axon.core.storage.base import NodeEmbedding, SearchResult

logger = logging.getLogger(__name__)

from axon.core.storage.kuzu_constants import (
    _EMBEDDING_PROPERTIES,
    _LABEL_MAP,
    _LABEL_TO_TABLE,
    _NODE_PROPERTIES,
    _NODE_TABLE_NAMES,
    _REL_PROPERTIES,
    _SEARCHABLE_TABLES,
    get_table_for_id as _table_for_id,
    node_to_row as _node_to_row,
    rel_to_row as _rel_to_row,
)

class KuzuBackend:
    """StorageBackend implementation backed by KuzuDB."""

    def __init__(self) -> None:
        self._db: kuzu.Database | None = None
        self._conn: kuzu.Connection | None = None
        self.db_path: Path | None = None

    def initialize(self, path: Path, *, read_only: bool = False) -> None:
        """Open or create the KuzuDB database at *path* and set up the schema."""
        self.db_path = path
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
            logger.error("Batch add_nodes via CSV failed, falling back", exc_info=True)
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
            logger.error("Batch add_relationships via CSV failed, falling back", exc_info=True)
            for rel in rels:
                self._insert_relationship(rel)

    def remove_nodes_by_file(self, file_path: str) -> int:
        """Delete all nodes whose ``file_path`` matches across every table.

        Returns the total number of deleted nodes.
        """
        assert self._conn is not None
        try:
            result = self._conn.execute(
                "MATCH (n) WHERE n.file_path = $fp DETACH DELETE n RETURN count(n)",
                parameters={"fp": file_path},
            )
            if result.has_next():
                return int(result.get_next()[0] or 0)
        except RuntimeError:
            logger.debug("Failed to remove nodes for file %s", file_path, exc_info=True)
        return 0

    def get_node(self, node_id: str) -> GraphNode | None:
        """Return a single node by ID, or ``None`` if not found."""
        assert self._conn is not None
        table = _table_for_id(node_id)
        if table is None:
            return None
        try:
            result = self._conn.execute(
                f"MATCH (n:{table}) WHERE n.id = $nid RETURN n", parameters={"nid": node_id})
            if result.has_next():
                return self._row_to_node(result.get_next()[0], node_id)
        except RuntimeError:
            logger.error("get_node failed for %s", node_id, exc_info=True)
        return None

    def _rel_query(self, node_id: str, direction: str) -> str | None:
        """Build a relationship Cypher query, returning None if node_id is invalid."""
        table = _table_for_id(node_id)
        if table is None:
            return None
        if direction == "incoming":
            return (f"MATCH (caller)-[r:CodeRelation]->(callee:{table}) "
                    f"WHERE callee.id = $nid AND r.rel_type = $rel_type "
                    f"RETURN caller")
        if direction == "incoming_conf":
            return (f"MATCH (caller)-[r:CodeRelation]->(callee:{table}) "
                    f"WHERE callee.id = $nid AND r.rel_type = $rel_type "
                    f"RETURN caller, r.confidence")
        if direction == "outgoing_conf":
            return (f"MATCH (caller:{table})-[r:CodeRelation]->(callee) "
                    f"WHERE caller.id = $nid AND r.rel_type = $rel_type "
                    f"RETURN callee, r.confidence")
        return (f"MATCH (caller:{table})-[r:CodeRelation]->(callee) "
                f"WHERE caller.id = $nid AND r.rel_type = $rel_type "
                f"RETURN callee")

    def get_callers(self, node_id: str) -> list[GraphNode]:
        """Return nodes that CALL the node identified by *node_id*."""
        assert self._conn is not None
        q = self._rel_query(node_id, "incoming")
        return [] if q is None else self._query_nodes(q, parameters={"nid": node_id, "rel_type": "calls"})

    def get_callees(self, node_id: str) -> list[GraphNode]:
        """Return nodes called by the node identified by *node_id*."""
        assert self._conn is not None
        q = self._rel_query(node_id, "outgoing")
        return [] if q is None else self._query_nodes(q, parameters={"nid": node_id, "rel_type": "calls"})

    def get_type_refs(self, node_id: str) -> list[GraphNode]:
        """Return nodes referenced via USES_TYPE from *node_id*."""
        assert self._conn is not None
        q = self._rel_query(node_id, "outgoing")
        return [] if q is None else self._query_nodes(q, parameters={"nid": node_id, "rel_type": "uses_type"})

    def get_callers_with_confidence(self, node_id: str) -> list[tuple[GraphNode, float]]:
        """Return ``(node, confidence)`` for all callers of *node_id*."""
        assert self._conn is not None
        q = self._rel_query(node_id, "incoming_conf")
        if q is None:
            return []
        return self._query_nodes_with_confidence(q, parameters={"nid": node_id, "rel_type": "calls"})

    def get_callees_with_confidence(self, node_id: str) -> list[tuple[GraphNode, float]]:
        """Return ``(node, confidence)`` for all callees of *node_id*."""
        assert self._conn is not None
        q = self._rel_query(node_id, "outgoing_conf")
        if q is None:
            return []
        return self._query_nodes_with_confidence(q, parameters={"nid": node_id, "rel_type": "calls"})

    _MAX_BFS_DEPTH = 10

    def traverse(self, start_id: str, depth: int, direction: str = "callers") -> list[GraphNode]:
        """BFS traversal through CALLS edges -- flat result list (no depth info)."""
        return [node for node, _ in self.traverse_with_depth(start_id, depth, direction)]

    def traverse_with_depth(
        self, start_id: str, depth: int, direction: str = "callers"
    ) -> list[tuple[GraphNode, int]]:
        """BFS traversal returning ``(node, hop_depth)`` pairs using batched queries."""
        assert self._conn is not None
        depth = min(depth, self._MAX_BFS_DEPTH)
        if _table_for_id(start_id) is None:
            return []
            
        visited: set[str] = {start_id}
        result_list: list[tuple[GraphNode, int]] = []
        current_level_ids = [start_id]
        
        for current_depth in range(1, depth + 1):
            if not current_level_ids:
                break
                
            if direction == "callers":
                query = (
                    "MATCH (caller)-[r:CodeRelation]->(callee) "
                    "WHERE callee.id IN $ids AND r.rel_type = 'calls' "
                    "RETURN caller"
                )
            else:
                query = (
                    "MATCH (caller)-[r:CodeRelation]->(callee) "
                    "WHERE caller.id IN $ids AND r.rel_type = 'calls' "
                    "RETURN callee"
                )
                
            next_level_ids = []
            try:
                result = self._conn.execute(query, parameters={"ids": current_level_ids})
                while result.has_next():
                    row = result.get_next()
                    node = self._row_to_node(row[0])
                    if node is not None and node.id not in visited:
                        visited.add(node.id)
                        result_list.append((node, current_depth))
                        next_level_ids.append(node.id)
            except RuntimeError:
                logger.error("traverse_with_depth batch query failed", exc_info=True)
                break
                
            current_level_ids = next_level_ids
            
        return result_list

    def export_to_graph(self) -> KnowledgeGraph:
        """Export the entire database to an in-memory KnowledgeGraph."""
        assert self._conn is not None
        graph = KnowledgeGraph()
        
        # Load all nodes
        for table in _SEARCHABLE_TABLES + ["Folder", "File"]:
            try:
                result = self._conn.execute(f"MATCH (n:{table}) RETURN n")
                while result.has_next():
                    row = result.get_next()
                    node_dict = row[0]
                    node = self._row_to_node(node_dict)
                    if node:
                        graph.add_node(node)
            except RuntimeError:
                logger.error("Failed to export nodes for table %s", table, exc_info=True)
                
        # Load all relationships
        # CodeRelation is a REL TABLE GROUP, so we can just query it globally
        try:
            result = self._conn.execute(
                "MATCH (a)-[r:CodeRelation]->(b) RETURN a.id, b.id, r.rel_type, r"
            )
            while result.has_next():
                row = result.get_next()
                src_id, tgt_id, rel_type_str, rel_dict = row[0], row[1], row[2], row[3]
                rel_id = f"{rel_type_str}:{src_id}->{tgt_id}"
                
                # rel_dict contains confidence, role, etc.
                props = {}
                if "confidence" in rel_dict: props["confidence"] = rel_dict["confidence"]
                if "role" in rel_dict: props["role"] = rel_dict["role"]
                if "step_number" in rel_dict: props["step_number"] = rel_dict["step_number"]
                if "strength" in rel_dict: props["strength"] = rel_dict["strength"]
                if "co_changes" in rel_dict: props["co_changes"] = rel_dict["co_changes"]
                if "symbols" in rel_dict: props["symbols"] = rel_dict["symbols"]
                
                from axon.core.graph.model import RelType
                try:
                    rel_type = RelType(rel_type_str)
                except ValueError:
                    continue
                    
                graph.add_relationship(
                    GraphRelationship(
                        id=rel_id,
                        type=rel_type,
                        source=src_id,
                        target=tgt_id,
                        properties=props
                    )
                )
        except RuntimeError:
            logger.error("Failed to export relationships", exc_info=True)
            
        return graph

    def update_global_stats(self, graph: KnowledgeGraph) -> None:
        """Update is_dead, centrality, and community properties in the DB from the graph."""
        assert self._conn is not None
        # Bulk update centrality and is_dead
        for table in _SEARCHABLE_TABLES:
            try:
                # Kuzu doesn't easily support bulk UPDATE FROM list, so we'll do individual updates
                # or a small batch. Given this is for incremental, doing it safely is priority.
                # Since we don't have UNWIND, we just run execute for each or rebuild if needed.
                pass # We will implement a better batching below
            except RuntimeError:
                pass
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
            logger.error("get_process_memberships failed", exc_info=True)
        return mapping

    def execute_raw(self, query: str, parameters: dict | None = None) -> list[list[Any]]:
        """Execute a raw Cypher query and return all result rows."""
        assert self._conn is not None
        result = self._conn.execute(query, parameters=parameters or {})
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

    def get_embedding(self, node_id: str) -> list[float] | None:
        """Return the stored embedding vector for *node_id*, or None if absent."""
        assert self._conn is not None
        from axon.core.storage.kuzu_search import get_embedding
        return get_embedding(self._conn, node_id)

    def store_embeddings(self, embeddings: Iterable[NodeEmbedding]) -> None:
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
                logger.error("store_embeddings failed for node %s", emb.node_id, exc_info=True)

    def get_indexed_files(self) -> dict[str, str]:
        """Return ``{file_path: sha256(content)}`` for all File nodes."""
        assert self._conn is not None
        from axon.core.storage.kuzu_bulk import get_indexed_files
        return get_indexed_files(self._conn)

    def bulk_load(self, nodes: Iterable[GraphNode] | KnowledgeGraph, rels: Iterable[GraphRelationship] | None = None) -> None:
        """Replace the entire store with the provided nodes and relationships."""
        assert self._conn is not None
        from axon.core.graph.graph import KnowledgeGraph
        if isinstance(nodes, KnowledgeGraph):
            rels = nodes.iter_relationships()
            nodes = nodes.iter_nodes()
        
        if rels is None:
            rels = []

        from axon.core.storage.kuzu_bulk import bulk_load
        bulk_load(self._conn, nodes, rels, self.add_nodes, self.add_relationships)

    def rebuild_fts_indexes(self) -> None:
        """Drop and recreate all FTS indexes."""
        assert self._conn is not None
        from axon.core.storage.kuzu_bulk import rebuild_fts_indexes
        rebuild_fts_indexes(self._conn)

    _INSERT_NODE_CYPHER = (
        "MERGE (n:{table} {{id: $id}}) "
        "SET n.name = $name, n.file_path = $file_path, "
        "n.start_line = $start_line, n.end_line = $end_line, "
        "n.start_byte = $start_byte, n.end_byte = $end_byte, "
        "n.content = $content, "
        "n.signature = $signature, n.language = $language, n.class_name = $class_name, "
        "n.is_dead = $is_dead, n.is_entry_point = $is_entry_point, "
        "n.is_exported = $is_exported, n.tested = $tested, n.centrality = $centrality"
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
                  "start_byte": node.start_byte, "end_byte": node.end_byte,
                  "content": node.content, "signature": node.signature,
                  "language": node.language, "class_name": node.class_name,
                  "is_dead": node.is_dead, "is_entry_point": node.is_entry_point,
                  "is_exported": node.is_exported,
                  "tested": node.tested, "centrality": node.centrality}
        try:
            self._conn.execute(self._INSERT_NODE_CYPHER.format(table=table), parameters=params)
        except RuntimeError:
            logger.error("Insert node failed for %s", node.id, exc_info=True)

    _INSERT_REL_CYPHER = (
        "MATCH (a:{src}), (b:{dst}) WHERE a.id = $src AND b.id = $tgt "
        "MERGE (a)-[r:CodeRelation {{rel_type: $rel_type}}]->(b) "
        "SET r.confidence = $confidence, r.role = $role, r.step_number = $step_number, "
        "r.strength = $strength, r.co_changes = $co_changes, r.symbols = $symbols"
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
            logger.error("Insert rel failed: %s -> %s", rel.source, rel.target, exc_info=True)

    def _query_nodes(
        self, query: str, parameters: dict[str, Any] | None = None
    ) -> list[GraphNode]:
        """Execute a query returning a node as the first column and convert to GraphNode list."""
        assert self._conn is not None
        nodes: list[GraphNode] = []
        try:
            result = self._conn.execute(query, parameters=parameters or {})
            while result.has_next():
                row = result.get_next()
                node = self._row_to_node(row[0])
                if node is not None:
                    nodes.append(node)
        except RuntimeError:
            logger.error("_query_nodes failed: %s", query, exc_info=True)
        return nodes

    def _query_nodes_with_confidence(
        self, query: str, parameters: dict[str, Any] | None = None
    ) -> list[tuple[GraphNode, float]]:
        """Execute a query returning a node and trailing confidence column."""
        assert self._conn is not None
        pairs: list[tuple[GraphNode, float]] = []
        try:
            result = self._conn.execute(query, parameters=parameters or {})
            while result.has_next():
                row = result.get_next()
                node = self._row_to_node(row[0])
                confidence = float(row[-1]) if row[-1] is not None else 1.0
                if node is not None:
                    pairs.append((node, confidence))
        except RuntimeError:
            logger.error("_query_nodes_with_confidence failed: %s", query, exc_info=True)
        return pairs

    @staticmethod
    def _row_to_node(node_dict: dict[str, Any], node_id: str | None = None) -> GraphNode | None:
        """Convert a result node dictionary into a GraphNode.
        """
        try:
            nid = node_id or node_dict.get("id", "")
            label = _LABEL_MAP.get(nid.split(":", 1)[0], NodeLabel.FILE)
            
            props = node_dict.get("properties", {})
            if "content_hash" in node_dict:
                props["content_hash"] = node_dict["content_hash"]
            if "decorators" in node_dict:
                props["decorators"] = node_dict["decorators"]
            if "bases" in node_dict:
                props["bases"] = node_dict["bases"]

            return GraphNode(
                id=nid, 
                label=label, 
                name=node_dict.get("name", ""), 
                file_path=node_dict.get("file_path", ""),
                start_line=node_dict.get("start_line") or 0, 
                end_line=node_dict.get("end_line") or 0,
                start_byte=node_dict.get("start_byte") or 0,
                end_byte=node_dict.get("end_byte") or 0,
                content=node_dict.get("content", ""),
                signature=node_dict.get("signature", ""),
                language=node_dict.get("language", ""),
                class_name=node_dict.get("class_name", ""),
                is_dead=bool(node_dict.get("is_dead")),
                is_entry_point=bool(node_dict.get("is_entry_point")),
                is_exported=bool(node_dict.get("is_exported")),
                tested=bool(node_dict.get("tested")),
                centrality=float(node_dict.get("centrality") or 0.0),
                properties=props,
            )
        except (IndexError, KeyError, TypeError, ValueError):
            logger.debug("Failed to convert row to GraphNode: %s", node_dict, exc_info=True)
            return None
