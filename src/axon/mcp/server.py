"""MCP server for Axon — exposes code intelligence tools over stdio transport.

Registers seven tools and three resources that give AI agents and MCP clients
access to the Axon knowledge graph.  The server lazily initialises a
:class:`KuzuBackend` from the ``.axon/kuzu`` directory in the current
working directory.

Usage::

    # MCP server only
    axon mcp

    # MCP server with live file watching (recommended)
    axon serve --watch
"""

from __future__ import annotations

import asyncio
import json
import logging
import socket as _socket
import threading
from pathlib import Path

from mcp.server import Server
from mcp.server.stdio import stdio_server
from mcp.types import Resource, TextContent, Tool

from axon.core.paths import central_db_path, daemon_sock_path
from axon.core.storage.kuzu_backend import KuzuBackend
from axon.daemon.protocol import decode_request, encode_request
from axon.mcp.resources import get_dead_code_list, get_overview, get_schema
from axon.mcp.tools import (
    MAX_TRAVERSE_DEPTH,
    handle_context,
    handle_cypher,
    handle_dead_code,
    handle_detect_changes,
    handle_find_similar,
    handle_impact,
    handle_list_repos,
    handle_query,
    handle_read_symbol,
)

logger = logging.getLogger(__name__)

server = Server("axon")

_storage: KuzuBackend | None = None
_storage_lock = threading.Lock()
_lock: asyncio.Lock | None = None


def set_storage(storage: KuzuBackend) -> None:
    """Inject a pre-initialised storage backend (e.g. from ``axon serve --watch``)."""
    global _storage  # noqa: PLW0603
    _storage = storage


def set_lock(lock: asyncio.Lock) -> None:
    """Inject a shared lock for coordinating storage access with the file watcher."""
    global _lock  # noqa: PLW0603
    _lock = lock


def _get_storage() -> KuzuBackend:
    """Lazily initialise and return the KuzuDB storage backend (thread-safe).

    Tries the centralised path (``~/.axon/repos/{slug}/kuzu``) from the
    local ``.axon/meta.json`` slug field.  Falls back to the legacy
    ``.axon/kuzu`` path for repos indexed before v0.6.
    """
    global _storage  # noqa: PLW0603
    if _storage is not None:
        return _storage
    with _storage_lock:
        if _storage is not None:  # double-checked
            return _storage
        _storage = KuzuBackend()
        meta_path = Path.cwd() / ".axon" / "meta.json"
        db_path: Path | None = None
        if meta_path.exists():
            try:
                meta = json.loads(meta_path.read_text(encoding="utf-8"))
                slug = meta.get("slug")
                if slug:
                    db_path = central_db_path(slug)
            except (json.JSONDecodeError, OSError):
                pass
        if db_path is None:
            db_path = Path.cwd() / ".axon" / "kuzu"
        if db_path.exists():
            _storage.initialize(db_path, read_only=True)
            logger.info("Initialised storage (read-only) from %s", db_path)
        else:
            logger.warning("No axon DB found for %s", Path.cwd())
    return _storage


def _get_local_slug() -> str | None:
    """Read the repo slug from .axon/meta.json in the current working directory."""
    meta_path = Path.cwd() / ".axon" / "meta.json"
    try:
        meta = json.loads(meta_path.read_text(encoding="utf-8"))
        return meta.get("slug")
    except (OSError, json.JSONDecodeError):
        return None


def _try_daemon_call(tool: str, slug: str | None, args: dict) -> str | None:
    """Send a tool call to the daemon via Unix socket.

    Returns the result string on success, or None if the daemon is unavailable
    or returns an error (caller should fall back to direct dispatch).
    """
    sock_path = daemon_sock_path()
    if not sock_path.exists():
        return None
    try:
        with _socket.socket(_socket.AF_UNIX, _socket.SOCK_STREAM) as sock:
            sock.settimeout(5.0)
            sock.connect(str(sock_path))
            sock.sendall(encode_request(tool, args, slug=slug, request_id="mcp"))
            data = sock.makefile("rb").readline()
        resp = decode_request(data)
        if resp.get("error"):
            logger.debug("Daemon error for tool %s: %s", tool, resp["error"])
            return None
        return resp.get("result", "")
    except (OSError, json.JSONDecodeError) as exc:
        logger.debug("Daemon unavailable for %s, falling back to direct: %s", tool, exc)
        return None


def _batch_daemon_call(calls: list[dict], max_tokens: int | None) -> str | None:
    """Execute multiple tool calls on a single daemon socket connection.

    Sends requests sequentially (send → recv per call) on one socket.
    Returns formatted result string, or None if daemon unavailable.
    """
    if not calls:
        return ""
    sock_path = daemon_sock_path()
    if not sock_path.exists():
        return None
    try:
        parts: list[str] = []
        failed_indices: list[int] = []
        with _socket.socket(_socket.AF_UNIX, _socket.SOCK_STREAM) as sock:
            sock.settimeout(5.0)
            sock.connect(str(sock_path))
            total = len(calls)
            f = sock.makefile("rb")
            for i, call in enumerate(calls):
                tool = call.get("tool", "")
                args = call.get("args", {})
                slug: str | None = args.get("repo") or None
                daemon_args = {k: v for k, v in args.items() if k != "repo"}
                req_id = f"batch-{i}"
                sock.sendall(encode_request(tool, daemon_args, slug=slug, request_id=req_id))
                data = f.readline()
                resp = decode_request(data)
                if resp.get("error"):
                    result = f"Error: {resp['error']}"
                    failed_indices.append(i)
                else:
                    result = resp.get("result", "")
                if max_tokens is not None and len(result) > max_tokens:
                    result = result[:max_tokens] + f"\n[truncated at {max_tokens} chars]"
                parts.append(f"### {tool} ({i + 1}/{total})\n{result}")
        if failed_indices:
            parts.append(
                f"[BATCH WARNING: {len(failed_indices)}/{total} failed: indices {failed_indices}]"
            )
        return "\n\n".join(parts)
    except (OSError, json.JSONDecodeError) as exc:
        logger.debug("Batch daemon call failed, falling back: %s", exc)
        return None


TOOLS: list[Tool] = [
    Tool(
        name="axon_list_repos",
        description=(
            "List all indexed repositories with their stats. "
            "Use first to discover which repos are available before querying. "
            "Returns name, path, file count, symbol count, and relationship count per repo."
        ),
        inputSchema={
            "type": "object",
            "properties": {
                "max_tokens": {
                    "type": "integer",
                    "description": "Truncate output to this many characters. Omit for full output.",
                },
            },
        },
    ),
    Tool(
        name="axon_query",
        description=(
            "Search the knowledge graph by natural language or symbol name using hybrid "
            "(keyword + vector) search. "
            "Use when you need to find relevant functions, classes, or files by concept or name. "
            "Returns ranked symbols with file path, label, and a code snippet per result. "
            "Optionally filter by language or specify a repo to search "
            "a different indexed project. "
            "Follow with axon_context on a specific result for its full dependency graph."
        ),
        inputSchema={
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query — natural language or symbol name.",
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of results (default 20).",
                    "default": 20,
                },
                "language": {
                    "type": "string",
                    "description": (
                        "Filter results to a specific language "
                        "(e.g. 'python', 'elixir', 'typescript'). Optional."
                    ),
                },
                "repo": {
                    "type": "string",
                    "description": (
                        "Name of an indexed repository to query (from axon_list_repos). "
                        "Defaults to the current directory. Optional."
                    ),
                },
                "max_tokens": {
                    "type": "integer",
                    "description": "Truncate output to this many characters. Omit for full output.",
                },
            },
            "required": ["query"],
        },
    ),
    Tool(
        name="axon_context",
        description=(
            "Get a 360-degree view of a symbol: callers, callees, and type references. "
            "Use before modifying a symbol to understand its full dependency graph. "
            "Returns callers, callees, type refs, signature, file location, and dead-code status. "
            "To disambiguate symbols with the same name across files, "
            "use 'path/to/file.py:symbol_name' format. "
            "Optionally specify a repo to look up a symbol in a different indexed project. "
            "Follow with axon_impact to assess blast radius before making changes."
        ),
        inputSchema={
            "type": "object",
            "properties": {
                "symbol": {
                    "type": "string",
                    "description": (
                        "Name of the symbol to look up. "
                        "Use 'file/path.py:symbol_name' to target a specific file."
                    ),
                },
                "repo": {
                    "type": "string",
                    "description": (
                        "Name of an indexed repository to query (from axon_list_repos). "
                        "Defaults to the current directory. Optional."
                    ),
                },
                "max_tokens": {
                    "type": "integer",
                    "description": "Truncate output to this many characters. Omit for full output.",
                },
            },
            "required": ["symbol"],
        },
    ),
    Tool(
        name="axon_impact",
        description=(
            "Blast radius analysis — find all symbols affected by changing a given symbol, "
            "grouped by hop depth. "
            "Use before refactoring to understand risk and scope of changes. "
            "Returns affected symbols per depth level with confidence scores for direct callers. "
            "Optionally specify a repo to analyse a symbol in a different indexed project."
        ),
        inputSchema={
            "type": "object",
            "properties": {
                "symbol": {
                    "type": "string",
                    "description": "Name of the symbol to analyse.",
                },
                "depth": {
                    "type": "integer",
                    "description": f"Maximum traversal depth (default 3, max {MAX_TRAVERSE_DEPTH}).",  # noqa: E501
                    "default": 3,
                    "minimum": 1,
                    "maximum": MAX_TRAVERSE_DEPTH,
                },
                "repo": {
                    "type": "string",
                    "description": (
                        "Name of an indexed repository to query (from axon_list_repos). "
                        "Defaults to the current directory. Optional."
                    ),
                },
                "max_tokens": {
                    "type": "integer",
                    "description": "Truncate output to this many characters. Omit for full output.",
                },
            },
            "required": ["symbol"],
        },
    ),
    Tool(
        name="axon_dead_code",
        description=(
            "List all symbols detected as dead (unreachable) code. "
            "Use during code review or cleanup to identify safe deletions. "
            "Returns symbols grouped by file with line numbers."
        ),
        inputSchema={
            "type": "object",
            "properties": {
                "max_tokens": {
                    "type": "integer",
                    "description": "Truncate output to this many characters. Omit for full output.",
                },
            },
        },
    ),
    Tool(
        name="axon_detect_changes",
        description=(
            "Map a git diff to the symbols it touches. "
            "Pass raw `git diff HEAD` output to identify which indexed symbols "
            "are affected by a changeset. "
            "Use to understand scope before reviewing or testing a PR. "
            "Returns affected symbols per file. "
            "Follow with axon_impact on each affected symbol to see downstream effects."
        ),
        inputSchema={
            "type": "object",
            "properties": {
                "diff": {
                    "type": "string",
                    "description": "Raw git diff output.",
                },
                "max_tokens": {
                    "type": "integer",
                    "description": "Truncate output to this many characters. Omit for full output.",
                },
            },
            "required": ["diff"],
        },
    ),
    Tool(
        name="axon_cypher",
        description=(
            "Execute a raw read-only Cypher query directly against the knowledge graph. "
            "Use for custom queries not covered by other tools (e.g. counting nodes by label, "
            "finding symbols matching complex patterns). "
            "Only MATCH/RETURN queries are allowed; write operations are rejected."
        ),
        inputSchema={
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Cypher query string (read-only; MATCH/RETURN only).",
                },
                "max_tokens": {
                    "type": "integer",
                    "description": "Truncate output to this many characters. Omit for full output.",
                },
            },
            "required": ["query"],
        },
    ),
    Tool(
        name="axon_read_symbol",
        description=(
            "Get the exact source code of a symbol by name using byte offsets (O(1) file read). "
            "Returns the precise source of a function, class, method, or interface. "
            "Optionally filter by file path substring to disambiguate same-named symbols. "
            "Use instead of reading the whole file when you know the symbol name."
        ),
        inputSchema={
            "type": "object",
            "properties": {
                "symbol": {
                    "type": "string",
                    "description": "Symbol name to look up (exact match).",
                },
                "file": {
                    "type": "string",
                    "description": "Optional file path substring filter (e.g. 'auth/login').",
                },
                "repo": {
                    "type": "string",
                    "description": "Optional repo slug. Defaults to current directory repo.",
                },
                "max_tokens": {
                    "type": "integer",
                    "description": "Truncate output to this many characters. Omit for full output.",
                },
            },
            "required": ["symbol"],
        },
    ),
    Tool(
        name="axon_find_similar",
        description=(
            "Find symbols semantically similar to a given symbol using stored embeddings. "
            "Use for semantic duplicate detection or to discover related functions/classes. "
            "Returns up to N symbols most semantically similar, with similarity scores. "
            "Requires axon analyze to have been run with embeddings enabled."
        ),
        inputSchema={
            "type": "object",
            "properties": {
                "symbol": {
                    "type": "string",
                    "description": "Name of the symbol to find similar symbols for.",
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of similar symbols to return (default 10).",
                    "default": 10,
                },
                "repo": {
                    "type": "string",
                    "description": (
                        "Name of an indexed repository to query (from axon_list_repos). "
                        "Defaults to the current directory. Optional."
                    ),
                },
                "max_tokens": {
                    "type": "integer",
                    "description": "Truncate output to this many characters. Omit for full output.",
                },
            },
            "required": ["symbol"],
        },
    ),
    Tool(
        name="axon_batch",
        description=(
            "Execute multiple axon tool calls in a single round-trip. "
            "Use when you need results from several tools at once — "
            "e.g., axon_context for 3 symbols. "
            "Reduces connection overhead compared to N separate tool calls. "
            "Each call in the list is executed in order; results are returned as a formatted block."
        ),
        inputSchema={
            "type": "object",
            "properties": {
                "calls": {
                    "type": "array",
                    "description": "Ordered list of tool calls to execute.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "tool": {
                                "type": "string",
                                "description": "Tool name (e.g., 'axon_query', 'axon_context').",
                            },
                            "args": {
                                "type": "object",
                                "description": "Arguments for the tool "
                                "(same as calling it directly).",
                            },
                        },
                        "required": ["tool", "args"],
                    },
                },
                "max_tokens": {
                    "type": "integer",
                    "description": "Truncate each individual result to this many characters. "
                    "Omit for full output.",
                },
            },
            "required": ["calls"],
        },
    ),
]

@server.list_tools()
async def list_tools() -> list[Tool]:
    """Return the list of available Axon tools."""
    return TOOLS

def _dispatch_tool(name: str, arguments: dict, storage: KuzuBackend) -> str:
    """Synchronous tool dispatch — called directly or via ``asyncio.to_thread``."""
    if name == "axon_list_repos":
        return handle_list_repos()
    elif name == "axon_query":
        return handle_query(
            storage,
            arguments.get("query", ""),
            limit=arguments.get("limit", 20),
            language=arguments.get("language"),
            repo=arguments.get("repo"),
        )
    elif name == "axon_context":
        return handle_context(storage, arguments.get("symbol", ""), repo=arguments.get("repo"))
    elif name == "axon_impact":
        return handle_impact(
            storage,
            arguments.get("symbol", ""),
            depth=arguments.get("depth", 3),
            repo=arguments.get("repo"),
        )
    elif name == "axon_dead_code":
        return handle_dead_code(storage)
    elif name == "axon_detect_changes":
        return handle_detect_changes(storage, arguments.get("diff", ""))
    elif name == "axon_cypher":
        return handle_cypher(storage, arguments.get("query", ""))
    elif name == "axon_read_symbol":
        return handle_read_symbol(
            storage,
            symbol=arguments.get("symbol", ""),
            file=arguments.get("file"),
            repo=arguments.get("repo"),
        )
    elif name == "axon_find_similar":
        return handle_find_similar(
            storage,
            symbol=arguments.get("symbol", ""),
            limit=arguments.get("limit", 10),
            repo=arguments.get("repo"),
        )
    else:
        return f"Unknown tool: {name}"


@server.call_tool()
async def call_tool(name: str, arguments: dict) -> list[TextContent]:
    """Dispatch a tool call to the daemon (best-effort) or direct backend."""
    # axon_batch: special multi-call path
    if name == "axon_batch":
        calls = arguments.get("calls", [])
        max_tokens: int | None = arguments.get("max_tokens")
        result: str | None = await asyncio.to_thread(_batch_daemon_call, calls, max_tokens)
        if result is None:
            # Fallback: direct dispatch per sub-call
            storage = _get_storage()
            parts: list[str] = []
            failed_indices: list[int] = []
            total = len(calls)
            for i, call in enumerate(calls):
                sub_name = call.get("tool", "")
                sub_args = call.get("args", {})
                if _lock is not None:
                    async with _lock:
                        sub_result = await asyncio.to_thread(
                            _dispatch_tool, sub_name, sub_args, storage
                        )
                else:
                    sub_result = _dispatch_tool(sub_name, sub_args, storage)
                if sub_result.startswith("Error: ") or sub_result.startswith("Unknown tool:"):
                    failed_indices.append(i)
                if max_tokens is not None and len(sub_result) > max_tokens:
                    sub_result = sub_result[:max_tokens] + f"\n[truncated at {max_tokens} chars]"
                parts.append(f"### {sub_name} ({i + 1}/{total})\n{sub_result}")
            if failed_indices:
                parts.append(
                    f"[BATCH WARNING: {len(failed_indices)}/{total} failed: indices {failed_indices}]"
                )
            result = "\n\n".join(parts)
        return [TextContent(type="text", text=result)]

    # Standard single-tool path (unchanged from 02-02)
    slug: str | None = arguments.get("repo") or _get_local_slug()
    max_tokens = arguments.get("max_tokens")
    daemon_args = {k: v for k, v in arguments.items() if k not in {"repo", "max_tokens"}}
    result = await asyncio.to_thread(_try_daemon_call, name, slug, daemon_args)
    if result is None:
        storage = _get_storage()
        if _lock is not None:
            async with _lock:
                result = await asyncio.to_thread(_dispatch_tool, name, arguments, storage)
        else:
            result = _dispatch_tool(name, arguments, storage)
    if max_tokens is not None and len(result) > max_tokens:
        result = result[:max_tokens] + f"\n[truncated at {max_tokens} chars]"
    return [TextContent(type="text", text=result)]

@server.list_resources()
async def list_resources() -> list[Resource]:
    """Return the list of available Axon resources."""
    return [
        Resource(
            uri="axon://overview",
            name="Codebase Overview",
            description="High-level statistics about the indexed codebase.",
            mimeType="text/plain",
        ),
        Resource(
            uri="axon://dead-code",
            name="Dead Code Report",
            description="List of all symbols flagged as unreachable.",
            mimeType="text/plain",
        ),
        Resource(
            uri="axon://schema",
            name="Graph Schema",
            description="Description of the Axon knowledge graph schema.",
            mimeType="text/plain",
        ),
    ]

def _dispatch_resource(uri_str: str, storage: KuzuBackend) -> str:
    """Synchronous resource dispatch."""
    if uri_str == "axon://overview":
        return get_overview(storage)
    if uri_str == "axon://dead-code":
        return get_dead_code_list(storage)
    if uri_str == "axon://schema":
        return get_schema()
    return f"Unknown resource: {uri_str}"


@server.read_resource()
async def read_resource(uri) -> str:
    """Read the contents of an Axon resource."""
    storage = _get_storage()
    uri_str = str(uri)

    if _lock is not None:
        async with _lock:
            return await asyncio.to_thread(_dispatch_resource, uri_str, storage)
    return _dispatch_resource(uri_str, storage)

async def main() -> None:
    """Run the Axon MCP server over stdio transport."""
    async with stdio_server() as (read, write):
        await server.run(read, write, server.create_initialization_options())

if __name__ == "__main__":
    asyncio.run(main())
