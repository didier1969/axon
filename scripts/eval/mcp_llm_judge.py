#!/usr/bin/env python3
"""
MCP Independent LLM-as-a-Judge Evaluation Harness
Uses Gemini 3.8 Flash (via local 'agy' CLI runtime and Enterprise subscription)
to evaluate Axon MCP tool responses according to the advanced-evaluation rubric.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import time
import urllib.request
from dataclasses import asdict, dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

DEFAULT_MCP_URL = "http://127.0.0.1:44129/mcp"
DEFAULT_MODEL = "gemini-3.8-flash-low"
REPORT_DIR = Path("/home/dstadel/projects/axon/.axon/eval")


@dataclass
class ToolTestCase:
    tool: str
    arguments: dict[str, Any]
    description: str


# Representative suite covering read-only, IST, graph algorithms, SOLL, and diagnostics
BENCHMARK_CASES: list[ToolTestCase] = [
    ToolTestCase(
        tool="status",
        arguments={"mode": "brief"},
        description="Core operational status snapshot",
    ),
    ToolTestCase(
        tool="sharded_graph_status",
        arguments={"project_code": "AXO"},
        description="Horizontal CSR sharded graph topology status",
    ),
    ToolTestCase(
        tool="query",
        arguments={"query": "ShardedIstGraph", "project": "AXO"},
        description="Lexical and semantic symbol search",
    ),
    ToolTestCase(
        tool="inspect",
        arguments={"symbol": "ShardedIstGraph", "project": "AXO"},
        description="Deep symbol inspection with callers/callees",
    ),
    ToolTestCase(
        tool="why",
        arguments={"symbol": "ShardedIstGraph", "project": "AXO"},
        description="Traceability and rationale lookup",
    ),
    ToolTestCase(
        tool="anomalies",
        arguments={"project": "AXO"},
        description="Graph structural anomalies detection",
    ),
    ToolTestCase(
        tool="soll_validate",
        arguments={"project_code": "AXO"},
        description="Intentional SOLL coherence validation",
    ),
    ToolTestCase(
        tool="soll_query_context",
        arguments={"project_code": "AXO"},
        description="High-level project intent overview",
    ),
    ToolTestCase(
        tool="tech_debt_inventory",
        arguments={"project_code": "AXO"},
        description="Tracked tech debt and migration remnants",
    ),
    ToolTestCase(
        tool="truth_check",
        arguments={},
        description="Reader vs writer coherence audit",
    ),
    ToolTestCase(
        tool="taint_trace",
        arguments={"source": "handle_request", "sink": "execute", "project": "AXO"},
        description="Inter-procedural dataflow and taint vulnerability analysis",
    ),
    ToolTestCase(
        tool="structural_health_index",
        arguments={"project_code": "AXO"},
        description="Geometric structural health index",
    ),
]


def call_mcp(url: str, tool: str, arguments: dict[str, Any], timeout: int = 15) -> dict[str, Any]:
    payload = {
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": tool,
            "arguments": arguments,
        },
        "id": int(time.time() * 1000) % 1000000,
    }
    data = json.dumps(payload).encode("utf-8")
    req = urllib.request.Request(
        url,
        data=data,
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    start = time.perf_counter()
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            elapsed_ms = (time.perf_counter() - start) * 1000
            res = json.loads(resp.read().decode("utf-8"))
            return {
                "ok": True,
                "elapsed_ms": round(elapsed_ms, 2),
                "response": res.get("result", res),
            }
    except Exception as e:
        elapsed_ms = (time.perf_counter() - start) * 1000
        return {
            "ok": False,
            "elapsed_ms": round(elapsed_ms, 2),
            "error": str(e),
        }


def ask_llm_judge(
    model: str,
    tool: str,
    arguments: dict[str, Any],
    response_data: Any,
    timeout: int = 35,
) -> dict[str, Any]:
    truncated_response = json.dumps(response_data, indent=2, ensure_ascii=False)
    if len(truncated_response) > 4000:
        truncated_response = truncated_response[:4000] + "\n... [TRUNCATED FOR JUDGE EVALUATION]"

    prompt = f"""You are an expert Systems Architect acting as an independent LLM-as-a-Judge.
Evaluate the following Model Context Protocol (MCP) tool response returned by the Axon infrastructure.

Target Tool: `{tool}`
Input Arguments: {json.dumps(arguments)}

Tool Output:
```json
{truncated_response}
```

Evaluation Rubric (Score each from 1 to 5, where 5 is exceptional, 3 is acceptable, 1 is poor):
1. **accuracy**: Is the content factually coherent, structurally sound, and free of hallucinations or empty/dead-end answers?
2. **actionability**: Does the response provide clear, machine-actionable information, structural anchors, or deterministic next steps for an AI coding agent?
3. **clarity**: Is the response concise, free of verbose fluff, and well-structured?

Verdict:
Choose "PASS" if the response meets production standards for autonomous agents, or "FAIL" if it contains bugs, dead-ends, misleading guidance, or broken JSON.

Output format (MUST be valid JSON only, no markdown wrapping, no extra text):
{{"accuracy": 5, "actionability": 5, "clarity": 5, "verdict": "PASS", "summary": "One or two sentences explaining your evaluation."}}
"""

    cmd = [
        "agy",
        "-p",
        prompt,
        "--model",
        model,
        "--output-format",
        "text",
    ]
    try:
        proc = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout)
        if proc.returncode != 0:
            return {
                "accuracy": 0,
                "actionability": 0,
                "clarity": 0,
                "verdict": "ERROR",
                "summary": f"LLM judge command failed: {proc.stderr.strip()}",
            }
        raw = proc.stdout.strip()
        # Clean potential markdown fences
        if raw.startswith("```"):
            lines = raw.splitlines()
            if lines[0].startswith("```"):
                lines = lines[1:]
            if lines and lines[-1].startswith("```"):
                lines = lines[:-1]
            raw = "\n".join(lines).strip()
        parsed = json.loads(raw)
        return parsed
    except Exception as e:
        return {
            "accuracy": 0,
            "actionability": 0,
            "clarity": 0,
            "verdict": "ERROR",
            "summary": f"Parsing judge evaluation failed: {e}",
        }


def main() -> int:
    parser = argparse.ArgumentParser(description="Evaluate MCP tool responses with Gemini 3.8 Flash judge")
    parser.add_argument("--url", default=DEFAULT_MCP_URL, help="MCP endpoint")
    parser.add_argument("--model", default=DEFAULT_MODEL, help="Judge model name")
    parser.add_argument("--top", type=int, default=len(BENCHMARK_CASES), help="Number of benchmark cases to test")
    args = parser.parse_args()

    REPORT_DIR.mkdir(parents=True, exist_ok=True)
    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    report_file = REPORT_DIR / f"judge_run_{stamp}.json"

    print(f"=== Axon MCP Independent LLM-as-a-Judge Evaluation ===")
    print(f"Endpoint: {args.url}")
    print(f"Judge Model: {args.model}")
    print(f"Test Cases: {min(args.top, len(BENCHMARK_CASES))}\n")

    results = []
    cases_to_run = BENCHMARK_CASES[: args.top]

    for idx, case in enumerate(cases_to_run, 1):
        print(f"[{idx}/{len(cases_to_run)}] Calling `{case.tool}`...", end=" ", flush=True)
        mcp_res = call_mcp(args.url, case.tool, case.arguments)
        if not mcp_res["ok"]:
            print(f"❌ MCP call failed in {mcp_res['elapsed_ms']}ms: {mcp_res.get('error')}")
            results.append({
                "tool": case.tool,
                "arguments": case.arguments,
                "mcp_ok": False,
                "elapsed_ms": mcp_res["elapsed_ms"],
                "judge": {"verdict": "FAIL", "summary": "MCP endpoint returned connection/execution error"},
            })
            continue

        print(f"✅ ({mcp_res['elapsed_ms']}ms) -> Evaluating with {args.model}...", end=" ", flush=True)
        judge_res = ask_llm_judge(args.model, case.tool, case.arguments, mcp_res["response"])
        verdict = judge_res.get("verdict", "FAIL")
        print(f"[{verdict}] (Acc: {judge_res.get('accuracy')}/5, Act: {judge_res.get('actionability')}/5, Cla: {judge_res.get('clarity')}/5)")
        print(f"   ↳ {judge_res.get('summary')}")

        results.append({
            "tool": case.tool,
            "description": case.description,
            "arguments": case.arguments,
            "mcp_ok": True,
            "elapsed_ms": mcp_res["elapsed_ms"],
            "judge": judge_res,
        })

    # Summary calculations
    total = len(results)
    passed = sum(1 for r in results if r.get("judge", {}).get("verdict") == "PASS")
    avg_acc = sum(r.get("judge", {}).get("accuracy", 0) for r in results) / max(total, 1)
    avg_act = sum(r.get("judge", {}).get("actionability", 0) for r in results) / max(total, 1)
    avg_cla = sum(r.get("judge", {}).get("clarity", 0) for r in results) / max(total, 1)

    summary_data = {
        "timestamp": stamp,
        "model": args.model,
        "endpoint": args.url,
        "total_evaluated": total,
        "passed": passed,
        "failed": total - passed,
        "pass_rate_pct": round((passed / max(total, 1)) * 100, 1),
        "avg_accuracy": round(avg_acc, 2),
        "avg_actionability": round(avg_act, 2),
        "avg_clarity": round(avg_cla, 2),
        "results": results,
    }

    with open(report_file, "w", encoding="utf-8") as f:
        json.dump(summary_data, f, indent=2, ensure_ascii=False)

    print("\n" + "=" * 60)
    print(f"Evaluation Complete. Results saved to: {report_file}")
    print(f"Pass Rate: {summary_data['pass_rate_pct']}% ({passed}/{total})")
    print(f"Average Scores: Accuracy={avg_acc:.2f}/5 | Actionability={avg_act:.2f}/5 | Clarity={avg_cla:.2f}/5")
    print("=" * 60)

    return 0 if (total > 0 and passed == total) else 1


if __name__ == "__main__":
    sys.exit(main())
