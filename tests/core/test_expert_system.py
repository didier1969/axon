from __future__ import annotations
import pytest
from axon.core.graph.graph import KnowledgeGraph
from axon.core.graph.model import GraphNode, NodeLabel, RelType, GraphRelationship
from axon.core.analysis.audit import AuditEngine
from axon.core.analysis.data_flow import DataFlowAnalyzer

@pytest.fixture
def expert_graph():
    graph = KnowledgeGraph()
    
    # Entry Point (Public API)
    ep = GraphNode(
        id="api:handler", label=NodeLabel.FUNCTION, name="handler",
        content="def handler(req): process(req.body)",
        is_entry_point=True, file_path="api.py"
    )
    
    # Intermediate function (Tainted)
    inter = GraphNode(
        id="logic:process", label=NodeLabel.FUNCTION, name="process",
        content="def process(data): db_execute(data)",
        file_path="logic.py"
    )
    
    # Sink (Dangerous SQL)
    sink = GraphNode(
        id="db:execute", label=NodeLabel.FUNCTION, name="db_execute",
        content="def db_execute(sql): conn.execute(sql)",
        file_path="db.py"
    )
    
    # Sensitive function without auth (A01)
    admin = GraphNode(
        id="admin:delete_all", label=NodeLabel.FUNCTION, name="delete_all_users",
        content="def delete_all_users(): pass",
        file_path="admin.py"
    )
    
    graph.add_node(ep)
    graph.add_node(inter)
    graph.add_node(sink)
    graph.add_node(admin)
    
    # Add relationships with arguments for Data Flow
    graph.add_relationship(GraphRelationship(
        id="r1", type=RelType.CALLS, source="api:handler", target="logic:process",
        properties={"arguments": ["req.body"]}
    ))
    graph.add_relationship(GraphRelationship(
        id="r2", type=RelType.CALLS, source="logic:process", target="db:execute",
        properties={"arguments": ["data"]}
    ))
    
    return graph

def test_audit_owasp_a01_access_control(expert_graph):
    engine = AuditEngine(expert_graph)
    reports = engine.run_all()
    
    # Should detect admin:delete_all as risky
    a01_reports = [r for r in reports if r.type == "OWASP_A01_ACCESS_CONTROL"]
    assert len(a01_reports) > 0
    assert "delete_all_users" in a01_reports[0].message

def test_audit_owasp_a03_injection_with_path(expert_graph):
    engine = AuditEngine(expert_graph)
    reports = engine.run_all()
    
    # Should detect db_execute as a sink reachable from handler
    a03_reports = [r for r in reports if r.type == "OWASP_A03_INJECTION"]
    assert len(a03_reports) > 0
    assert a03_reports[0].exposure_path is not None
    assert len(a03_reports[0].exposure_path) >= 2 # api:handler -> logic:process -> db:execute

def test_audit_owasp_a07_auth_gap(expert_graph):
    engine = AuditEngine(expert_graph)
    reports = engine.run_all()
    
    # api:handler has no dependency on 'auth' modules
    a07_reports = [r for r in reports if r.type == "OWASP_A07_AUTH_GAP"]
    assert len(a07_reports) > 0
    assert "handler" in a07_reports[0].message

def test_data_flow_tracing(expert_graph):
    analyzer = DataFlowAnalyzer(expert_graph)
    
    # Trace 'req.body' from handler
    paths = analyzer.trace_variable("api:handler", "req.body")
    
    assert len(paths) > 0
    # The path should reach db:execute
    assert any(p.target_id == "db:execute" for p in paths)
    
    # Verify steps
    path = paths[0]
    assert path.steps[0].symbol_name == "handler"
    assert "req.body" in path.steps[0].passed_arguments
    assert path.steps[1].symbol_name == "process"

def test_remediation_suggestion(expert_graph):
    engine = AuditEngine(expert_graph)
    reports = engine.run_all()
    
    # Check that remediation is provided
    for r in reports:
        assert r.remediation != ""
        assert isinstance(r.remediation, str)
