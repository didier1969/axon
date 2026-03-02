"""Tests for the daemon protocol and tool dispatch."""
import json
from unittest.mock import MagicMock, patch

from axon.daemon.lru_cache import LRUBackendCache
from axon.daemon.protocol import decode_request, encode_request, encode_response
from axon.daemon.server import _dispatch_tool


class TestProtocol:
    def test_encode_decode_request(self):
        encoded = encode_request("axon_query", {"query": "foo"}, slug="myapp", request_id="1")
        decoded = decode_request(encoded)
        assert decoded["tool"] == "axon_query"
        assert decoded["slug"] == "myapp"
        assert decoded["args"]["query"] == "foo"
        assert decoded["id"] == "1"

    def test_encode_response_success(self):
        encoded = encode_response("result text", None, "req-1")
        decoded = json.loads(encoded)
        assert decoded["result"] == "result text"
        assert decoded["error"] is None
        assert decoded["id"] == "req-1"

    def test_encode_response_error(self):
        encoded = encode_response(None, "something went wrong", "req-2")
        decoded = json.loads(encoded)
        assert decoded["result"] is None
        assert decoded["error"] == "something went wrong"


class TestDispatch:
    def test_dispatch_list_repos(self):
        cache = MagicMock(spec=LRUBackendCache)
        with patch("axon.mcp.tools.handle_list_repos", return_value="repos: myapp") as mock_lr:
            result = _dispatch_tool(cache, "axon_list_repos", None, {})
        assert result == "repos: myapp"
        mock_lr.assert_called_once()

    def test_dispatch_unknown_tool(self):
        cache = MagicMock(spec=LRUBackendCache)
        result = _dispatch_tool(cache, "axon_nonexistent", "myapp", {})
        assert "Unknown tool" in result

    def test_dispatch_missing_slug_for_tool_requiring_storage(self):
        cache = MagicMock(spec=LRUBackendCache)
        result = _dispatch_tool(cache, "axon_query", None, {"query": "foo"})
        assert "slug required" in result.lower() or "error" in result.lower()

    def test_dispatch_slug_not_found(self):
        cache = MagicMock(spec=LRUBackendCache)
        cache.get_or_load.return_value = None
        result = _dispatch_tool(cache, "axon_query", "unknown", {"query": "foo"})
        assert "no index" in result.lower() or "not found" in result.lower()

    def test_dispatch_query_calls_handle_query(self):
        cache = MagicMock(spec=LRUBackendCache)
        mock_storage = MagicMock()
        cache.get_or_load.return_value = mock_storage
        with patch("axon.mcp.tools.handle_query", return_value="query result") as mock_hq:
            result = _dispatch_tool(cache, "axon_query", "myapp", {"query": "foo", "limit": 10})
        assert result == "query result"
        mock_hq.assert_called_once_with(mock_storage, "foo", limit=10, language=None)

    def test_dispatch_daemon_status(self):
        cache = MagicMock(spec=LRUBackendCache)
        cache.status.return_value = {"cached": ["a", "b"], "count": 2, "maxsize": 5}
        result = _dispatch_tool(cache, "axon_daemon_status", None, {})
        assert "2/5" in result
        assert "a" in result
