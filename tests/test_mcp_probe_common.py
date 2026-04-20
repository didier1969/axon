import importlib.util
import json
import sys
import unittest
from pathlib import Path


MODULE_PATH = Path(__file__).resolve().parents[1] / "scripts" / "mcp_probe_common.py"
SPEC = importlib.util.spec_from_file_location("mcp_probe_common", MODULE_PATH)
MODULE = importlib.util.module_from_spec(SPEC)
assert SPEC is not None and SPEC.loader is not None
sys.modules[SPEC.name] = MODULE
SPEC.loader.exec_module(MODULE)


class _FakeResponse:
    def __init__(self, body: str, headers: dict[str, str] | None = None) -> None:
        self._body = body.encode("utf-8")
        self.headers = headers or {}

    def read(self) -> bytes:
        return self._body

    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc, tb) -> None:
        return None


class McpProbeCommonTests(unittest.TestCase):
    def test_rpc_call_rejects_empty_body_by_default(self) -> None:
        original_urlopen = MODULE.urllib.request.urlopen
        try:
            MODULE.urllib.request.urlopen = lambda req, timeout=0: _FakeResponse("")
            with self.assertRaisesRegex(ValueError, "empty MCP response body"):
                MODULE.rpc_call("http://example.test/mcp", {"jsonrpc": "2.0", "id": 1}, 1)
        finally:
            MODULE.urllib.request.urlopen = original_urlopen

    def test_rpc_call_allows_empty_body_when_explicit(self) -> None:
        original_urlopen = MODULE.urllib.request.urlopen
        try:
            MODULE.urllib.request.urlopen = lambda req, timeout=0: _FakeResponse("")
            _, response = MODULE.rpc_call(
                "http://example.test/mcp",
                {"jsonrpc": "2.0", "method": "notifications/initialized"},
                1,
                allow_empty_body=True,
            )
        finally:
            MODULE.urllib.request.urlopen = original_urlopen

        self.assertIsNone(response)

    def test_initialize_session_tolerates_empty_initialized_notification(self) -> None:
        original_rpc_call = MODULE.rpc_call
        calls = []

        def fake_rpc_call(url, payload, timeout, allow_empty_body=False):
            calls.append(
                {
                    "url": url,
                    "payload": payload,
                    "timeout": timeout,
                    "allow_empty_body": allow_empty_body,
                }
            )
            if payload["method"] == "initialize":
                return 1.0, {"jsonrpc": "2.0", "result": {"protocolVersion": "2025-11-25"}}
            return 0.5, None

        try:
            MODULE.rpc_call = fake_rpc_call
            MODULE.initialize_session("http://example.test/mcp", 5, "measure_test")
        finally:
            MODULE.rpc_call = original_rpc_call

        self.assertEqual(len(calls), 2)
        self.assertEqual(calls[0]["payload"]["method"], "initialize")
        self.assertEqual(calls[1]["payload"]["method"], "notifications/initialized")
        self.assertTrue(calls[1]["allow_empty_body"])

    def test_rpc_call_stores_negotiated_protocol_on_initialize(self) -> None:
        original_urlopen = MODULE.urllib.request.urlopen
        MODULE.NEGOTIATED_PROTOCOL_BY_URL.clear()
        try:
            MODULE.urllib.request.urlopen = lambda req, timeout=0: _FakeResponse(
                json.dumps(
                    {
                        "jsonrpc": "2.0",
                        "result": {"protocolVersion": "2025-11-25"},
                    }
                ),
                headers={"MCP-Protocol-Version": "2025-11-25"},
            )
            MODULE.rpc_call(
                "http://example.test/mcp",
                {"jsonrpc": "2.0", "id": 1, "method": "initialize"},
                1,
            )
        finally:
            MODULE.urllib.request.urlopen = original_urlopen

        self.assertEqual(
            MODULE.NEGOTIATED_PROTOCOL_BY_URL["http://example.test/mcp"],
            "2025-11-25",
        )

    def test_rpc_call_sends_negotiated_protocol_header_after_initialize(self) -> None:
        original_urlopen = MODULE.urllib.request.urlopen
        MODULE.NEGOTIATED_PROTOCOL_BY_URL.clear()
        MODULE.NEGOTIATED_PROTOCOL_BY_URL["http://example.test/mcp"] = "2025-11-25"
        seen_headers = {}

        def fake_urlopen(req, timeout=0):
            nonlocal seen_headers
            seen_headers = dict(req.header_items())
            return _FakeResponse(json.dumps({"jsonrpc": "2.0", "result": {}}))

        try:
            MODULE.urllib.request.urlopen = fake_urlopen
            MODULE.rpc_call(
                "http://example.test/mcp",
                {"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}},
                1,
            )
        finally:
            MODULE.urllib.request.urlopen = original_urlopen

        self.assertEqual(seen_headers.get("Mcp-protocol-version"), "2025-11-25")


if __name__ == "__main__":
    unittest.main()
