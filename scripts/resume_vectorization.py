#!/usr/bin/env python3
"""Thin CLI wrapper for the MCP `resume_vectorization` tool."""

from __future__ import annotations

import argparse
import json
import sys
import urllib.request
from typing import Any


DEFAULT_MCP_URL = "http://127.0.0.1:44129/mcp"


def rpc_call(url: str, tool_name: str, arguments: dict[str, Any], timeout: int) -> dict[str, Any]:
    payload = {
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {"name": tool_name, "arguments": arguments},
    }
    data = json.dumps(payload).encode("utf-8")
    req = urllib.request.Request(
        url,
        data=data,
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(req, timeout=timeout) as resp:
        return json.loads(resp.read().decode("utf-8"))


def extract_text(resp: dict[str, Any]) -> str:
    if resp.get("error") is not None:
        return json.dumps(resp["error"], ensure_ascii=False)
    result = resp.get("result", {})
    if not isinstance(result, dict):
        return ""
    content = result.get("content")
    if not isinstance(content, list):
        return ""
    chunks: list[str] = []
    for item in content:
        if isinstance(item, dict) and isinstance(item.get("text"), str):
            chunks.append(item["text"])
    return "\n".join(chunks).strip()


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Requeue missing chunk vectorization from graph-ready files."
    )
    parser.add_argument("--json", action="store_true", help="Print JSON data payload instead of text")
    parser.add_argument("--url", default=DEFAULT_MCP_URL, help=f"MCP endpoint (default: {DEFAULT_MCP_URL})")
    parser.add_argument("--timeout", type=int, default=60, help="RPC timeout in seconds")
    args = parser.parse_args()

    try:
        resp = rpc_call(args.url, "resume_vectorization", {}, timeout=args.timeout)
    except Exception as exc:
        print(f"RPC error: {exc}", file=sys.stderr)
        return 2

    if resp.get("error") is not None:
        print(json.dumps(resp["error"], ensure_ascii=False), file=sys.stderr)
        return 2

    result = resp.get("result", {})
    if not isinstance(result, dict):
        print("Invalid MCP response: missing result object", file=sys.stderr)
        return 2
    if result.get("isError"):
        print(extract_text(resp), file=sys.stderr)
        return 2

    data = result.get("data", {})
    if args.json:
        print(json.dumps(data, indent=2, ensure_ascii=False))
    else:
        print(extract_text(resp))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
