#!/usr/bin/env python3
"""Measure inspect / path / impact resolution for a concrete symbol probe."""

from __future__ import annotations

import argparse
import json
from pathlib import Path

from mcp_probe_common import (
    DEFAULT_URL,
    call_tool,
    discover_symbol_probe,
    initialize_session,
    preview_text,
    response_text,
)


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Measure inspect / path / impact behavior for an exact symbol on the live Axon server."
    )
    parser.add_argument("--url", default=DEFAULT_URL, help=f"MCP URL (default: {DEFAULT_URL})")
    parser.add_argument("--project", default="AXO", help="Canonical project code (default: AXO)")
    parser.add_argument("--symbol", help="Exact symbol identifier to probe; defaults to live discovery")
    parser.add_argument("--timeout", type=int, default=20, help="Per-request timeout in seconds")
    parser.add_argument("--json-out", type=Path, help="Optional JSON output path")
    args = parser.parse_args()

    initialize_session(args.url, args.timeout, "measure_symbol_flow_tools")
    probe = discover_symbol_probe(args.url, args.timeout, args.project)
    symbol = args.symbol or probe["exact_symbol"]

    probes = [
        ("inspect", {"project": args.project, "symbol": symbol, "mode": "brief"}),
        ("path", {"project": args.project, "source": symbol, "mode": "brief"}),
        ("impact", {"project": args.project, "symbol": symbol, "mode": "brief"}),
    ]

    results = []
    for tool_name, tool_args in probes:
        try:
            latency_ms, response = call_tool(args.url, args.timeout, tool_name, tool_args)
            text = response_text(response)
            results.append(
                {
                    "tool": tool_name,
                    "latency_ms": round(latency_ms, 1),
                    "ok": not bool(response.get("result", {}).get("isError")),
                    "text_preview": preview_text(text, limit=220),
                }
            )
        except Exception as exc:  # pragma: no cover - live probe path
            results.append({"tool": tool_name, "ok": False, "error": f"{type(exc).__name__}: {exc}"})

    payload = {
        "url": args.url,
        "project": args.project,
        "symbol": symbol,
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
