"""MCP server for Axon v1.3 — High-performance, async-native architecture."""

from __future__ import annotations

import asyncio
import json
import logging
import socket as _socket
import os
import threading
from pathlib import Path
from typing import Any, List, Optional

from mcp.server import Server, NotificationOptions
from mcp.server.stdio import stdio_server
from mcp.types import (
    Resource,
    TextContent,
    Tool,
    CallToolRequest,
    CallToolResult,
    LoggingLevel,
)

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

# Standardize timeout
_DAEMON_TIMEOUT = float(os.environ.get("AXON_TIMEOUT", "30.0"))

# Configure specialized logger for MCP
logger = logging.getLogger("axon.mcp")

# Initialize Server with full capabilities
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

def set_lock(lock: asyncio.Lock) -> None:
    """Inject a shared lock for coordinating access."""
    global _lock  # noqa: PLW0603
    _lock = lock

async def _get_async_storage() -> AstralBackend:
    """Thread-safe async retrieval of the storage backend."""
    global _storage  # noqa: PLW0603
    if _storage is not None:
        return _storage
    
    # We use to_thread for the heavy initialization
    return await asyncio.to_thread(_get_storage)

def _get_storage() -> AstralBackend:
    """Legacy sync retrieval for daemon compatibility."""
    global _storage  # noqa: PLW0603
    if _storage is not None:
        return _storage
    with _storage_lock:
        if _storage is not None: return _storage
        _storage = AstralBackend()
        slug = _get_local_slug()
        if slug:
            db_path = central_db_path(slug)
            try:
                _storage.initialize(db_path, read_only=True)
            except Exception as e:
                logger.error(f"Failed to initialize storage for {slug}: {e}")
    return _storage

async def _try_daemon_call(tool: str, slug: str | None, args: dict) -> str | None:
    """Asynchronous tool call to the Axon daemon."""
    sock_path = daemon_sock_path()
    if not sock_path.exists():
        return None
    
    try:
        reader, writer = await asyncio.open_unix_connection(str(sock_path))
        writer.write(encode_request(tool, args, slug=slug, request_id="mcp"))
        await writer.drain()
        
        data = await reader.readline()
        writer.close()
        await writer.wait_closed()
        
        resp = decode_request(data)
        if resp.get("error"):
            logger.debug(f"Daemon error for tool {tool}: {resp['error']}")
            return None
        return resp.get("result", "")
    except Exception as exc:
        logger.debug(f"Daemon call failed for {tool}, falling back: {exc}")
        return None

# Definitive Tool List for v1.3
TOOLS: List[Tool] = [
    Tool(
        name="axon_query",
        description="High-performance hybrid search (text + vector) or semantic similarity.",
        inputSchema={
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "Concept, feature name, or symbol."},
                "limit": {"type": "integer", "default": 20, "description": "Max results."},
                "repo": {"type": "string", "description": "Target repository slug."},
                "mode": {"type": "string", "enum": ["hybrid", "similar"], "default": "hybrid"}
            },
            "required": ["query"],
        },
    ),
    Tool(
        name="axon_inspect",
        description="Vue 360° of a code symbol: full source, architectural context (callers/callees), and stats.",
        inputSchema={
            "type": "object",
            "properties": {
                "symbol": {"type": "string", "description": "Symbol name or 'path/to/file.py:name'."},
                "repo": {"type": "string"},
                "include_usages": {"type": "boolean", "default": False, "description": "Fetch exhaustive call sites."}
            },
            "required": ["symbol"],
        },
    ),
    Tool(
        name="axon_audit",
        description="Architectural security (OWASP) or quality audit on the codebase.",
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
        description="Blast radius analysis and critical path discovery between symbols.",
        inputSchema={
            "type": "object",
            "properties": {
                "symbol": {"type": "string", "description": "Starting point symbol."},
                "target": {"type": "string", "description": "Optional destination to find path to."},
                "depth": {"type": "integer", "default": 3, "maximum": 10},
                "repo": {"type": "string"}
            },
            "required": ["symbol"],
        },
    ),
    Tool(
        name="axon_health",
        description="Global repository health: dead code, coverage gaps, and entry points.",
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
        description="Semantic analysis of changes between branches or from raw git diff.",
        inputSchema={
            "type": "object",
            "properties": {
                "repo": {"type": "string"},
                "branch_range": {"type": "string", "description": "e.g., 'main..feature'."},
                "raw_diff": {"type": "string", "description": "Raw 'git diff HEAD' output."}
            },
            "required": ["repo"],
        },
    ),
    Tool(
        name="axon_batch",
        description="Performance booster: execute multiple Axon tools in a single round-trip.",
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
        description="Expert access: direct Datalog/Cypher query execution on HydraDB.",
        inputSchema={
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "Datalog logic or Cypher shim."},
                "repo": {"type": "string"}
            },
            "required": ["query"],
        },
    ),
]

@server.list_tools()
async def list_tools() -> list[Tool]:
    """Register consolidated tools."""
    return TOOLS

@server.call_tool()
async def call_tool(name: str, arguments: dict) -> list[TextContent]:
    """Modern tool dispatcher with robust error reporting and daemon support."""
    slug = arguments.get("repo") or _get_local_slug()
    
    # 1. Try Daemon First (Fastest path)
    daemon_result = await _try_daemon_call(name, slug, arguments)
    if daemon_result is not None:
        return [TextContent(type="text", text=daemon_result)]
    
    # 2. Fallback to In-Process Execution
    try:
        storage = await _get_async_storage()
        
        # Use centralized lock if available (to sync with watcher)
        if _lock:
            async with _lock:
                result = await asyncio.to_thread(_dispatch_tool, name, arguments, storage)
        else:
            result = await asyncio.to_thread(_dispatch_tool, name, arguments, storage)
            
        return [TextContent(type="text", text=result)]
    
    except Exception as e:
        logger.exception(f"Error executing tool {name}")
        return [TextContent(type="text", text=f"Error: Internal system failure while executing {name}. Details: {e}")]

def _dispatch_tool(name: str, arguments: dict, storage: AstralBackend) -> str:
    """Core tool logic dispatcher (Sync wrapper)."""
    # This remains sync to allow easy wrapping in to_thread and daemon compatibility
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

@server.list_resources()
async def list_resources() -> list[Resource]:
    """Expose architectural insights as resources."""
    return [
        Resource(
            uri="axon://overview",
            name="Ecosystem Overview",
            description="High-level statistics about the Nexus projects.",
            mimeType="text/plain",
        ),
        Resource(
            uri="axon://schema",
            name="Knowledge Graph Schema",
            description="Datalog rules and node/edge types documentation.",
            mimeType="text/plain",
        ),
    ]

@server.read_resource()
async def read_resource(uri) -> str:
    """Read resource content."""
    storage = await _get_async_storage()
    uri_str = str(uri)
    
    if uri_str == "axon://overview":
        return await asyncio.to_thread(get_overview, storage)
    if uri_str == "axon://schema":
        return await asyncio.to_thread(get_schema)
    
    return f"Unknown resource: {uri_str}"

async def main() -> None:
    """Run the Axon MCP v1.3 server."""
    logger.info("Axon MCP Server v1.3 starting...")
    async with stdio_server() as (read, write):
        await server.run(read, write, server.create_initialization_options(
            notification_options=NotificationOptions(
                resources_changed=True,
                tools_changed=True
            )
        ))

if __name__ == "__main__":
    logging.basicConfig(level=logging.INFO)
    asyncio.run(main())
