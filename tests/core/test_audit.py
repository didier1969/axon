from __future__ import annotations
import pytest
from axon.core.graph.graph import KnowledgeGraph
from axon.core.graph.model import GraphNode, NodeLabel, RelType, GraphRelationship
from axon.core.analysis.audit import AuditEngine

def test_audit_detects_shallow_implementation():
    graph = KnowledgeGraph()
    # A function named 'persist' but with almost no content (Shallow Implementation)
    node = GraphNode(
        id="f1", label=NodeLabel.FUNCTION, name="persist_data",
        content="def persist_data(data): pass", # Very short
        file_path="storage.py", centrality=0.8 # Highly important
    )
    graph.add_node(node)
    
    engine = AuditEngine(graph)
    reports = engine.run_all()
    
    # Should detect the semantic gap
    assert any(r.type == "SEMANTIC_GAP" for r in reports)

def test_audit_detects_structural_twins():
    graph = KnowledgeGraph()
    # Two functions with different names but exactly the same structure/content
    # (Structural Twins / Divergence Risk)
    n1 = GraphNode(id="f1", label=NodeLabel.FUNCTION, name="sort_v1", content="sort(x)", file_path="a.rs")
    n2 = GraphNode(id="f2", label=NodeLabel.FUNCTION, name="sort_v2", content="sort(x)", file_path="b.rs")
    graph.add_node(n1)
    graph.add_node(n2)
    
    engine = AuditEngine(graph)
    reports = engine.run_all()
    
    assert any(r.type == "STRUCTURAL_TWIN" for r in reports)

def test_audit_detects_fragile_boundary():
    graph = KnowledgeGraph()
    # Calling a NIF (external) without guards (is_list, etc.)
    caller = GraphNode(id="c1", label=NodeLabel.FUNCTION, name="call_nif", content="Nif.run(data)", file_path="bridge.ex")
    nif = GraphNode(id="n1", label=NodeLabel.FUNCTION, name="run", file_path="nif_bridge.ex")
    graph.add_node(caller)
    graph.add_node(nif)
    graph.add_relationship(GraphRelationship(id="r1", type=RelType.CALLS, source="c1", target="n1"))
    
    engine = AuditEngine(graph)
    reports = engine.run_all()
    
    assert any(r.type == "FRAGILE_BOUNDARY" for r in reports)
