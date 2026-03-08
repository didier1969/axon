"""MCP server for Axon v1.2 — High-precision consolidated API."""

from __future__ import annotations

import asyncio
import json
import logging
import socket as _socket
import os
_DAEMON_TIMEOUT = float(os.environ.get("AXON_TIMEOUT", "30.0"))
import threading
from pathlib import Path

from mcp.server import Server
from mcp.server.stdio import stdio_server
from mcp.types import Resource, TextContent, Tool

from axon.core.paths import central_db_path, daemon_sock_path
from axon.core.storage.astral_backend import AstralBackend
from axon.daemon.protocol import decode_request, encode_request
from axon.mcp.resources import get_dead_code_list, get_overview, get_schema
from axon.mcp.tools import (
    handle_query,
    handle_inspect,
    handle_audit,
    handle_impact,
    handle_health,
    handle_diff,
    handle_batch_call,
    handle_cypher,
    _load_repo_storage,
    _get_local_slug,
)

logger = logging.getLogger(__name__)

server = Server("axon")

def create_mcp_server() -> Server:
    """Factory function to create and configure the Axon MCP server."""
    return server

_storage: AstralBackend | None = None
_storage_lock = threading.Lock()
_lock: asyncio.Lock | None = None

def set_storage(storage: AstralBackend) -> None:
    """Inject a pre-initialised storage backend."""
    global _storage  # noqa: PLW0603
    _storage = storage

def _get_storage() -> AstralBackend:
    """Lazily initialise the storage backend."""
    global _storage  # noqa: PLW0603
    if _storage is not None:
        return _storage
    with _storage_lock:
        if _storage is not None: return _storage
        _storage = AstralBackend()
        slug = _get_local_slug()
        if slug:
            db_path = central_db_path(slug)
            _storage.initialize(db_path, read_only=True)
    return _storage

def _try_daemon_call(tool: str, slug: str | None, args: dict) -> str | None:
    """Send a tool call to the daemon via Unix socket."""
    sock_path = daemon_sock_path()
    if not sock_path.exists(): return None
    try:
        with _socket.socket(_socket.AF_UNIX, _socket.SOCK_STREAM) as sock:
            sock.settimeout(_DAEMON_TIMEOUT)
            sock.connect(str(sock_path))
            sock.sendall(encode_request(tool, args, slug=slug, request_id="mcp"))
            data = sock.makefile("rb").readline()
        resp = decode_request(data)
        return resp.get("result", "") if not resp.get("error") else None
    except Exception:
        return None

TOOLS: list[Tool] = [
    Tool(
        name="axon_query",
        description="Search for code using hybrid search (text + vectors) or similarity.",
        inputSchema={
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "Search query or symbol name."},
                "limit": {"type": "integer", "default": 20},
                "repo": {"type": "string", "description": "Repository slug."},
                "mode": {"type": "string", "enum": ["hybrid", "similar"], "default": "hybrid"}
            },
            "required": ["query"],
        },
    ),
    Tool(
        name="axon_inspect",
        description="Vue 360° of a symbol: source code, context (callers/callees), and usages.",
        inputSchema={
            "type": "object",
            "properties": {
                "symbol": {"type": "string", "description": "Symbol name or path/to/file.py:name."},
                "repo": {"type": "string"},
                "include_usages": {"type": "boolean", "default": False}
            },
            "required": ["symbol"],
        },
    ),
    Tool(
        name="axon_audit",
        description="Run security (OWASP) or quality audits on the codebase.",
        inputSchema={
            "type": "object",
            "properties": {
                "repo": {"type": "string"},
                "mode": {"type": "string", "enum": ["security", "quality", "full"], "default": "security"}
            },
        },
    ),
    Tool(
        name="axon_impact",
        description="Calculate blast radius and critical paths between symbols.",
        inputSchema={
            "type": "object",
            "properties": {
                "symbol": {"type": "string"},
                "target": {"type": "string", "description": "Optional: find path to this target."},
                "depth": {"type": "integer", "default": 3},
                "repo": {"type": "string"}
            },
            "required": ["symbol"],
        },
    ),
    Tool(
        name="axon_health",
        description="Global health report: dead code, test gaps, and entry points.",
        inputSchema={
            "type": "object",
            "properties": {
                "repo": {"type": "string"},
                "filter": {"type": "string", "enum": ["all", "dead_code", "tests", "entries"], "default": "all"}
            },
        },
    ),
    Tool(
        name="axon_diff",
        description="Analyze semantic changes between branches or from git diff.",
        inputSchema={
            "type": "object",
            "properties": {
                "repo": {"type": "string"},
                "branch_range": {"type": "string", "description": "e.g. 'main..feature'"},
                "raw_diff": {"type": "string", "description": "Raw git diff output."}
            },
            "required": ["repo"],
        },
    ),
    Tool(
        name="axon_batch",
        description="Execute multiple tools in a single request for performance.",
        inputSchema={
            "type": "object",
            "properties": {
                "calls": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "tool": {"type": "string"},
                            "args": {"type": "object"}
                        },
                        "required": ["tool", "args"]
                    }
                }
            },
            "required": ["calls"],
        },
    ),
    Tool(
        name="axon_cypher",
        description="Expert access: run raw Cypher/Datalog queries on HydraDB.",
        inputSchema={
            "type": "object",
            "properties": {
                "query": {"type": "string"},
                "repo": {"type": "string"}
            },
            "required": ["query"],
        },
    ),
]

def _dispatch_tool(name: str, arguments: dict, storage: AstralBackend) -> str:
    """Consolidated tool dispatch logic."""
    if name == "axon_query":
        return handle_query(storage, arguments.get("query", ""), limit=arguments.get("limit", 20), repo=arguments.get("repo"))
    elif name == "axon_inspect":
        return handle_inspect(storage, arguments.get("symbol", ""), repo=arguments.get("repo"), include_usages=arguments.get("include_usages", False))
    elif name == "axon_audit":
        return handle_audit(storage, repo=arguments.get("repo"), check_type=arguments.get("mode", "security"))
    elif name == "axon_impact":
        return handle_impact(storage, arguments.get("symbol", ""), depth=arguments.get("depth", 3), repo=arguments.get("repo"))
    elif name == "axon_health":
        return handle_health(storage, repo=arguments.get("repo"), filter_type=arguments.get("filter", "all"))
    elif name == "axon_diff":
        return handle_diff(storage, repo=arguments.get("repo"), branch_range=arguments.get("branch_range"))
    elif name == "axon_batch":
        return handle_batch_call(arguments.get("calls", []))
    elif name == "axon_cypher":
        return handle_cypher(storage, arguments.get("query", ""))
    return f"Unknown tool: {name}"

@server.list_tools()
async def list_tools() -> list[Tool]:
    return TOOLS

@server.call_tool()
async def call_tool(name: str, arguments: dict) -> list[TextContent]:
    slug = arguments.get("repo") or _get_local_slug()
    result = await asyncio.to_thread(_try_daemon_call, name, slug, arguments)
    if result is None:
        storage = _get_storage()
        if _lock:
            async with _lock:
                result = await asyncio.to_thread(_dispatch_tool, name, arguments, storage)
        else:
            result = _dispatch_tool(name, arguments, storage)
    return [TextContent(type="text", text=result)]

async def main() -> None:
    async with stdio_server() as (read, write):
        await server.run(read, write, server.create_initialization_options())

if __name__ == "__main__":
    asyncio.run(main())
