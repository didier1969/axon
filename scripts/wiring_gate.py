#!/usr/bin/env python3
"""Shell/CI wiring gate — REQ-AXO-902224.

Runs the MCP `wiring` (+ optional `orphan_clusters`) analysis on a project and
returns a NON-ZERO exit code when a public production symbol has no production
caller. Lets a consumer project's CI encode the "organe câblé" invariant
(REQ-NEX-123: every public `lib/` module must have >=1 prod caller) as a
BLOCKING shell gate, instead of it living only inside an interactive MCP session
or inside `axon_pre_flight_check` (which only gates the Axon repo's own commits).

STALENESS — READ THIS. The gate measures the LAST-INDEXED working-tree state as
held by the live Axon brain (the snapshot is warmed at call time), NOT your exact
PR checkout. It relies on the brain's continuous reconciliation (<=900s sweep +
Watchman deltas) to have already indexed the code under test. Treat it as a
PERIODIC structural-health gate, not a strict per-diff blocker: if a PR's code is
not yet on disk at the indexed root and re-indexed, the gate evaluates stale
state (worst case a false PASS on unindexed code).

Exit codes:
  0  PASS  — every selected metric is <= threshold
  1  FAIL  — a selected metric exceeds threshold (a real structural violation)
  2  ERROR — operational/usage failure (RPC failed, project unknown, bad args)

Metrics (select with --fail-on, comma-separated, or `any`):
  test_only  wiring.test_only_count   delivered + tested but NO prod caller (OPV class; high confidence)
  isolated   wiring.isolated_count    no caller at all (advisory: may be an undetected entry point)
  orphan     orphan_clusters.unreached_count   symbols in mutually-wired-but-globally-dead clusters

Examples:
  ./scripts/axon --instance live wiring --project NEX --fail-on test_only
  ./scripts/axon --instance live wiring --project NEX --fail-on test_only --threshold 2
  ./scripts/axon --instance live wiring --project NEX --fail-on test_only,orphan --json
  # cross-host CI must point --url at the live brain (default is 127.0.0.1):
  ./scripts/axon --instance live wiring --project NEX --url http://axon-host:44129/mcp
"""

from __future__ import annotations

import argparse
import json
import sys
import urllib.request
from typing import Any

DEFAULT_MCP_URL = "http://127.0.0.1:44129/mcp"

# `wiring` computes test_only_count/isolated_count over its RETURNED orphans array,
# which is capped by `top` (max 200). We therefore always request the max for the
# gate DECISION so the counts are true totals (up to 200 combined orphans), and cap
# only the DISPLAYED offenders via --top. Above 200 combined orphans the counts
# saturate — flagged in the output so a --threshold check can't silently under-count.
COUNT_TOP = 200

# Selectable gate metrics -> (source tool, count field in that tool's data envelope).
METRICS = {
    "test_only": ("wiring", "test_only_count"),
    "isolated": ("wiring", "isolated_count"),
    "orphan": ("orphan_clusters", "unreached_count"),
}


def rpc_call(url: str, tool_name: str, arguments: dict[str, Any], timeout: int) -> dict[str, Any]:
    payload = {
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {"name": tool_name, "arguments": arguments},
    }
    data = json.dumps(payload).encode("utf-8")
    req = urllib.request.Request(
        url,
        data=data,
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(req, timeout=timeout) as resp:
        return json.loads(resp.read().decode("utf-8"))


def tool_data(url: str, tool: str, arguments: dict[str, Any], timeout: int) -> dict[str, Any]:
    """Call an MCP tool and return its `result.data` envelope, or raise on error."""
    resp = rpc_call(url, tool, arguments, timeout)
    if resp.get("error") is not None:
        raise RuntimeError(f"{tool}: {json.dumps(resp['error'], ensure_ascii=False)}")
    result = resp.get("result")
    if not isinstance(result, dict):
        raise RuntimeError(f"{tool}: malformed MCP response (no result object)")
    data = result.get("data")
    if not isinstance(data, dict):
        raise RuntimeError(f"{tool}: response carried no data envelope")
    return data


def main() -> int:
    parser = argparse.ArgumentParser(
        prog="axon wiring",
        description=(
            "CI wiring gate: EXIT NON-ZERO (1) when a public production symbol has no "
            "production caller. Not a plain read — use `mcp-call call wiring` for that. "
            "Measures the last-indexed working-tree state via the live brain, not your "
            "exact commit (see module docstring: staleness)."
        ),
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument("--project", required=True, help="Project code, e.g. NEX")
    parser.add_argument(
        "--fail-on",
        default="test_only",
        help=(
            "Comma-separated metrics that trip the gate: test_only, isolated, orphan, "
            "or `any` (all three). Default: test_only (the high-confidence OPV class)."
        ),
    )
    parser.add_argument(
        "--threshold",
        type=int,
        default=0,
        help="Max allowed value for EACH selected metric before failing (default: 0).",
    )
    parser.add_argument(
        "--top", type=int, default=20, help="Max offending symbols to list (default: 20)."
    )
    parser.add_argument(
        "--no-warm",
        action="store_true",
        help="Skip the ist_snapshot_warm call (only if the brain snapshot is already warm).",
    )
    parser.add_argument("--json", action="store_true", help="Emit a JSON verdict instead of text.")
    parser.add_argument("--url", default=DEFAULT_MCP_URL, help=f"MCP endpoint (default: {DEFAULT_MCP_URL})")
    parser.add_argument("--timeout", type=int, default=60, help="RPC timeout in seconds")
    args = parser.parse_args()

    raw = [m.strip() for m in args.fail_on.split(",") if m.strip()]
    if "any" in raw:
        selected = list(METRICS.keys())
    else:
        selected = raw
    unknown = [m for m in selected if m not in METRICS]
    if unknown or not selected:
        print(
            f"Invalid --fail-on {args.fail_on!r}: choose from {', '.join(METRICS)} or 'any'.",
            file=sys.stderr,
        )
        return 2

    need_wiring = any(METRICS[m][0] == "wiring" for m in selected)
    need_orphan = any(METRICS[m][0] == "orphan_clusters" for m in selected)

    try:
        warm = None
        if not args.no_warm:
            warm = tool_data(args.url, "ist_snapshot_warm", {"project_code": args.project}, args.timeout)
        wiring = tool_data(args.url, "wiring", {"project_code": args.project, "top": COUNT_TOP}, args.timeout) if need_wiring else {}
        orphan = tool_data(args.url, "orphan_clusters", {"project_code": args.project}, args.timeout) if need_orphan else {}
    except Exception as exc:  # noqa: BLE001 — any RPC/envelope failure is operational
        print(f"wiring-gate error: {exc}", file=sys.stderr)
        return 2

    sources = {"wiring": wiring, "orphan_clusters": orphan}

    metrics_report: dict[str, dict[str, Any]] = {}
    tripped = False
    for name in selected:
        tool, field = METRICS[name]
        value = sources[tool].get(field)
        if not isinstance(value, int):
            print(f"wiring-gate error: {tool} did not return integer '{field}'", file=sys.stderr)
            return 2
        metric_tripped = value > args.threshold
        tripped = tripped or metric_tripped
        metrics_report[name] = {"value": value, "threshold": args.threshold, "tripped": metric_tripped}

    # Offenders (capped by --top) for human/JSON output.
    offenders: dict[str, list[str]] = {}
    if need_wiring:
        for cat in ("test_only", "isolated"):
            if cat in selected:
                offenders[cat] = [
                    o.get("name", o.get("id", "?"))
                    for o in wiring.get("orphans", [])
                    if o.get("category") == cat
                ][: args.top]
    if need_orphan and "orphan" in selected:
        flat: list[str] = []
        for cl in orphan.get("clusters", []):
            flat.extend(cl.get("nodes", []))
        offenders["orphan"] = flat[: args.top]

    # wiring counts saturate at COUNT_TOP; flag it so a --threshold check is trusted.
    counts_saturated = need_wiring and len(wiring.get("orphans", [])) >= COUNT_TOP

    scope = {
        "candidate_count": orphan.get("candidate_count") if need_orphan else wiring.get("candidate_count"),
        "root_count": orphan.get("root_count"),
        "soll_declared_symbols": wiring.get("soll_declared_symbols"),
        "snapshot_nodes_loaded": (warm or {}).get("nodes_loaded"),
        "wiring_counts_saturated_at": COUNT_TOP if counts_saturated else None,
    }

    if args.json:
        print(json.dumps(
            {
                "project": args.project,
                "verdict": "fail" if tripped else "pass",
                "threshold": args.threshold,
                "metrics": metrics_report,
                "offenders": offenders,
                "scope": scope,
                "staleness_note": "last-indexed working-tree state via live brain, not the exact CI commit",
            },
            indent=2,
            ensure_ascii=False,
        ))
    else:
        verdict = "FAIL" if tripped else "PASS"
        print(f"wiring-gate {args.project}: {verdict} (threshold={args.threshold})")
        for name in selected:
            r = metrics_report[name]
            mark = "✗" if r["tripped"] else "✓"
            print(f"  {mark} {name:<10} = {r['value']} (max {r['threshold']})")
            for sym in offenders.get(name, []):
                print(f"        · {sym}")
        sc = ", ".join(f"{k}={v}" for k, v in scope.items() if v is not None)
        if sc:
            print(f"  scope: {sc}")
        if counts_saturated:
            print(f"  warning: wiring returned >= {COUNT_TOP} orphans — test_only/isolated counts saturate; a high --threshold may under-count.")
        print("  note: last-indexed working-tree state via live brain, not the exact CI commit.")

    return 1 if tripped else 0


if __name__ == "__main__":
    raise SystemExit(main())
