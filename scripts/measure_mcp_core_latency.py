#!/usr/bin/env python3
"""Measure the live MCP core/public surface with deterministic probes."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any

from mcp_probe_common import (
    DEFAULT_URL,
    call_tool,
    discover_symbol_probe,
    initialize_session,
    preview_text,
    response_data,
    response_text,
)


def build_probe_rows(project: str, symbol: str, exact_symbol: str) -> list[tuple[str, dict[str, Any]]]:
    return [
        ("status", {"mode": "brief"}),
        ("project_status", {"project_code": project, "mode": "brief"}),
        ("query", {"project": project, "query": symbol, "mode": "brief"}),
        ("inspect", {"project": project, "symbol": symbol, "mode": "brief"}),
        (
            "retrieve_context",
            {
                "project": project,
                "question": f"Where is {symbol} wired?",
                "token_budget": 900,
                "mode": "brief",
            },
        ),
        ("why", {"project": project, "symbol": symbol, "mode": "brief"}),
        ("path", {"project": project, "source": exact_symbol, "mode": "brief"}),
        ("impact", {"project": project, "symbol": exact_symbol, "mode": "brief"}),
        ("anomalies", {"project": project, "mode": "brief"}),
        (
            "change_safety",
            {
                "project_code": project,
                "target": exact_symbol,
                "target_type": "symbol",
                "mode": "brief",
            },
        ),
        ("conception_view", {"project_code": project, "mode": "brief"}),
        ("snapshot_history", {"project_code": project, "limit": 3}),
        ("snapshot_diff", {"project_code": project}),
    ]


def main() -> int:
    parser = argparse.ArgumentParser(description="Measure core MCP tools against the live Axon server.")
    parser.add_argument("--url", default=DEFAULT_URL, help=f"MCP URL (default: {DEFAULT_URL})")
    parser.add_argument("--project", default="AXO", help="Canonical project code (default: AXO)")
    parser.add_argument("--symbol", help="Loose symbol probe for search-oriented tools; defaults to live discovery")
    parser.add_argument("--exact-symbol", help="Exact symbol probe for path/impact/change_safety; defaults to live discovery")
    parser.add_argument("--timeout", type=int, default=20, help="Per-request timeout in seconds")
    parser.add_argument("--json-out", type=Path, help="Optional JSON output path")
    args = parser.parse_args()

    initialize_session(args.url, args.timeout, "measure_mcp_core_latency")
    probe = discover_symbol_probe(args.url, args.timeout, args.project)
    symbol = args.symbol or probe["symbol"]
    exact_symbol = args.exact_symbol or probe["exact_symbol"]

    results: list[dict[str, Any]] = []
    for tool_name, tool_args in build_probe_rows(args.project, symbol, exact_symbol):
        try:
            latency_ms, response = call_tool(args.url, args.timeout, tool_name, tool_args)
            text = response_text(response)
            data = response_data(response)
            row: dict[str, Any] = {
                "tool": tool_name,
                "latency_ms": round(latency_ms, 1),
                "ok": not bool(response.get("result", {}).get("isError")),
                "text_preview": preview_text(text),
            }
            if tool_name == "retrieve_context":
                planner = data.get("planner", {})
                packet = data.get("packet", {})
                row["route"] = planner.get("route")
                row["direct_evidence"] = len(packet.get("direct_evidence", []) or [])
                row["supporting_chunks"] = len(packet.get("supporting_chunks", []) or [])
            elif isinstance(data, dict):
                row["data_keys"] = list(data.keys())[:10]
            results.append(row)
        except Exception as exc:  # pragma: no cover - live probe path
            results.append(
                {
                    "tool": tool_name,
                    "ok": False,
                    "error": f"{type(exc).__name__}: {exc}",
                }
            )

    payload = {
        "url": args.url,
        "project": args.project,
        "symbol": symbol,
        "exact_symbol": exact_symbol,
        "discovered_probe": probe,
        "results": results,
    }
    rendered = json.dumps(payload, ensure_ascii=False, indent=2)
    print(rendered)
    if args.json_out:
        args.json_out.write_text(rendered + "\n", encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
