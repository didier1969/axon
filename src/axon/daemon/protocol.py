"""JSON-line protocol for axon daemon IPC.

Request:  {"id": "<str>", "tool": "<tool_name>", "slug": "<slug>|null", "args": {...}}\n
Response: {"id": "<str>", "result": "<str>|null", "error": "<str>|null"}\n

Special tool: "axon_daemon_status" → returns cache status (no slug needed).
"""
from __future__ import annotations

import json


def encode_request(tool: str, args: dict, slug: str | None = None, request_id: str = "") -> bytes:
    """Encode a tool request to bytes."""
    payload = json.dumps({"id": request_id, "tool": tool, "slug": slug, "args": args})
    return (payload + "\n").encode()


def decode_request(line: bytes) -> dict:
    """Decode a request line. Raises json.JSONDecodeError on bad input."""
    return json.loads(line)


def encode_response(result: str | None, error: str | None, request_id: str = "") -> bytes:
    """Encode a tool response to bytes."""
    return (json.dumps({"id": request_id, "result": result, "error": error}) + "\n").encode()
