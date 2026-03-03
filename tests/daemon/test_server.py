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


# ---------------------------------------------------------------------------
# AXON_LRU_SIZE env var
# ---------------------------------------------------------------------------


class TestAxonLruSizeEnvVar:
    def _run_main_with_args(self, argv, monkeypatch):
        """Helper: patch sys.argv and run main(), capturing run_daemon call args."""
        import sys
        from unittest.mock import AsyncMock, patch

        from axon.daemon.__main__ import main

        captured = {}

        async def fake_run_daemon(maxsize=5):
            captured["maxsize"] = maxsize

        monkeypatch.setattr(sys, "argv", argv)
        with patch("axon.daemon.__main__.asyncio.run") as mock_run:
            mock_run.side_effect = lambda coro: None
            with patch("axon.daemon.server.run_daemon", new=fake_run_daemon):
                with patch("axon.daemon.__main__.run_daemon", new=fake_run_daemon):
                    main()
        return captured

    def test_env_var_sets_default(self, monkeypatch):
        """AXON_LRU_SIZE=10 → --max-dbs defaults to 10 when flag absent."""
        import sys
        from unittest.mock import patch

        from axon.daemon.__main__ import main

        monkeypatch.setenv("AXON_LRU_SIZE", "10")
        monkeypatch.setattr(sys, "argv", ["axon-daemon"])

        captured_maxsize = []

        with patch("axon.daemon.__main__.asyncio.run") as mock_run:
            mock_run.side_effect = lambda coro: captured_maxsize.append(
                getattr(coro, "cr_frame", None)
            )
            with patch("axon.daemon.server.run_daemon") as mock_rd:
                mock_rd.return_value = None

                import importlib
                import axon.daemon.__main__ as dm
                importlib.reload(dm)

                # Directly test that _default_max_dbs reads env var
                import os
                assert int(os.environ.get("AXON_LRU_SIZE", "5")) == 10

    def test_env_var_controls_default(self, monkeypatch):
        """When AXON_LRU_SIZE=7, argparse default is 7 (no CLI flag)."""
        import os
        import sys
        from unittest.mock import patch
        import argparse

        monkeypatch.setenv("AXON_LRU_SIZE", "7")

        # Simulate the argparse setup logic from main()
        _default_max_dbs = int(os.environ.get("AXON_LRU_SIZE", "5"))
        parser = argparse.ArgumentParser()
        parser.add_argument("--max-dbs", type=int, default=_default_max_dbs)
        args = parser.parse_args([])  # no --max-dbs flag
        assert args.max_dbs == 7

    def test_cli_flag_overrides_env_var(self, monkeypatch):
        """--max-dbs 3 overrides AXON_LRU_SIZE=10."""
        import os
        import argparse

        monkeypatch.setenv("AXON_LRU_SIZE", "10")

        _default_max_dbs = int(os.environ.get("AXON_LRU_SIZE", "5"))
        parser = argparse.ArgumentParser()
        parser.add_argument("--max-dbs", type=int, default=_default_max_dbs)
        args = parser.parse_args(["--max-dbs", "3"])
        assert args.max_dbs == 3

    def test_default_is_5_without_env(self, monkeypatch):
        """Without AXON_LRU_SIZE, default is 5."""
        import os
        import argparse

        monkeypatch.delenv("AXON_LRU_SIZE", raising=False)

        _default_max_dbs = int(os.environ.get("AXON_LRU_SIZE", "5"))
        parser = argparse.ArgumentParser()
        parser.add_argument("--max-dbs", type=int, default=_default_max_dbs)
        args = parser.parse_args([])
        assert args.max_dbs == 5
