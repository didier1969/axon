#!/usr/bin/env python3
"""Deterministic qualification for the hidden retrieve_context MCP surface."""

from __future__ import annotations

import argparse
import json
import statistics
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any


PROJECT_ROOT = Path("/home/dstadel/projects/axon")
DEFAULT_URL = "http://127.0.0.1:44129/mcp"
DEFAULT_CORPUS = PROJECT_ROOT / "scripts" / "retrieval_context_cases.json"

DEFAULT_THRESHOLDS = {
    "route_accuracy": 0.90,
    "direct_anchor_hit_rate": 0.90,
    "grounded_chunk_hit_rate": 0.80,
    "citation_file_hit_rate": 0.80,
    "useful_context_ratio": 0.75,
    "soll_relevance_hit_rate": 0.80,
    "p95_latency_ms": 1500,
}


def rpc_call(url: str, payload: dict[str, Any], timeout: int) -> dict[str, Any]:
    data = json.dumps(payload).encode("utf-8")
    req = urllib.request.Request(
        url,
        data=data,
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(req, timeout=timeout) as resp:
        return json.loads(resp.read().decode("utf-8"))


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


def load_cases(path: Path) -> list[dict[str, Any]]:
    payload = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(payload, dict):
        raise ValueError("retrieval corpus must be a JSON object")
    cases = payload.get("cases")
    if not isinstance(cases, list) or not cases:
        raise ValueError("retrieval corpus must contain a non-empty 'cases' array")
    normalized: list[dict[str, Any]] = []
    for idx, case in enumerate(cases, start=1):
        if not isinstance(case, dict):
            raise ValueError(f"retrieval case #{idx} must be an object")
        if not isinstance(case.get("id"), str) or not case["id"].strip():
            raise ValueError(f"retrieval case #{idx} has invalid id")
        if not isinstance(case.get("question"), str) or not case["question"].strip():
            raise ValueError(f"retrieval case #{idx} has invalid question")
        normalized.append(case)
    return normalized


def json_text(value: Any) -> str:
    return json.dumps(value, ensure_ascii=False).lower()


def term_hit(payload_text: str, expected_terms: list[str]) -> bool:
    if not expected_terms:
        return True
    lowered_terms = [term.lower() for term in expected_terms]
    return any(term in payload_text for term in lowered_terms)


def file_hit(payload_text: str, expected_paths: list[str]) -> bool:
    if not expected_paths:
        return True
    lowered_paths = [path.lower() for path in expected_paths]
    return any(path in payload_text for path in lowered_paths)


def expected_soll_hit(packet: dict[str, Any], expected_ids: list[str]) -> bool:
    if not expected_ids:
        return True
    payload_text = json_text(packet.get("relevant_soll_entities", []))
    return any(expected_id.lower() in payload_text for expected_id in expected_ids)


def count_relevant_items(packet: dict[str, Any], expected_terms: list[str], expected_paths: list[str], expected_soll_ids: list[str]) -> tuple[int, int]:
    total = 0
    relevant = 0
    expected_text = [item.lower() for item in [*expected_terms, *expected_paths, *expected_soll_ids] if item]
    for key in ("direct_evidence", "supporting_chunks", "structural_neighbors", "relevant_soll_entities"):
        items = packet.get(key, [])
        if not isinstance(items, list):
            continue
        for item in items:
            total += 1
            item_text = json_text(item)
            if not expected_text or any(token in item_text for token in expected_text):
                relevant += 1
    return relevant, total


def percentile(values: list[int], q: float) -> int:
    if not values:
        return 0
    ordered = sorted(values)
    if len(ordered) == 1:
        return ordered[0]
    index = max(0, min(len(ordered) - 1, int(round((len(ordered) - 1) * q))))
    return ordered[index]


def evaluate_direct_anchor_hit(packet: dict[str, Any], expected_terms: list[str]) -> bool:
    if not expected_terms:
        return True
    direct_evidence = packet.get("direct_evidence", [])
    if not isinstance(direct_evidence, list):
        return False
    return term_hit(json_text(direct_evidence), expected_terms)


def run_case(url: str, timeout: int, case: dict[str, Any], default_project: str) -> dict[str, Any]:
    project = case.get("project") or default_project
    payload = {
        "jsonrpc": "2.0",
        "id": case["id"],
        "method": "tools/call",
        "params": {
            "name": "retrieve_context",
            "arguments": {
                "question": case["question"],
                "project": project,
                "token_budget": int(case.get("token_budget", 1200)),
            },
        },
    }
    started = time.time()
    response = rpc_call(url, payload, timeout)
    duration_ms = int((time.time() - started) * 1000)
    if response.get("error") is not None:
        raise RuntimeError(json.dumps(response["error"], ensure_ascii=False))

    result = response.get("result")
    if not isinstance(result, dict):
        raise RuntimeError("retrieve_context did not return result object")
    data = result.get("data")
    text = extract_text(response).lower()
    if not isinstance(data, dict) and "unavailable in runtime mode" in text:
        return {
            "id": case["id"],
            "project": project,
            "question": case["question"],
            "duration_ms": duration_ms,
            "planner_route": None,
            "route_hit": False,
            "direct_anchor_hit": False,
            "grounded_chunk_hit": False,
            "citation_file_hit": False,
            "soll_relevance_hit": False,
            "relevant_items": 0,
            "total_items": 0,
            "skipped": True,
            "skip_reason": "retrieve_context unavailable in current runtime mode",
            "packet_excerpt": {},
            "raw_text_excerpt": extract_text(response)[:500],
            "packet_text": "",
        }
    if not isinstance(data, dict):
        raise RuntimeError("retrieve_context did not return result.data")
    planner = data.get("planner")
    packet = data.get("packet")
    if not isinstance(planner, dict) or not isinstance(packet, dict):
        raise RuntimeError("retrieve_context did not return planner and packet")

    expected_route = str(case.get("expected_route", "") or "")
    expected_terms = [
        term for term in case.get("expected_anchor_terms", []) if isinstance(term, str) and term
    ]
    expected_paths = [
        path for path in case.get("expected_file_paths", []) if isinstance(path, str) and path
    ]
    expected_soll_ids = [
        soll_id for soll_id in case.get("expected_soll_ids", []) if isinstance(soll_id, str) and soll_id
    ]

    packet_text = json_text(packet)
    route_hit = not expected_route or planner.get("route") == expected_route
    direct_anchor_hit = evaluate_direct_anchor_hit(packet, expected_terms)
    citation_hit = file_hit(json_text(packet), expected_paths) if expected_paths else None
    chunk_hit = True
    if case.get("require_supporting_chunks") or expected_terms or expected_paths:
        supporting_chunks = packet.get("supporting_chunks", [])
        chunk_hit = isinstance(supporting_chunks, list) and supporting_chunks != [] and (
            term_hit(json_text(supporting_chunks), expected_terms)
            or file_hit(json_text(supporting_chunks), expected_paths)
        )
    soll_hit = expected_soll_hit(packet, expected_soll_ids)
    relevant_items, total_items = count_relevant_items(packet, expected_terms, expected_paths, expected_soll_ids)

    allow_missing = bool(case.get("allow_missing", False))
    skipped = False
    skip_reason = ""
    if allow_missing and not direct_anchor_hit and not bool(citation_hit) and not soll_hit:
        skipped = True
        skip_reason = "expected live fixture missing from current IST/runtime"

    return {
        "id": case["id"],
        "project": project,
        "question": case["question"],
        "duration_ms": duration_ms,
        "planner_route": planner.get("route"),
        "route_hit": route_hit,
        "direct_anchor_hit": direct_anchor_hit,
        "grounded_chunk_hit": chunk_hit,
        "citation_file_hit": citation_hit,
        "soll_relevance_hit": soll_hit,
        "relevant_items": relevant_items,
        "total_items": total_items,
        "skipped": skipped,
        "skip_reason": skip_reason,
        "planner": planner,
        "packet_excerpt": {
            "answer_sketch": packet.get("answer_sketch"),
            "direct_evidence": packet.get("direct_evidence", [])[:2],
            "supporting_chunks": packet.get("supporting_chunks", [])[:2],
            "relevant_soll_entities": packet.get("relevant_soll_entities", [])[:2],
            "missing_evidence": packet.get("missing_evidence", []),
        },
        "raw_text_excerpt": extract_text(response)[:500],
        "packet_text": packet_text[:1200],
    }


def metric_rate(cases: list[dict[str, Any]], key: str) -> float | None:
    applicable = [case for case in cases if not case["skipped"]]
    if not applicable:
        return None
    hits = sum(1 for case in applicable if case.get(key))
    return round(hits / len(applicable), 4)


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description="Qualify retrieve_context on a deterministic corpus.")
    parser.add_argument("--url", default=DEFAULT_URL, help=f"MCP URL (default: {DEFAULT_URL})")
    parser.add_argument("--project", default="AXO", help="Fallback project scope when a corpus case omits project")
    parser.add_argument("--corpus", default=str(DEFAULT_CORPUS), help=f"Corpus JSON path (default: {DEFAULT_CORPUS})")
    parser.add_argument("--timeout", type=int, default=20, help="Per-request timeout in seconds")
    parser.add_argument("--json-out", default="", help="Optional JSON output path")
    args = parser.parse_args(argv)

    corpus_path = Path(args.corpus)
    try:
        cases = load_cases(corpus_path)
    except Exception as exc:
        print(f"FATAL: failed to load retrieval corpus {corpus_path}: {type(exc).__name__}: {exc}")
        return 2

    try:
        rpc_call(
            args.url,
            {
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2024-11-05",
                    "clientInfo": {"name": "qualify_retrieval_context", "version": "1.0"},
                    "capabilities": {},
                },
            },
            args.timeout,
        )
    except Exception as exc:
        print(f"FATAL: MCP initialize failed: {type(exc).__name__}: {exc}")
        return 2

    results: list[dict[str, Any]] = []
    for case in cases:
        try:
            results.append(run_case(args.url, args.timeout, case, args.project))
        except (urllib.error.URLError, TimeoutError, OSError, json.JSONDecodeError, RuntimeError) as exc:
            results.append(
                {
                    "id": case["id"],
                    "project": case.get("project") or args.project,
                    "question": case["question"],
                    "duration_ms": 0,
                    "planner_route": None,
                    "route_hit": False,
                    "direct_anchor_hit": False,
                    "grounded_chunk_hit": False,
                    "citation_file_hit": False,
                    "soll_relevance_hit": False,
                    "relevant_items": 0,
                    "total_items": 0,
                    "skipped": False,
                    "skip_reason": "",
                    "error": f"{type(exc).__name__}: {exc}",
                }
            )

    evaluated = [result for result in results if not result["skipped"]]
    skipped = [result for result in results if result["skipped"]]
    route_accuracy = metric_rate(results, "route_hit")
    direct_anchor_hit_rate = metric_rate(results, "direct_anchor_hit")
    grounded_chunk_hit_rate = metric_rate(
        [result for result in results if "grounded_chunk_hit" in result], "grounded_chunk_hit"
    )
    citation_file_hit_rate = metric_rate(
        [result for result in results if result.get("citation_file_hit") is not None], "citation_file_hit"
    )
    soll_applicable = [
        result
        for result in results
        if not result["skipped"]
        and any(
            isinstance(case.get("expected_soll_ids"), list) and case.get("id") == result["id"]
            for case in cases
        )
    ]
    soll_relevance_hit_rate = metric_rate(soll_applicable, "soll_relevance_hit") if soll_applicable else None
    total_relevant = sum(result.get("relevant_items", 0) for result in evaluated)
    total_items = sum(result.get("total_items", 0) for result in evaluated)
    useful_context_ratio = round(total_relevant / total_items, 4) if total_items else 0.0
    latencies = [int(result.get("duration_ms", 0)) for result in evaluated]
    latency_summary = {
        "p50": int(statistics.median(latencies)) if latencies else 0,
        "p95": percentile(latencies, 0.95),
        "max": max(latencies) if latencies else 0,
    }

    metrics = {
        "route_accuracy": route_accuracy,
        "direct_anchor_hit_rate": direct_anchor_hit_rate,
        "grounded_chunk_hit_rate": grounded_chunk_hit_rate,
        "citation_file_hit_rate": citation_file_hit_rate,
        "useful_context_ratio": useful_context_ratio,
        "soll_relevance_hit_rate": soll_relevance_hit_rate,
    }

    threshold_failures = []
    for key, threshold in DEFAULT_THRESHOLDS.items():
        if key == "p95_latency_ms":
            value = latency_summary["p95"]
            if value > threshold:
                threshold_failures.append(f"{key}={value} > {threshold}")
            continue
        value = metrics.get(key)
        if value is None:
            continue
        if value < threshold:
            threshold_failures.append(f"{key}={value} < {threshold}")

    verdict = "pass"
    if threshold_failures:
        verdict = "fail"
    elif skipped:
        verdict = "warn"

    summary = {
        "corpus": str(corpus_path),
        "evaluated_cases": len(evaluated),
        "skipped_cases": len(skipped),
        "metrics": metrics,
        "latency_ms": latency_summary,
        "thresholds": DEFAULT_THRESHOLDS,
        "threshold_failures": threshold_failures,
        "verdict": verdict,
    }

    print("retrieve_context qualification")
    print(f"corpus={corpus_path}")
    print(f"evaluated={len(evaluated)} skipped={len(skipped)} verdict={verdict}")
    print(json.dumps(summary, indent=2, ensure_ascii=False))

    if args.json_out:
        payload = {
            "summary": summary,
            "results": results,
        }
        Path(args.json_out).write_text(json.dumps(payload, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")

    if verdict == "fail":
        return 1
    if verdict == "warn":
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
