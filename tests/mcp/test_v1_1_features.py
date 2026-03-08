import pytest
from axon.mcp.server import create_mcp_server, TOOLS

@pytest.mark.asyncio
async def test_mcp_axon_audit_tool_exists():
    """Verify that axon_audit is exposed in the MCP tool list."""
    audit_tool = next((t for t in TOOLS if t.name == "axon_audit"), None)
    assert audit_tool is not None
    assert "repo" in audit_tool.inputSchema["properties"]

@pytest.mark.asyncio
async def test_mcp_axon_audit_execution(monkeypatch):
    """Verify that axon_audit dispatch logic works."""
    from axon.mcp.server import _dispatch_tool
    from unittest.mock import MagicMock
    
    mock_storage = MagicMock()
    
    # Mocking storage loading and audit execution
    with monkeypatch.context() as m:
        m.setattr("axon.mcp.tools._load_repo_storage", lambda r: mock_storage)
        m.setattr("axon.core.analysis.audit.AuditEngine.run_all", lambda self: [])
        
        # We don't provide a repo arg to use the current storage directly
        result = _dispatch_tool("axon_audit", {"check_type": "security"}, mock_storage)
        
    assert "Audit complete" in result
