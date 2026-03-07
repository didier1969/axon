from __future__ import annotations
import pytest
import json
from pathlib import Path
from axon.core.storage.kuzu_backend import KuzuBackend
from axon.core.graph.model import GraphNode, NodeLabel, RelType, GraphRelationship

def test_expert_schema_serialization(tmp_path):
    backend = KuzuBackend()
    backend.initialize(tmp_path / "test_db")
    
    # 1. Create a node with expert properties
    node = GraphNode(
        id="function:file.py:risky_fn", label=NodeLabel.FUNCTION, name="risky_fn",
        properties={"unsafe": True, "nif_loader": True}
    )
    backend.add_nodes([node])
    
    # 2. Create a relationship with arguments
    rel = GraphRelationship(
        id="rel:1", type=RelType.CALLS, source="function:file.py:risky_fn", target="function:file.py:target_fn",
        properties={"arguments": ["user_id", "token"]}
    )
    # target node needs to exist
    node2 = GraphNode(id="function:file.py:target_fn", label=NodeLabel.FUNCTION, name="target_fn")
    backend.add_nodes([node2])
    backend.add_relationships([rel])
    
    # 3. Read back from DB
    read_node = backend.get_node("function:file.py:risky_fn")
    assert read_node is not None
    assert read_node.properties.get("unsafe") is True
    assert read_node.properties.get("nif_loader") is True
    
    # 4. Export to graph and check relationships
    graph = backend.export_to_graph()
    rels = list(graph.iter_relationships())
    assert len(rels) > 0
    assert "arguments" in rels[0].properties
    assert "user_id" in rels[0].properties["arguments"]
    
    backend.close()
