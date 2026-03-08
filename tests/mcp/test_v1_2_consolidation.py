import pytest
from axon.mcp.server import create_mcp_server, TOOLS

@pytest.mark.asyncio
async def test_mcp_v1_2_tool_list():
    """Verify that only the 8 consolidated tools are exposed."""
    expected_tools = {
        "axon_query", "axon_inspect", "axon_audit", "axon_impact",
        "axon_health", "axon_diff", "axon_batch", "axon_cypher"
    }
    current_tools = {t.name for t in TOOLS}
    
    # This will fail initially as we still have 17 tools
    assert current_tools == expected_tools

@pytest.mark.asyncio
async def test_axon_inspect_capability(monkeypatch):
    """Verify that axon_inspect returns both source and relationships."""
    from axon.mcp.server import _dispatch_tool
    from unittest.mock import MagicMock
    
    mock_storage = MagicMock()
    # Mocking code retrieval and graph traversal
    with monkeypatch.context() as m:
        m.setattr("axon.mcp.tools.handle_read_symbol", lambda *args, **kwargs: "def test(): pass")
        m.setattr("axon.mcp.tools.handle_context", lambda *args, **kwargs: "Callers: []")
        
        result = _dispatch_tool("axon_inspect", {"symbol": "test"}, mock_storage)
        
    assert "def test()" in result
    assert "Callers" in result
