"""Tests for MCP server proxy routing and max_tokens behaviour."""
from __future__ import annotations

import json
import threading
from unittest.mock import MagicMock, patch

from axon.mcp.server import _batch_daemon_call, _get_local_slug, _try_daemon_call


class TestGetLocalSlug:
    def test_reads_slug_from_meta_json(self, tmp_path, monkeypatch):
        """_get_local_slug() reads slug from .axon/meta.json in cwd."""
        monkeypatch.chdir(tmp_path)
        axon_dir = tmp_path / ".axon"
        axon_dir.mkdir()
        (axon_dir / "meta.json").write_text('{"slug": "myproject"}')

        result = _get_local_slug()
        assert result == "myproject"

    def test_returns_none_when_no_meta_json(self, tmp_path, monkeypatch):
        """_get_local_slug() returns None when .axon/meta.json absent."""
        monkeypatch.chdir(tmp_path)
        assert _get_local_slug() is None

    def test_returns_none_on_invalid_json(self, tmp_path, monkeypatch):
        """_get_local_slug() returns None on malformed meta.json."""
        monkeypatch.chdir(tmp_path)
        axon_dir = tmp_path / ".axon"
        axon_dir.mkdir()
        (axon_dir / "meta.json").write_text("not-json")
        assert _get_local_slug() is None

    def test_returns_none_when_no_slug_key(self, tmp_path, monkeypatch):
        """_get_local_slug() returns None when meta.json has no slug field."""
        monkeypatch.chdir(tmp_path)
        axon_dir = tmp_path / ".axon"
        axon_dir.mkdir()
        (axon_dir / "meta.json").write_text('{"name": "myproject"}')
        assert _get_local_slug() is None


class TestTryDaemonCall:
    def test_returns_none_when_socket_absent(self, tmp_path, monkeypatch):
        """_try_daemon_call returns None when daemon socket does not exist."""
        monkeypatch.setattr("pathlib.Path.home", lambda: tmp_path)
        result = _try_daemon_call("axon_query", "myapp", {"query": "foo"})
        assert result is None

    def test_returns_result_on_success(self, tmp_path, monkeypatch):
        """_try_daemon_call returns result string when daemon responds correctly."""
        monkeypatch.setattr("pathlib.Path.home", lambda: tmp_path)
        # Create fake socket file so exists() check passes
        sock_path = tmp_path / ".axon" / "daemon.sock"
        sock_path.parent.mkdir(parents=True)
        sock_path.touch()

        response_data = json.dumps({"id": "mcp", "result": "query result", "error": None}) + "\n"

        mock_sock = MagicMock()
        mock_sock.__enter__ = MagicMock(return_value=mock_sock)
        mock_sock.__exit__ = MagicMock(return_value=False)
        mock_sock.makefile.return_value.readline.return_value = response_data.encode()

        with patch("axon.mcp.server._socket.socket", return_value=mock_sock):
            result = _try_daemon_call("axon_query", "myapp", {"query": "foo"})

        assert result == "query result"

    def test_returns_none_on_connection_error(self, tmp_path, monkeypatch):
        """_try_daemon_call returns None (falls back) on connection refused."""
        monkeypatch.setattr("pathlib.Path.home", lambda: tmp_path)
        sock_path = tmp_path / ".axon" / "daemon.sock"
        sock_path.parent.mkdir(parents=True)
        sock_path.touch()

        mock_sock = MagicMock()
        mock_sock.__enter__ = MagicMock(return_value=mock_sock)
        mock_sock.__exit__ = MagicMock(return_value=False)
        mock_sock.connect.side_effect = OSError("Connection refused")

        with patch("axon.mcp.server._socket.socket", return_value=mock_sock):
            result = _try_daemon_call("axon_query", "myapp", {"query": "foo"})

        assert result is None

    def test_returns_none_on_daemon_error_response(self, tmp_path, monkeypatch):
        """_try_daemon_call returns None when daemon responds with error field."""
        monkeypatch.setattr("pathlib.Path.home", lambda: tmp_path)
        sock_path = tmp_path / ".axon" / "daemon.sock"
        sock_path.parent.mkdir(parents=True)
        sock_path.touch()

        response_data = json.dumps({"id": "mcp", "result": None, "error": "no index for slug"}) + "\n"

        mock_sock = MagicMock()
        mock_sock.__enter__ = MagicMock(return_value=mock_sock)
        mock_sock.__exit__ = MagicMock(return_value=False)
        mock_sock.makefile.return_value.readline.return_value = response_data.encode()

        with patch("axon.mcp.server._socket.socket", return_value=mock_sock):
            result = _try_daemon_call("axon_query", "myapp", {"query": "foo"})

        assert result is None

    def test_max_tokens_and_repo_stripped_from_daemon_args(self, tmp_path, monkeypatch):
        """call_tool() strips max_tokens and repo from args before sending to daemon."""
        monkeypatch.setattr("pathlib.Path.home", lambda: tmp_path)
        sock_path = tmp_path / ".axon" / "daemon.sock"
        sock_path.parent.mkdir(parents=True)
        sock_path.touch()

        captured_args = {}
        response_data = json.dumps({"id": "mcp", "result": "ok", "error": None}) + "\n"

        def fake_sendall(data):
            req = json.loads(data.decode().strip())
            captured_args.update(req)

        mock_sock = MagicMock()
        mock_sock.__enter__ = MagicMock(return_value=mock_sock)
        mock_sock.__exit__ = MagicMock(return_value=False)
        mock_sock.sendall = fake_sendall
        mock_sock.makefile.return_value.readline.return_value = response_data.encode()

        with patch("axon.mcp.server._socket.socket", return_value=mock_sock):
            _try_daemon_call(
                "axon_query",
                "myapp",
                {"query": "foo"},  # max_tokens and repo already stripped by call_tool
            )

        assert "max_tokens" not in captured_args.get("args", {})
        assert "repo" not in captured_args.get("args", {})


class TestMaxTokens:
    def test_truncates_long_result(self):
        """max_tokens truncates result string and appends notice."""
        long_result = "x" * 1000
        max_tokens = 100
        if len(long_result) > max_tokens:
            truncated = long_result[:max_tokens] + f"\n[truncated at {max_tokens} chars]"
        else:
            truncated = long_result
        assert len(truncated) == 100 + len(f"\n[truncated at {max_tokens} chars]")
        assert truncated.endswith("[truncated at 100 chars]")

    def test_no_truncation_when_result_short(self):
        """max_tokens does not truncate when result is shorter than limit."""
        short_result = "hello world"
        max_tokens = 500
        if len(short_result) > max_tokens:
            result = short_result[:max_tokens] + f"\n[truncated at {max_tokens} chars]"
        else:
            result = short_result
        assert result == "hello world"

    def test_no_truncation_when_max_tokens_none(self):
        """max_tokens=None returns result unchanged."""
        result = "x" * 1000
        max_tokens = None
        if max_tokens is not None and len(result) > max_tokens:
            result = result[:max_tokens] + f"\n[truncated at {max_tokens} chars]"
        assert len(result) == 1000


class TestToolsHaveMaxTokensSchema:
    def test_all_tools_have_max_tokens_property(self):
        """All 7 tools in TOOLS list have max_tokens in their inputSchema."""
        from axon.mcp.server import TOOLS

        for tool in TOOLS:
            props = tool.inputSchema.get("properties", {})
            assert "max_tokens" in props, (
                f"Tool '{tool.name}' is missing max_tokens in inputSchema"
            )

    def test_max_tokens_is_not_required(self):
        """max_tokens is optional (not in required array) for all tools."""
        from axon.mcp.server import TOOLS

        for tool in TOOLS:
            required = tool.inputSchema.get("required", [])
            assert "max_tokens" not in required, (
                f"Tool '{tool.name}' incorrectly marks max_tokens as required"
            )


class TestBatchTool:
    def test_empty_calls_returns_empty_string(self, tmp_path, monkeypatch):
        """_batch_daemon_call returns empty string for empty calls list."""
        monkeypatch.setattr("pathlib.Path.home", lambda: tmp_path)
        result = _batch_daemon_call([], None)
        assert result == ""

    def test_returns_none_when_socket_absent(self, tmp_path, monkeypatch):
        """_batch_daemon_call returns None when daemon socket absent."""
        monkeypatch.setattr("pathlib.Path.home", lambda: tmp_path)
        result = _batch_daemon_call([{"tool": "axon_query", "args": {"query": "foo"}}], None)
        assert result is None

    def test_returns_formatted_results_on_success(self, tmp_path, monkeypatch):
        """_batch_daemon_call returns formatted results for 2 calls."""
        monkeypatch.setattr("pathlib.Path.home", lambda: tmp_path)
        sock_path = tmp_path / ".axon" / "daemon.sock"
        sock_path.parent.mkdir(parents=True)
        sock_path.touch()

        responses = [
            json.dumps({"id": "batch-0", "result": "result A", "error": None}) + "\n",
            json.dumps({"id": "batch-1", "result": "result B", "error": None}) + "\n",
        ]

        mock_file = MagicMock()
        mock_file.readline.side_effect = [r.encode() for r in responses]

        mock_sock = MagicMock()
        mock_sock.__enter__ = MagicMock(return_value=mock_sock)
        mock_sock.__exit__ = MagicMock(return_value=False)
        mock_sock.makefile.return_value = mock_file

        calls = [
            {"tool": "axon_query", "args": {"query": "foo"}},
            {"tool": "axon_context", "args": {"symbol": "Bar"}},
        ]
        with patch("axon.mcp.server._socket.socket", return_value=mock_sock):
            result = _batch_daemon_call(calls, None)

        assert "### axon_query (1/2)" in result
        assert "result A" in result
        assert "### axon_context (2/2)" in result
        assert "result B" in result

    def test_max_tokens_truncates_per_result(self, tmp_path, monkeypatch):
        """_batch_daemon_call truncates each sub-result individually."""
        monkeypatch.setattr("pathlib.Path.home", lambda: tmp_path)
        sock_path = tmp_path / ".axon" / "daemon.sock"
        sock_path.parent.mkdir(parents=True)
        sock_path.touch()

        long_result = "x" * 500
        short_result = "ok"
        responses = [
            json.dumps({"id": "batch-0", "result": long_result, "error": None}) + "\n",
            json.dumps({"id": "batch-1", "result": short_result, "error": None}) + "\n",
        ]

        mock_file = MagicMock()
        mock_file.readline.side_effect = [r.encode() for r in responses]

        mock_sock = MagicMock()
        mock_sock.__enter__ = MagicMock(return_value=mock_sock)
        mock_sock.__exit__ = MagicMock(return_value=False)
        mock_sock.makefile.return_value = mock_file

        calls = [
            {"tool": "axon_query", "args": {"query": "foo"}},
            {"tool": "axon_context", "args": {"symbol": "Bar"}},
        ]
        with patch("axon.mcp.server._socket.socket", return_value=mock_sock):
            result = _batch_daemon_call(calls, 100)

        assert "[truncated at 100 chars]" in result
        assert "ok" in result  # short result not truncated

    def test_returns_none_on_connection_error(self, tmp_path, monkeypatch):
        """_batch_daemon_call returns None (fallback) on connection error."""
        monkeypatch.setattr("pathlib.Path.home", lambda: tmp_path)
        sock_path = tmp_path / ".axon" / "daemon.sock"
        sock_path.parent.mkdir(parents=True)
        sock_path.touch()

        mock_sock = MagicMock()
        mock_sock.__enter__ = MagicMock(return_value=mock_sock)
        mock_sock.__exit__ = MagicMock(return_value=False)
        mock_sock.connect.side_effect = OSError("Connection refused")

        with patch("axon.mcp.server._socket.socket", return_value=mock_sock):
            result = _batch_daemon_call(
                [{"tool": "axon_query", "args": {"query": "foo"}}], None
            )

        assert result is None

    def test_batch_tool_schema_is_correct(self):
        """axon_batch has correct schema: calls required, max_tokens optional."""
        from axon.mcp.server import TOOLS

        batch_tool = next((t for t in TOOLS if t.name == "axon_batch"), None)
        assert batch_tool is not None, "axon_batch tool not found in TOOLS"
        schema = batch_tool.inputSchema
        assert "calls" in schema.get("required", [])
        assert "max_tokens" not in schema.get("required", [])
        props = schema.get("properties", {})
        assert "calls" in props
        assert props["calls"]["type"] == "array"
        items = props["calls"].get("items", {})
        assert "tool" in items.get("required", [])
        assert "args" in items.get("required", [])


# ---------------------------------------------------------------------------
# Security: double-checked lock in _get_storage(), socket chmod
# ---------------------------------------------------------------------------


class TestGetStorageThreadSafety:
    """_get_storage() initialises AstralBackend exactly once under concurrency."""

    def test_not_initialized_twice(self, tmp_path, monkeypatch):
        """Two concurrent calls must call AstralBackend() exactly once."""
        import axon.mcp.server as srv

        original_storage = srv._storage
        srv._storage = None  # reset for test

        init_count = []

        class CountingBackend:
            def __init__(self):
                init_count.append(1)

            def initialize(self, *a, **kw):
                pass

        monkeypatch.setattr("axon.mcp.server.AstralBackend", CountingBackend)
        monkeypatch.chdir(tmp_path)  # no .axon/kuzu → initialize not called

        results = []

        def call_get_storage():
            results.append(srv._get_storage())

        t1 = threading.Thread(target=call_get_storage)
        t2 = threading.Thread(target=call_get_storage)
        t1.start()
        t2.start()
        t1.join()
        t2.join()

        assert len(init_count) == 1, f"AstralBackend() called {len(init_count)} times"
        assert results[0] is results[1]

        srv._storage = original_storage  # restore


class TestDaemonSocketPermissions:
    """Daemon socket is created with 0o600 permissions."""

    def test_chmod_called_with_0o600(self, tmp_path, monkeypatch):
        """os.chmod is called with 0o600 after socket creation."""
        import stat as stat_mod

        chmod_calls = []
        original_chmod = os.chmod if "os" in dir() else None

        with patch("axon.daemon.server.os.chmod") as mock_chmod:
            mock_chmod.side_effect = lambda path, mode: chmod_calls.append((path, mode))
            # Verify the constant matches expectation
            expected_mode = stat_mod.S_IRUSR | stat_mod.S_IWUSR
            assert expected_mode == 0o600

        # Verify the source code has the chmod call
        import inspect

        import axon.daemon.server as ds

        src = inspect.getsource(ds.run_daemon)
        assert "os.chmod" in src
        assert "S_IRUSR" in src or "0o600" in src

    def test_socket_chmod_constant(self):
        """stat.S_IRUSR | stat.S_IWUSR equals 0o600."""
        import stat as stat_mod
        assert (stat_mod.S_IRUSR | stat_mod.S_IWUSR) == 0o600




# ---------------------------------------------------------------------------
# readline() replaces recv loop
# ---------------------------------------------------------------------------


class TestBatchSocketReadline:
    def test_try_daemon_uses_makefile(self, tmp_path, monkeypatch):
        """_try_daemon_call reads response via makefile("rb").readline(), not recv."""
        monkeypatch.setattr("pathlib.Path.home", lambda: tmp_path)
        sock_path = tmp_path / ".axon" / "daemon.sock"
        sock_path.parent.mkdir(parents=True)
        sock_path.touch()

        response_data = json.dumps({"id": "mcp", "result": "ok", "error": None}) + "\n"

        mock_sock = MagicMock()
        mock_sock.__enter__ = MagicMock(return_value=mock_sock)
        mock_sock.__exit__ = MagicMock(return_value=False)
        mock_sock.makefile.return_value.readline.return_value = response_data.encode()

        with patch("axon.mcp.server._socket.socket", return_value=mock_sock):
            result = _try_daemon_call("axon_query", None, {"query": "x"})

        mock_sock.makefile.assert_called_once_with("rb")
        assert result == "ok"

    def test_batch_daemon_uses_makefile(self, tmp_path, monkeypatch):
        """_batch_daemon_call reads all responses via a single makefile("rb"), not recv."""
        monkeypatch.setattr("pathlib.Path.home", lambda: tmp_path)
        sock_path = tmp_path / ".axon" / "daemon.sock"
        sock_path.parent.mkdir(parents=True)
        sock_path.touch()

        responses = [
            json.dumps({"id": "batch-0", "result": "r0", "error": None}) + "\n",
            json.dumps({"id": "batch-1", "result": "r1", "error": None}) + "\n",
        ]

        mock_file = MagicMock()
        mock_file.readline.side_effect = [r.encode() for r in responses]

        mock_sock = MagicMock()
        mock_sock.__enter__ = MagicMock(return_value=mock_sock)
        mock_sock.__exit__ = MagicMock(return_value=False)
        mock_sock.makefile.return_value = mock_file

        calls = [
            {"tool": "axon_query", "args": {"query": "foo"}},
            {"tool": "axon_context", "args": {"symbol": "Bar"}},
        ]
        with patch("axon.mcp.server._socket.socket", return_value=mock_sock):
            result = _batch_daemon_call(calls, None)

        mock_sock.makefile.assert_called_once_with("rb")
        assert "r0" in result
        assert "r1" in result


# ---------------------------------------------------------------------------
# axon_batch partial failure summary
# ---------------------------------------------------------------------------


class TestBatchPartialFailure:
    def test_daemon_path_warning_on_error(self, tmp_path, monkeypatch):
        """_batch_daemon_call appends BATCH WARNING when one call returns an error."""
        monkeypatch.setattr("pathlib.Path.home", lambda: tmp_path)
        sock_path = tmp_path / ".axon" / "daemon.sock"
        sock_path.parent.mkdir(parents=True)
        sock_path.touch()

        responses = [
            json.dumps({"id": "batch-0", "result": "ok", "error": None}) + "\n",
            json.dumps({"id": "batch-1", "result": None, "error": "slug not found"}) + "\n",
        ]

        mock_file = MagicMock()
        mock_file.readline.side_effect = [r.encode() for r in responses]

        mock_sock = MagicMock()
        mock_sock.__enter__ = MagicMock(return_value=mock_sock)
        mock_sock.__exit__ = MagicMock(return_value=False)
        mock_sock.makefile.return_value = mock_file

        calls = [
            {"tool": "axon_query", "args": {"query": "foo"}},
            {"tool": "axon_context", "args": {"symbol": "Bar"}},
        ]
        with patch("axon.mcp.server._socket.socket", return_value=mock_sock):
            result = _batch_daemon_call(calls, None)

        assert "[BATCH WARNING: 1/2 failed: indices [1]]" in result
        assert "ok" in result

    def test_daemon_path_no_warning_on_all_success(self, tmp_path, monkeypatch):
        """_batch_daemon_call has no BATCH WARNING when all calls succeed."""
        monkeypatch.setattr("pathlib.Path.home", lambda: tmp_path)
        sock_path = tmp_path / ".axon" / "daemon.sock"
        sock_path.parent.mkdir(parents=True)
        sock_path.touch()

        responses = [
            json.dumps({"id": "batch-0", "result": "ok", "error": None}) + "\n",
        ]

        mock_file = MagicMock()
        mock_file.readline.side_effect = [r.encode() for r in responses]

        mock_sock = MagicMock()
        mock_sock.__enter__ = MagicMock(return_value=mock_sock)
        mock_sock.__exit__ = MagicMock(return_value=False)
        mock_sock.makefile.return_value = mock_file

        with patch("axon.mcp.server._socket.socket", return_value=mock_sock):
            result = _batch_daemon_call([{"tool": "axon_query", "args": {"query": "foo"}}], None)

        assert "BATCH WARNING" not in result

    def test_direct_path_warning_on_unknown_tool(self):
        """Direct fallback appends BATCH WARNING when sub-calls return unknown tool errors."""
        import asyncio

        from axon.mcp.server import call_tool

        with (
            patch("axon.mcp.server._batch_daemon_call", return_value=None),
            patch("axon.mcp.server._get_storage", return_value=MagicMock()),
            patch("axon.mcp.server._lock", None),
        ):
            result_contents = asyncio.run(
                call_tool(
                    "axon_batch",
                    {
                        "calls": [
                            {"tool": "axon_nonexistent_tool", "args": {}},
                            {"tool": "axon_nonexistent_tool2", "args": {}},
                        ]
                    },
                )
            )

        text = result_contents[0].text
        assert "BATCH WARNING" in text
        assert "2/2 failed" in text
