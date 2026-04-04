#!/usr/bin/env python3
"""Exhaustive MCP validation runner (non-intrusive by default)."""

from __future__ import annotations

import argparse
import json
import re
import sys
import time
import urllib.error
import urllib.request
from dataclasses import dataclass
from typing import Any


DEFAULT_URL = "http://127.0.0.1:44129/mcp"
WRITE_CAPABLE_TOOLS = {
    "refine_lattice",
    "soll_manager",
    "soll_apply_plan",
    "soll_apply_plan_v2",
    "soll_commit_revision",
    "soll_attach_evidence",
    "soll_rollback_revision",
    "export_soll",
    "restore_soll",
    "resume_vectorization",
}


@dataclass
class ToolResult:
    name: str
    status: str  # ok | warn | fail | skip
    duration_ms: int
    note: str
    request_args: dict[str, Any]
    response_excerpt: str
    response_size: int


@dataclass
class ScenarioStep:
    name: str
    tool: str
    args: dict[str, Any]
    expect_contains: list[str]
    fail_if_contains: list[str]


def rpc_call(url: str, payload: dict[str, Any], timeout: int) -> dict[str, Any]:
    data = json.dumps(payload).encode("utf-8")
    req = urllib.request.Request(
        url,
        data=data,
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(req, timeout=timeout) as resp:
        raw = resp.read().decode("utf-8")
    return json.loads(raw)


def default_from_schema(schema: dict[str, Any]) -> Any:
    if not schema:
        return ""
    if "enum" in schema and schema["enum"]:
        return schema["enum"][0]
    t = schema.get("type")
    if t == "string":
        return "x"
    if t == "integer":
        return 1
    if t == "number":
        return 1
    if t == "boolean":
        return False
    if t == "array":
        return []
    if t == "object":
        return {}
    return "x"


def build_args(
    tool_name: str,
    schema: dict[str, Any],
    project: str,
    query: str,
    symbol_probe: str,
) -> dict[str, Any]:
    # Safe, deterministic overrides for known tools.
    overrides: dict[str, dict[str, Any]] = {
        "query": {"query": query, "project": project},
        "inspect": {"symbol": symbol_probe, "project": project},
        "health": {"project": project},
        "audit": {"project": project},
        "impact": {"symbol": symbol_probe, "depth": 2, "project": project},
        "bidi_trace": {"symbol": symbol_probe, "depth": 2},
        "diff": {"diff_content": "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1 +1 @@\n-a\n+b\n"},
        "batch": {"calls": [{"tool": "health", "args": {"project": project}}]},
        "api_break_check": {"symbol": symbol_probe},
        "simulate_mutation": {"symbol": symbol_probe, "project": project},
        "semantic_clones": {"symbol": symbol_probe},
        "architectural_drift": {"source_layer": "ui", "target_layer": "db"},
        "diagnose_indexing": {"project": project},
        "truth_check": {"project": project},
        "schema_overview": {},
        "list_labels_tables": {},
        "query_examples": {},
        "debug": {"project": project},
        "soll_query_context": {"project_slug": "AXO", "limit": 5},
        "soll_work_plan": {"project_slug": "AXO", "limit": 10, "include_ist": True, "format": "json"},
        "soll_verify_requirements": {"project_slug": "AXO"},
        "soll_apply_plan": {"project_slug": "AXO", "dry_run": True, "plan": {}},
        "soll_apply_plan_v2": {"project_slug": "AXO", "author": "mcp_validate", "dry_run": True, "plan": {}},
        "soll_commit_revision": {"preview_id": "dry-run-preview"},
        "soll_rollback_revision": {"revision_id": "dry-run-revision"},
        "soll_attach_evidence": {
            "entity_type": "requirement",
            "entity_id": "REQ-DRY-RUN",
            "artifacts": [{"kind": "metric", "value": "dry-run"}],
        },
        "soll_manager": {
            "action": "update",
            "entity": "requirement",
            "data": {"id": "REQ-DRY-RUN", "status": "planned"},
        },
        "export_soll": {},
        "restore_soll": {"path": "docs/vision/non-existent-file.md"},
        "resume_vectorization": {},
        "validate_soll": {},
        "fs_read": {"uri": "README.md", "start_line": 1, "end_line": 20},
        "refine_lattice": {},
    }
    if tool_name in overrides:
        return overrides[tool_name]

    # Generic fallback from tool schema required fields.
    args: dict[str, Any] = {}
    properties = schema.get("properties", {}) if isinstance(schema, dict) else {}
    required = schema.get("required", []) if isinstance(schema, dict) else []
    for key in required:
        args[key] = default_from_schema(properties.get(key, {}))
    if "project" in properties and "project" not in args:
        args["project"] = project
    return args


def extract_text(result_payload: dict[str, Any]) -> str:
    result = result_payload.get("result")
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


def evaluate_response(tool_name: str, resp: dict[str, Any]) -> tuple[str, str]:
    if "error" in resp and resp["error"] is not None:
        err = resp["error"]
        if isinstance(err, dict):
            code = err.get("code")
            msg = str(err.get("message", ""))
            if code == -32602:
                return "warn", f"invalid params ({msg})"
            return "fail", f"json-rpc error code={code} msg={msg}"
        return "fail", "json-rpc error"

    text_raw = extract_text(resp).strip()
    text = text_raw.lower()
    # Tool-level transport failures are sometimes returned as plain text,
    # but we only trust explicit/leading signatures to avoid false positives
    # on very large business payloads (e.g. diff/search results).
    if text.startswith("not connected"):
        return "fail", "tool response starts with 'Not connected'"
    if text.startswith("mcp error"):
        return "fail", "tool response starts with 'MCP error'"
    if text.startswith("axon backend is unavailable"):
        return "fail", "tool response starts with backend-unavailable"
    if (
        len(text) < 1200
        and "error sending request for url" in text
        and "http://127.0.0.1:44129/mcp" in text
    ):
        return "fail", "tool response indicates backend transport failure"

    # Functional-semantic failures (business-level negatives).
    semantic_warn_patterns = [
        "aucun symbole correspondant n'a ete trouve",
        "symbol not found in current scope",
        "status: warn_input_not_found",
    ]
    for p in semantic_warn_patterns:
        if p in text:
            return "warn", f"semantic warning pattern detected: {p}"

    semantic_fail_patterns = [
        "seems unindexed or parser failed (found 0 files)",
        "preview not found:",
        "erreur update: entité soll introuvable",
        "tool not found",
    ]
    for p in semantic_fail_patterns:
        if p in text:
            return "fail", f"semantic failure pattern detected: {p}"

    # Tool-specific semantic guards.
    if tool_name == "query" and "aucun résultat trouvé" in text:
        return "fail", "query returned no result"
    if tool_name in {"health", "audit", "diagnose_indexing"} and "known files: 0" in text:
        return "fail", f"{tool_name} reports empty project scope"

    return "ok", "ok"


def truncate_text(text: str, limit: int) -> str:
    if len(text) <= limit:
        return text
    return text[: limit - 3] + "..."


def summarize_response(resp: dict[str, Any], excerpt_limit: int) -> tuple[str, int]:
    raw = json.dumps(resp, ensure_ascii=False)
    text = extract_text(resp).strip()
    if text:
        return truncate_text(text.replace("\n", " "), excerpt_limit), len(raw)
    if resp.get("error") is not None:
        return truncate_text(json.dumps(resp.get("error"), ensure_ascii=False), excerpt_limit), len(raw)
    return truncate_text(raw, excerpt_limit), len(raw)


def discover_symbol_probe(url: str, project: str, query: str, timeout: int) -> str:
    payload = {
        "jsonrpc": "2.0",
        "id": 77,
        "method": "tools/call",
        "params": {
            "name": "query",
            "arguments": {"query": query, "project": project},
        },
    }
    try:
        resp = rpc_call(url, payload, timeout)
    except Exception:
        return ""

    text = extract_text(resp)
    if not text:
        return ""

    # Parse first markdown table data row in query output.
    for line in text.splitlines():
        line = line.strip()
        if not line.startswith("|"):
            continue
        if re.search(r"^\|\s*---", line):
            continue
        cells = [part.strip() for part in line.strip("|").split("|")]
        if not cells:
            continue
        first = cells[0]
        if not first or first.lower() in {"nom", "name"}:
            continue
        return first
    return ""


def load_scenario_steps(path: str, default_project: str) -> tuple[str, list[ScenarioStep]]:
    with open(path, "r", encoding="utf-8") as f:
        payload = json.load(f)

    if not isinstance(payload, dict):
        raise ValueError("scenario file must contain a JSON object")

    project = payload.get("project", default_project)
    if not isinstance(project, str) or not project.strip():
        raise ValueError("scenario project must be a non-empty string")

    raw_steps = payload.get("steps")
    if not isinstance(raw_steps, list) or not raw_steps:
        raise ValueError("scenario file must contain a non-empty 'steps' list")

    steps: list[ScenarioStep] = []
    for index, raw_step in enumerate(raw_steps, start=1):
        if not isinstance(raw_step, dict):
            raise ValueError(f"scenario step #{index} must be an object")
        name = raw_step.get("name", f"scenario.step_{index}")
        tool = raw_step.get("tool")
        args = raw_step.get("args", {})
        expect_contains = raw_step.get("expect_contains", [])
        fail_if_contains = raw_step.get("fail_if_contains", [])
        if not isinstance(name, str) or not name.strip():
            raise ValueError(f"scenario step #{index} has invalid name")
        if not isinstance(tool, str) or not tool.strip():
            raise ValueError(f"scenario step #{index} has invalid tool")
        if not isinstance(args, dict):
            raise ValueError(f"scenario step #{index} args must be an object")
        if not isinstance(expect_contains, list) or not all(
            isinstance(item, str) for item in expect_contains
        ):
            raise ValueError(f"scenario step #{index} expect_contains must be a list of strings")
        if not isinstance(fail_if_contains, list) or not all(
            isinstance(item, str) for item in fail_if_contains
        ):
            raise ValueError(f"scenario step #{index} fail_if_contains must be a list of strings")

        step_args = dict(args)
        if "project" not in step_args and tool in {"query", "inspect", "health", "audit", "impact", "debug", "diagnose_indexing", "truth_check"}:
            step_args["project"] = project
        steps.append(
            ScenarioStep(
                name=name,
                tool=tool,
                args=step_args,
                expect_contains=expect_contains,
                fail_if_contains=fail_if_contains,
            )
        )

    return project, steps


def run_query_sequence_scenario(
    url: str,
    timeout: int,
    excerpt_limit: int,
    scenario_steps: list[ScenarioStep],
) -> list[ToolResult]:
    results: list[ToolResult] = []
    for offset, step in enumerate(scenario_steps, start=900):
        payload = {
            "jsonrpc": "2.0",
            "id": offset,
            "method": "tools/call",
            "params": {"name": step.tool, "arguments": step.args},
        }
        t0 = time.time()
        try:
            resp = rpc_call(url, payload, timeout)
            status, note = evaluate_response(step.tool, resp)
            excerpt, response_size = summarize_response(resp, excerpt_limit)
            text = extract_text(resp)
            if status == "ok":
                for expected_snippet in step.expect_contains:
                    if expected_snippet not in text:
                        status, note = "fail", f"missing expected snippet: {expected_snippet}"
                        break
            if status == "ok":
                for forbidden_snippet in step.fail_if_contains:
                    if forbidden_snippet in text:
                        status, note = "fail", f"forbidden snippet present: {forbidden_snippet}"
                        break
        except (urllib.error.URLError, TimeoutError, OSError, json.JSONDecodeError) as e:
            status, note = "fail", f"{type(e).__name__}: {e}"
            excerpt, response_size = f"{type(e).__name__}: {e}", 0
        dt = int((time.time() - t0) * 1000)
        results.append(
            ToolResult(
                name=step.name,
                status=status,
                duration_ms=dt,
                note=note,
                request_args=step.args,
                response_excerpt=excerpt,
                response_size=response_size,
            )
        )

    return results


def run(args: argparse.Namespace) -> int:
    started = time.time()
    scenario_steps: list[ScenarioStep] = []
    scenario_project = args.project
    if args.scenario_file:
        try:
            scenario_project, scenario_steps = load_scenario_steps(args.scenario_file, args.project)
        except (OSError, json.JSONDecodeError, ValueError) as e:
            print(f"FATAL: scenario load failed: {type(e).__name__}: {e}")
            return 2
    project = scenario_project

    # 1) Transport + initialize
    try:
        init_resp = rpc_call(
            args.url,
            {
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2024-11-05",
                    "clientInfo": {"name": "mcp_validate", "version": "1.0"},
                    "capabilities": {},
                },
            },
            args.timeout,
        )
    except (urllib.error.URLError, TimeoutError, OSError, json.JSONDecodeError) as e:
        print(f"FATAL: MCP initialize failed: {type(e).__name__}: {e}")
        return 2

    if init_resp.get("error"):
        print(f"FATAL: initialize returned error: {init_resp['error']}")
        return 2

    # 2) Tools catalog
    try:
        tools_resp = rpc_call(
            args.url,
            {"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}},
            args.timeout,
        )
    except (urllib.error.URLError, TimeoutError, OSError, json.JSONDecodeError) as e:
        print(f"FATAL: tools/list failed: {type(e).__name__}: {e}")
        return 2

    tools = (
        tools_resp.get("result", {}).get("tools", [])
        if isinstance(tools_resp.get("result"), dict)
        else []
    )
    if not isinstance(tools, list) or not tools:
        print("FATAL: tools/list returned no tools")
        return 2

    symbol_probe = args.symbol.strip() if isinstance(args.symbol, str) else ""
    if not symbol_probe:
        discovered = discover_symbol_probe(args.url, project, args.query, args.timeout)
        symbol_probe = discovered or (args.query if args.query.strip() else "booking")

    tool_results: list[ToolResult] = []
    for i, tool in enumerate(tools, start=100):
        name = str(tool.get("name", "")).strip()
        schema = tool.get("inputSchema", {}) if isinstance(tool, dict) else {}
        if not name:
            continue
        if (not args.allow_mutations) and name in WRITE_CAPABLE_TOOLS:
            tool_results.append(
                ToolResult(
                    name=name,
                    status="skip",
                    duration_ms=0,
                    note="skipped write-capable tool (enable --allow-mutations to execute)",
                    request_args={},
                    response_excerpt="",
                    response_size=0,
                )
            )
            continue
        call_args = build_args(
            name,
            schema if isinstance(schema, dict) else {},
            project,
            args.query,
            symbol_probe,
        )
        payload = {
            "jsonrpc": "2.0",
            "id": i,
            "method": "tools/call",
            "params": {"name": name, "arguments": call_args},
        }
        t0 = time.time()
        try:
            resp = rpc_call(args.url, payload, args.timeout)
            status, note = evaluate_response(name, resp)
            excerpt, response_size = summarize_response(resp, args.excerpt)
        except (urllib.error.URLError, TimeoutError, OSError, json.JSONDecodeError) as e:
            status, note = "fail", f"{type(e).__name__}: {e}"
            excerpt, response_size = f"{type(e).__name__}: {e}", 0
        dt = int((time.time() - t0) * 1000)
        tool_results.append(
            ToolResult(
                name=name,
                status=status,
                duration_ms=dt,
                note=note,
                request_args=call_args,
                response_excerpt=excerpt,
                response_size=response_size,
            )
        )

    if scenario_steps:
        tool_results.extend(
            run_query_sequence_scenario(args.url, args.timeout, args.excerpt, scenario_steps)
        )

    ok = sum(1 for r in tool_results if r.status == "ok")
    warn = sum(1 for r in tool_results if r.status == "warn")
    fail = sum(1 for r in tool_results if r.status == "fail")
    skip = sum(1 for r in tool_results if r.status == "skip")

    elapsed_ms = int((time.time() - started) * 1000)
    print(f"MCP validation completed in {elapsed_ms} ms")
    print(f"URL: {args.url}")
    print(f"Project: {project}")
    if args.scenario_file:
        print(f"Scenario: {args.scenario_file}")
    print(f"Symbol Probe: {symbol_probe}")
    print(f"Tools total: {len(tool_results)} | ok={ok} warn={warn} fail={fail} skip={skip}")
    transport_health = "pass" if fail == 0 else "degraded"
    semantic_quality = "pass" if (fail == 0 and warn == 0) else ("warn" if fail == 0 else "degraded")
    print(f"Health gates: transport_health={transport_health} semantic_quality={semantic_quality}")
    print("")
    print("Per-tool status:")
    for r in sorted(tool_results, key=lambda x: (x.status, x.name)):
        print(f"- {r.name}: {r.status} ({r.duration_ms} ms) :: {r.note}")
        if args.verbose:
            print(f"  args={json.dumps(r.request_args, ensure_ascii=False)}")
            print(f"  response_size={r.response_size}B")
            print(f"  excerpt={r.response_excerpt}")

    if args.json_out:
        payload = {
            "url": args.url,
            "project": project,
            "summary": {
                "total": len(tool_results),
                "ok": ok,
                "warn": warn,
                "fail": fail,
                "skip": skip,
                "elapsed_ms": elapsed_ms,
                "allow_mutations": args.allow_mutations,
                "symbol_probe": symbol_probe,
                "transport_health": transport_health,
                "semantic_quality": semantic_quality,
                "scenario_file": args.scenario_file,
            },
            "results": [r.__dict__ for r in tool_results],
            "slowest_tools": [
                r.__dict__
                for r in sorted(tool_results, key=lambda x: x.duration_ms, reverse=True)[: args.top_slowest]
            ],
            "failed_tools": [r.__dict__ for r in tool_results if r.status == "fail"],
            "skipped_tools": [r.__dict__ for r in tool_results if r.status == "skip"],
        }
        with open(args.json_out, "w", encoding="utf-8") as f:
            json.dump(payload, f, indent=2, ensure_ascii=False)

    if fail > 0:
        return 1
    if args.strict and warn > 0:
        return 1
    return 0


def parse_args(argv: list[str]) -> argparse.Namespace:
    p = argparse.ArgumentParser(description="Exhaustive MCP tool validator")
    p.add_argument("--url", default=DEFAULT_URL, help="MCP HTTP endpoint")
    p.add_argument("--project", default="BookingSystem", help="Project scope for project-aware tools")
    p.add_argument("--query", default="booking", help="Default semantic query term")
    p.add_argument(
        "--symbol",
        default="",
        help="Optional symbol probe for symbol-based tools (defaults to --query)",
    )
    p.add_argument("--timeout", type=int, default=20, help="Per-call timeout in seconds")
    p.add_argument("--strict", action="store_true", help="Treat warnings as failures")
    p.add_argument(
        "--allow-mutations",
        action="store_true",
        help="Execute write-capable tools (disabled by default to avoid changing workspace/client files)",
    )
    p.add_argument("--verbose", action="store_true", help="Print per-tool args and response excerpts")
    p.add_argument("--excerpt", type=int, default=240, help="Max chars for response excerpt")
    p.add_argument("--top-slowest", type=int, default=5, help="Top N slowest tools in JSON report")
    p.add_argument("--json-out", default="", help="Optional JSON output path")
    p.add_argument("--scenario-file", default="", help="Optional JSON scenario file for sequential validation")
    return p.parse_args(argv)


if __name__ == "__main__":
    raise SystemExit(run(parse_args(sys.argv[1:])))
