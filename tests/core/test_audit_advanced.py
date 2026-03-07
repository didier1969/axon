from __future__ import annotations
import pytest
from axon.core.graph.graph import KnowledgeGraph
from axon.core.graph.model import GraphNode, NodeLabel, RelType, GraphRelationship
from axon.core.analysis.audit import AuditEngine

def test_audit_clustering_tests():
    graph = KnowledgeGraph()
    # 3 tests files in the same folder with Auth Gap
    for i in range(3):
        node = GraphNode(
            id=f"t{i}", label=NodeLabel.FUNCTION, name=f"test_func_{i}",
            file_path=f"tests/core/test_{i}.py", is_entry_point=True
        )
        graph.add_node(node)
    
    engine = AuditEngine(graph)
    
    # Mode Clustered (Default)
    reports = engine.run_all(cluster=True)
    assert len(reports) == 1
    assert reports[0].count == 3
    assert "Multiple test files" in reports[0].message
    
    # Mode Verbose (No Clustering)
    reports_verbose = engine.run_all(cluster=False)
    assert len(reports_verbose) == 3

def test_audit_ejection_by_centrality():
    graph = KnowledgeGraph()
    # 2 files in .paul, one is a common report, one is a critical hub
    n1 = GraphNode(
        id="paul1", label=NodeLabel.FUNCTION, name="report1",
        file_path=".paul/handoffs/archive/1.md", content="Header1", centrality=0.01
    )
    n2 = GraphNode(
        id="paul2", label=NodeLabel.FUNCTION, name="report2",
        file_path=".paul/handoffs/archive/2.md", content="Header1", centrality=0.01
    )
    # This one is a twin but has HIGH CENTRALITY
    n3 = GraphNode(
        id="critical_hub", label=NodeLabel.FUNCTION, name="critical_report",
        file_path=".paul/handoffs/archive/hub.md", content="Header1", centrality=0.9
    )
    
    graph.add_node(n1)
    graph.add_node(n2)
    graph.add_node(n3)
    
    engine = AuditEngine(graph)
    reports = engine.run_all(cluster=True)
    
    # Should have 2 reports: 1 for the cluster (2 twins) and 1 for the ejected hub
    assert len(reports) == 2
    hub_report = next(r for r in reports if r.symbol_ids[0] == "critical_hub")
    assert hub_report.count == 1 # Ejected from cluster

def test_audit_location_folder():
    graph = KnowledgeGraph()
    # 2 twins in a specific folder
    n1 = GraphNode(id="f1", label=NodeLabel.FUNCTION, name="twin1", content="long enough content for twins", file_path="src/utils/a.py")
    n2 = GraphNode(id="f2", label=NodeLabel.FUNCTION, name="twin2", content="long enough content for twins", file_path="src/utils/b.py")
    graph.add_node(n1)
    graph.add_node(n2)
    
    engine = AuditEngine(graph)
    reports = engine.run_all(cluster=True)
    
    # Check folder key generation (indirectly via report count)
    assert any(r.type == "STRUCTURAL_TWIN" for r in reports)
