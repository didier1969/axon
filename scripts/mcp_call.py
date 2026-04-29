#!/usr/bin/env python3
"""Minimal direct MCP caller for local Axon runtimes."""

from __future__ import annotations

import argparse
import json
import os
import sys
from typing import Any

from mcp_probe_common import (
    DEFAULT_URL,
    call_tool,
    initialize_session,
    response_data,
    response_text,
    rpc_call,
)


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Call the local Axon MCP server directly through its HTTP JSON-RPC surface."
    )
    subparsers = parser.add_subparsers(dest="command", required=True)

    list_parser = subparsers.add_parser("list", help="List MCP tools")
    list_parser.add_argument("--url", default=os.environ.get("AXON_MCP_URL", DEFAULT_URL))
    list_parser.add_argument("--timeout", type=int, default=10)
    list_parser.add_argument(
        "--format",
        choices=["json", "names"],
        default="names",
        help="Output format. Default: names",
    )

    call_parser = subparsers.add_parser("call", help="Call one MCP tool")
    call_parser.add_argument("tool", help="MCP tool name")
    call_parser.add_argument(
        "--args",
        default="{}",
        help="JSON object of tool arguments. Default: {}",
    )
    call_parser.add_argument(
        "--args-file",
        help="Path to a JSON file containing the tool arguments. Use '-' to read JSON from stdin. Overrides --args.",
    )
    call_parser.add_argument("--url", default=os.environ.get("AXON_MCP_URL", DEFAULT_URL))
    call_parser.add_argument("--timeout", type=int, default=15)
    call_parser.add_argument(
        "--format",
        choices=["json", "data", "text"],
        default="json",
        help="How to render the MCP result. Default: json",
    )

    return parser.parse_args(argv)


def parse_json_object(raw: str, source: str = "--args") -> dict[str, Any]:
    try:
        value = json.loads(raw)
    except json.JSONDecodeError as exc:
        raise SystemExit(f"{source} must be valid JSON: {exc}") from exc
    if not isinstance(value, dict):
        raise SystemExit(f"{source} must decode to a JSON object")
    return value


def load_tool_args(args: argparse.Namespace) -> dict[str, Any]:
    if not args.args_file:
        return parse_json_object(args.args)
    if args.args_file == "-":
        return parse_json_object(sys.stdin.read(), "--args-file -")
    try:
        with open(args.args_file, "r", encoding="utf-8") as handle:
            return parse_json_object(handle.read(), f"--args-file {args.args_file}")
    except OSError as exc:
        raise SystemExit(f"Cannot read --args-file {args.args_file}: {exc}") from exc


def initialize(url: str, timeout: int) -> None:
    initialize_session(url, timeout, "axon-mcp-call")


def run_list(url: str, timeout: int, output_format: str) -> int:
    initialize(url, timeout)
    _, response = rpc_call(
        url,
        {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/list",
            "params": {},
        },
        timeout,
    )
    if output_format == "json":
        print(json.dumps(response, indent=2, ensure_ascii=False))
        return 0
    tools = response.get("result", {}).get("tools", [])
    if not isinstance(tools, list):
        raise SystemExit("tools/list did not return a tools array")
    for tool in tools:
        name = tool.get("name")
        if isinstance(name, str):
            print(name)
    return 0


def run_call(url: str, timeout: int, tool: str, args: dict[str, Any], output_format: str) -> int:
    initialize(url, timeout)
    _, response = call_tool(url, timeout, tool, args)
    if output_format == "json":
        print(json.dumps(response, indent=2, ensure_ascii=False))
        return 0
    if output_format == "data":
        print(json.dumps(response_data(response), indent=2, ensure_ascii=False))
        return 0
    text = response_text(response)
    if text:
        print(text)
    else:
        print(json.dumps(response_data(response), indent=2, ensure_ascii=False))
    return 0


def main(argv: list[str]) -> int:
    args = parse_args(argv)
    if args.command == "list":
        return run_list(args.url, args.timeout, args.format)
    if args.command == "call":
        tool_args = load_tool_args(args)
        return run_call(args.url, args.timeout, args.tool, tool_args, args.format)
    raise SystemExit(f"Unsupported command: {args.command}")


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
