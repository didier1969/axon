#!/usr/bin/env python3
"""Measure the live subcomponents that feed project_status."""

from __future__ import annotations

import argparse
import json
import statistics
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
    parser.add_argument(
        "--samples",
        type=int,
        default=1,
        help="Samples per tool; latency_ms is their median (default: 1)",
    )
    parser.add_argument("--json-out", type=Path, help="Optional JSON output path")
    args = parser.parse_args()
    if args.samples < 1:
        parser.error("--samples must be >= 1")

    initialize_session(args.url, args.timeout, "measure_project_status_stack")

    probes = [
        ("status", {"mode": "brief"}),
        ("soll_query_context", {"project_code": args.project, "limit": 5}),
        ("conception_view", {"project_code": args.project, "mode": "brief"}),
        ("project_status", {"project_code": args.project, "mode": "brief"}),
    ]

    measurements: dict[str, list[dict[str, object]]] = {name: [] for name, _ in probes}
    for _sample in range(args.samples):
        for tool_name, tool_args in probes:
            try:
                latency_ms, response = call_tool(args.url, args.timeout, tool_name, tool_args)
                text = response_text(response)
                data = response_data(response)
                measurements[tool_name].append(
                    {
                        "latency_ms": round(latency_ms, 1),
                        "ok": not bool(response.get("result", {}).get("isError")),
                        "text_preview": preview_text(text),
                        "data_keys": list(data.keys())[:12] if isinstance(data, dict) else [],
                    }
                )
            except Exception as exc:  # pragma: no cover - live probe path
                measurements[tool_name].append(
                    {"ok": False, "error": f"{type(exc).__name__}: {exc}"}
                )

    results = []
    for tool_name, _tool_args in probes:
        samples = measurements[tool_name]
        latencies = [
            float(sample["latency_ms"])
            for sample in samples
            if isinstance(sample.get("latency_ms"), (int, float))
        ]
        last = samples[-1]
        result = {
            "tool": tool_name,
            "ok": len(latencies) == args.samples and all(sample.get("ok") is True for sample in samples),
            "sample_count": args.samples,
            "latency_samples_ms": latencies,
            "text_preview": last.get("text_preview", ""),
            "data_keys": last.get("data_keys", []),
        }
        if latencies:
            result["latency_ms"] = round(statistics.median(latencies), 1)
        errors = [sample["error"] for sample in samples if "error" in sample]
        if errors:
            result["errors"] = errors
        results.append(result)

    payload = {"url": args.url, "project": args.project, "samples": args.samples, "results": results}
    rendered = json.dumps(payload, ensure_ascii=False, indent=2)
    print(rendered)
    if args.json_out:
        args.json_out.write_text(rendered + "\n", encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
