#!/usr/bin/env python3
"""Measure the live subcomponents that feed project_status."""

from __future__ import annotations

import argparse
import json
from pathlib import Path

from mcp_probe_common import (
    DEFAULT_URL,
    call_tool,
    initialize_session,
    preview_text,
    response_data,
    response_text,
)


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Measure status / soll_query_context / conception_view / project_status against the live Axon server."
    )
    parser.add_argument("--url", default=DEFAULT_URL, help=f"MCP URL (default: {DEFAULT_URL})")
    parser.add_argument("--project", default="AXO", help="Canonical project code (default: AXO)")
    parser.add_argument("--timeout", type=int, default=30, help="Per-request timeout in seconds")
    parser.add_argument("--json-out", type=Path, help="Optional JSON output path")
    args = parser.parse_args()

    initialize_session(args.url, args.timeout, "measure_project_status_stack")

    probes = [
        ("status", {"mode": "brief"}),
        ("soll_query_context", {"project_code": args.project, "limit": 5}),
        ("conception_view", {"project_code": args.project, "mode": "brief"}),
        ("project_status", {"project_code": args.project, "mode": "brief"}),
    ]

    results = []
    for tool_name, tool_args in probes:
        try:
            latency_ms, response = call_tool(args.url, args.timeout, tool_name, tool_args)
            text = response_text(response)
            data = response_data(response)
            results.append(
                {
                    "tool": tool_name,
                    "latency_ms": round(latency_ms, 1),
                    "ok": not bool(response.get("result", {}).get("isError")),
                    "text_preview": preview_text(text),
                    "data_keys": list(data.keys())[:12] if isinstance(data, dict) else [],
                }
            )
        except Exception as exc:  # pragma: no cover - live probe path
            results.append({"tool": tool_name, "ok": False, "error": f"{type(exc).__name__}: {exc}"})

    payload = {"url": args.url, "project": args.project, "results": results}
    rendered = json.dumps(payload, ensure_ascii=False, indent=2)
    print(rendered)
    if args.json_out:
        args.json_out.write_text(rendered + "\n", encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
