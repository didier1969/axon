"""Axon daemon — asyncio Unix socket server.

Listens on ~/.axon/daemon.sock, dispatches MCP tool calls via LRU-cached KuzuBackend instances.

Protocol: JSON-line (one JSON object per line, newline-terminated).
"""
from __future__ import annotations

import asyncio
import json
import logging
import os
import signal

from axon.core.paths import daemon_pid_path, daemon_sock_path
from axon.daemon.lru_cache import LRUBackendCache
from axon.daemon.protocol import decode_request, encode_response

logger = logging.getLogger(__name__)

# Supported tool names (must match MCP tool names)
_TOOL_NAMES = frozenset({
    "axon_query", "axon_context", "axon_impact", "axon_dead_code",
    "axon_detect_changes", "axon_cypher", "axon_list_repos", "axon_daemon_status",
})


def _dispatch_tool(cache: LRUBackendCache, tool: str, slug: str | None, args: dict) -> str:
    """Synchronous tool dispatch — runs in a thread pool via asyncio.to_thread."""
    from axon.mcp.tools import (
        MAX_TRAVERSE_DEPTH,
        handle_context,
        handle_cypher,
        handle_dead_code,
        handle_detect_changes,
        handle_impact,
        handle_list_repos,
        handle_query,
    )

    if tool == "axon_list_repos":
        return handle_list_repos()

    if tool == "axon_daemon_status":
        s = cache.status()
        repos = ", ".join(s["cached"]) if s["cached"] else "none"
        return f"Daemon running\nCached: {s['count']}/{s['maxsize']} repos: {repos}"

    if not slug:
        return "Error: slug required for tool " + tool

    storage = cache.get_or_load(slug)
    if storage is None:
        return f"Error: no index found for repo '{slug}'"

    if tool == "axon_query":
        return handle_query(
            storage,
            args.get("query", ""),
            limit=args.get("limit", 20),
            language=args.get("language"),
        )
    if tool == "axon_context":
        return handle_context(storage, args.get("symbol", ""))
    if tool == "axon_impact":
        return handle_impact(
            storage,
            args.get("symbol", ""),
            depth=min(args.get("depth", 3), MAX_TRAVERSE_DEPTH),
        )
    if tool == "axon_dead_code":
        return handle_dead_code(storage)
    if tool == "axon_detect_changes":
        return handle_detect_changes(storage, args.get("diff", ""))
    if tool == "axon_cypher":
        return handle_cypher(storage, args.get("query", ""))

    return f"Unknown tool: {tool}"


async def _handle_connection(
    reader: asyncio.StreamReader, writer: asyncio.StreamWriter, cache: LRUBackendCache
) -> None:
    """Handle a single client connection — reads requests, writes responses."""
    peer = writer.get_extra_info("peername", "unknown")
    logger.debug("Client connected: %s", peer)
    try:
        while True:
            line = await reader.readline()
            if not line:
                break
            try:
                req = decode_request(line)
            except json.JSONDecodeError as exc:
                writer.write(encode_response(None, f"Bad request: {exc}"))
                await writer.drain()
                continue

            req_id = req.get("id", "")
            tool = req.get("tool", "")
            slug = req.get("slug")
            args = req.get("args", {})

            try:
                result = await asyncio.to_thread(_dispatch_tool, cache, tool, slug, args)
                writer.write(encode_response(result, None, req_id))
            except Exception as exc:  # noqa: BLE001
                logger.exception("Error dispatching tool %s", tool)
                writer.write(encode_response(None, str(exc), req_id))
            await writer.drain()
    except (ConnectionResetError, BrokenPipeError):
        pass
    finally:
        writer.close()
        logger.debug("Client disconnected: %s", peer)


async def run_daemon(maxsize: int = 5) -> None:
    """Start the daemon event loop. Writes PID, listens on socket, handles SIGTERM."""
    sock_path = daemon_sock_path()
    pid_path = daemon_pid_path()

    # Write PID file
    pid_path.parent.mkdir(parents=True, exist_ok=True)
    pid_path.write_text(str(os.getpid()))

    # Remove stale socket
    if sock_path.exists():
        sock_path.unlink()

    cache = LRUBackendCache(maxsize)
    stop_event = asyncio.Event()

    loop = asyncio.get_running_loop()
    loop.add_signal_handler(signal.SIGTERM, stop_event.set)
    loop.add_signal_handler(signal.SIGINT, stop_event.set)

    async def handle_conn(reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
        await _handle_connection(reader, writer, cache)

    server = await asyncio.start_unix_server(handle_conn, str(sock_path))
    logger.info("Daemon listening on %s (PID %d, maxsize=%d)", sock_path, os.getpid(), maxsize)

    async with server:
        await stop_event.wait()

    # Cleanup
    cache.close_all()
    if sock_path.exists():
        sock_path.unlink()
    if pid_path.exists():
        pid_path.unlink()
    logger.info("Daemon shutdown complete")
