#!/usr/bin/env python3
"""Shared helpers for live MCP probe scripts."""

from __future__ import annotations

import json
import time
import urllib.request
from typing import Any


DEFAULT_URL = "http://127.0.0.1:44129/mcp"
DEFAULT_SQL_URL = "http://127.0.0.1:44129/sql"
DEFAULT_PROTOCOL_VERSION = "2025-06-18"


def rpc_call(url: str, payload: dict[str, Any], timeout: int) -> tuple[float, dict[str, Any]]:
    data = json.dumps(payload).encode("utf-8")
    req = urllib.request.Request(
        url,
        data=data,
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    started = time.perf_counter()
    with urllib.request.urlopen(req, timeout=timeout) as response:
        raw = response.read().decode("utf-8")
    duration_ms = (time.perf_counter() - started) * 1000.0
    return duration_ms, json.loads(raw)


def initialize_session(url: str, timeout: int, client_name: str) -> None:
    for payload in (
        {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": DEFAULT_PROTOCOL_VERSION,
                "clientInfo": {"name": client_name, "version": "1.0"},
                "capabilities": {},
            },
        },
        {"jsonrpc": "2.0", "method": "notifications/initialized"},
    ):
        rpc_call(url, payload, timeout)


def call_tool(
    url: str,
    timeout: int,
    tool_name: str,
    arguments: dict[str, Any],
) -> tuple[float, dict[str, Any]]:
    return rpc_call(
        url,
        {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {"name": tool_name, "arguments": arguments},
        },
        timeout,
    )


def response_text(response: dict[str, Any]) -> str:
    result = response.get("result")
    if not isinstance(result, dict):
        return ""
    content = result.get("content")
    if not isinstance(content, list):
        return ""
    chunks: list[str] = []
    for item in content:
        if isinstance(item, dict):
            text = item.get("text")
            if isinstance(text, str):
                chunks.append(text)
    return "\n".join(chunks)


def response_data(response: dict[str, Any]) -> dict[str, Any]:
    result = response.get("result")
    if not isinstance(result, dict):
        return {}
    data = result.get("data")
    return data if isinstance(data, dict) else {}


def preview_text(text: str, limit: int = 180) -> str:
    compact = " ".join(text.split())
    return compact[:limit]


def default_sql_url(mcp_url: str) -> str:
    if mcp_url.endswith("/mcp"):
        return mcp_url[:-4] + "/sql"
    return DEFAULT_SQL_URL


def sql_query(sql_url: str, timeout: int, query: str) -> list[list[Any]]:
    payload = {"query": query}
    _, response = rpc_call(sql_url, payload, timeout)
    return response if isinstance(response, list) else []


def discover_symbol_probe(url: str, timeout: int, project: str) -> dict[str, str]:
    sql_url = default_sql_url(url)
    escaped_project = project.replace("'", "''")
    rows = sql_query(
        sql_url,
        timeout,
        f"""
        SELECT id, name
        FROM Symbol
        WHERE project_code = '{escaped_project}'
          AND kind IN ('function', 'method')
        ORDER BY
          CASE
            WHEN name = 'Axon.Scanner.scan' THEN 0
            WHEN name = 'Axon.Watcher.Application.start' THEN 1
            WHEN name = 'main' THEN 2
            WHEN lower(name) LIKE '%scan%' THEN 3
            WHEN lower(name) LIKE '%start%' THEN 4
            ELSE 10
          END,
          tested ASC,
          name ASC
        LIMIT 1
        """.strip(),
    )
    if rows and len(rows[0]) >= 2:
        symbol_id = rows[0][0]
        symbol_name = rows[0][1]
        if isinstance(symbol_id, str) and isinstance(symbol_name, str):
            return {"symbol": symbol_name, "exact_symbol": symbol_id}
    return {
        "symbol": "Axon.Scanner.scan",
        "exact_symbol": f"{project}::Axon.Scanner.scan",
    }
