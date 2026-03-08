from __future__ import annotations
from typing import Any
from axon.core.graph.graph import KnowledgeGraph
from axon.core.graph.model import (
    GraphNode,
    GraphRelationship,
    NodeLabel,
    RelType,
    generate_id,
)

def add_file_node(graph: KnowledgeGraph, path: str) -> str:
    """Add a File node and return its ID."""
    node_id = generate_id(NodeLabel.FILE, path)
    graph.add_node(
        GraphNode(
            id=node_id,
            label=NodeLabel.FILE,
            name=path.rsplit("/", 1)[-1],
            file_path=path,
        )
    )
    return node_id

def add_symbol_node(
    graph: KnowledgeGraph,
    label: NodeLabel,
    file_path: str,
    name: str,
    *,
    start_line: int = 1,
    end_line: int = 1,
    is_exported: bool = False,
    is_entry_point: bool = False,
    class_name: str = "",
    properties: dict[str, Any] | None = None,
    defines_from_file: bool = True,
) -> str:
    """
    Add a symbol node and return its ID.
    By default, also adds a DEFINES relationship from the parent file node.
    """
    symbol_name = (
        f"{class_name}.{name}" if label == NodeLabel.METHOD and class_name else name
    )
    node_id = generate_id(label, file_path, symbol_name)
    graph.add_node(
        GraphNode(
            id=node_id,
            label=label,
            name=name,
            file_path=file_path,
            start_line=start_line,
            end_line=end_line,
            class_name=class_name,
            is_exported=is_exported,
            is_entry_point=is_entry_point,
            properties=properties or {},
        )
    )
    
    if defines_from_file:
        file_id = generate_id(NodeLabel.FILE, file_path)
        # Ensure file node exists (or at least its ID is known)
        graph.add_relationship(
            GraphRelationship(
                id=f"defines:{file_id}->{node_id}",
                type=RelType.DEFINES,
                source=file_id,
                target=node_id,
            )
        )
        
    return node_id

def add_calls_relationship(
    graph: KnowledgeGraph,
    source_id: str,
    target_id: str,
    confidence: float = 1.0,
) -> None:
    """Add a CALLS relationship from *source_id* to *target_id*."""
    rel_id = f"calls:{source_id}->{target_id}"
    graph.add_relationship(
        GraphRelationship(
            id=rel_id,
            type=RelType.CALLS,
            source=source_id,
            target=target_id,
            properties={"confidence": confidence}
        )
    )
