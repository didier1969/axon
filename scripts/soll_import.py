#!/usr/bin/env python3
"""Bulk SOLL import orchestrator via MCP.

Supports:
- Markdown export restore (`--format md`) via `restore_soll`
- Structured payload import (`json|ndjson|yaml`) via:
  - `soll_apply_plan` for pillars/requirements/decisions/milestones
  - `soll_manager` for vision/concept/stakeholder/validation/relation links
  - `soll_attach_evidence` for traceability artifacts
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import urllib.request
from dataclasses import dataclass
from typing import Any


DEFAULT_MCP_URL = "http://127.0.0.1:44129/mcp"


@dataclass
class StepResult:
    tool: str
    ok: bool
    note: str
    response: dict[str, Any] | None = None


def rpc_call(url: str, tool_name: str, arguments: dict[str, Any], timeout: int = 60) -> dict[str, Any]:
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
        raw = resp.read().decode("utf-8")
    return json.loads(raw)


def extract_text(resp: dict[str, Any]) -> str:
    if "error" in resp and resp["error"] is not None:
        return json.dumps(resp["error"], ensure_ascii=False)
    result = resp.get("result", {})
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
    return "\n".join(chunks).strip()


def is_error(resp: dict[str, Any]) -> bool:
    if "error" in resp and resp["error"] is not None:
        return True
    result = resp.get("result")
    if not isinstance(result, dict):
        return False
    return bool(result.get("isError"))


def load_yaml_optional(path: str) -> Any:
    try:
        import yaml  # type: ignore
    except Exception as exc:
        raise RuntimeError(
            "YAML requested but PyYAML is unavailable. Install it or use JSON/NDJSON."
        ) from exc
    with open(path, "r", encoding="utf-8") as f:
        return yaml.safe_load(f)


def load_payload(path: str, fmt: str) -> dict[str, Any]:
    if fmt == "md":
        return {"_md_path": path}

    if fmt == "json":
        with open(path, "r", encoding="utf-8") as f:
            obj = json.load(f)
        if not isinstance(obj, dict):
            raise ValueError("JSON input must be an object.")
        return obj

    if fmt == "yaml":
        obj = load_yaml_optional(path)
        if not isinstance(obj, dict):
            raise ValueError("YAML input must be a mapping/object.")
        return obj

    if fmt == "ndjson":
        records: list[dict[str, Any]] = []
        with open(path, "r", encoding="utf-8") as f:
            for line in f:
                line = line.strip()
                if not line:
                    continue
                row = json.loads(line)
                if not isinstance(row, dict):
                    raise ValueError("Each NDJSON line must be a JSON object.")
                records.append(row)
        return {"records": records}

    raise ValueError(f"Unsupported format: {fmt}")


def norm_list(payload: dict[str, Any], key: str) -> list[dict[str, Any]]:
    raw = payload.get(key, [])
    if raw is None:
        return []
    if not isinstance(raw, list):
        raise ValueError(f"Expected list for key '{key}'")
    out: list[dict[str, Any]] = []
    for item in raw:
        if not isinstance(item, dict):
            raise ValueError(f"Expected object items in key '{key}'")
        out.append(item)
    return out


def append_step(steps: list[StepResult], tool: str, resp: dict[str, Any]) -> None:
    ok = not is_error(resp)
    note = extract_text(resp)[:400]
    steps.append(StepResult(tool=tool, ok=ok, note=note, response=resp))


def run_structured_import(
    *,
    url: str,
    payload: dict[str, Any],
    project_slug: str,
    author: str,
    dry_run: bool,
    strict: bool,
) -> list[StepResult]:
    steps: list[StepResult] = []

    # NDJSON mode: each record is {"tool":"...", "arguments":{...}}
    if "records" in payload:
        records = payload["records"]
        if not isinstance(records, list):
            raise ValueError("records must be a list")
        for idx, rec in enumerate(records):
            if not isinstance(rec, dict):
                raise ValueError(f"records[{idx}] must be an object")
            tool = str(rec.get("tool", "")).strip()
            args = rec.get("arguments", {})
            if not tool:
                raise ValueError(f"records[{idx}] missing 'tool'")
            if not isinstance(args, dict):
                raise ValueError(f"records[{idx}].arguments must be an object")
            resp = rpc_call(url, tool, args)
            append_step(steps, tool, resp)
            if strict and is_error(resp):
                return steps
        return steps

    # 1) Plan import (idempotent + revision-aware for core entities)
    if "plan" in payload and isinstance(payload["plan"], dict):
        resp = rpc_call(
            url,
            "soll_apply_plan",
            {
                "project_slug": project_slug,
                "author": author,
                "dry_run": dry_run,
                "plan": payload["plan"],
            },
        )
        append_step(steps, "soll_apply_plan", resp)
        if strict and is_error(resp):
            return steps

    # 2) Generic entities via soll_manager
    entity_keys = [
        ("visions", "vision"),
        ("concepts", "concept"),
        ("stakeholders", "stakeholder"),
        ("validations", "validation"),
        ("pillars", "pillar"),
        ("requirements", "requirement"),
        ("decisions", "decision"),
        ("milestones", "milestone"),
    ]
    for list_key, entity in entity_keys:
        for item in norm_list(payload, list_key):
            action = "update" if "id" in item and str(item.get("id", "")).strip() else "create"
            data = dict(item)
            if action == "create":
                data.setdefault("project_slug", project_slug)
            if dry_run:
                steps.append(
                    StepResult(
                        tool="soll_manager",
                        ok=True,
                        note=f"DRY-RUN would {action} {entity}",
                    )
                )
                continue
            resp = rpc_call(
                url,
                "soll_manager",
                {"action": action, "entity": entity, "data": data},
            )
            append_step(steps, "soll_manager", resp)
            if strict and is_error(resp):
                return steps

    # 3) Relations
    for rel in norm_list(payload, "relations"):
        source_id = str(rel.get("source_id", "")).strip()
        target_id = str(rel.get("target_id", "")).strip()
        if not source_id or not target_id:
            raise ValueError("relation requires source_id and target_id")
        relation_type = rel.get("relation_type")
        args_data = {"source_id": source_id, "target_id": target_id}
        if relation_type is not None:
            args_data["relation_type"] = relation_type
        if dry_run:
            steps.append(
                StepResult(
                    tool="soll_manager",
                    ok=True,
                    note=f"DRY-RUN would link {source_id}->{target_id}",
                )
            )
            continue
        resp = rpc_call(
            url,
            "soll_manager",
            {"action": "link", "entity": "requirement", "data": args_data},
        )
        append_step(steps, "soll_manager", resp)
        if strict and is_error(resp):
            return steps

    # 4) Evidence
    for ev in norm_list(payload, "evidence"):
        entity_type = str(ev.get("entity_type", "")).strip()
        entity_id = str(ev.get("entity_id", "")).strip()
        artifacts = ev.get("artifacts", [])
        if not entity_type or not entity_id:
            raise ValueError("evidence item requires entity_type and entity_id")
        if not isinstance(artifacts, list):
            raise ValueError("evidence.artifacts must be a list")
        if dry_run:
            steps.append(
                StepResult(
                    tool="soll_attach_evidence",
                    ok=True,
                    note=f"DRY-RUN would attach evidence to {entity_type}:{entity_id}",
                )
            )
            continue
        resp = rpc_call(
            url,
            "soll_attach_evidence",
            {"entity_type": entity_type, "entity_id": entity_id, "artifacts": artifacts},
        )
        append_step(steps, "soll_attach_evidence", resp)
        if strict and is_error(resp):
            return steps

    return steps


def print_summary(steps: list[StepResult]) -> int:
    ok = sum(1 for s in steps if s.ok)
    ko = sum(1 for s in steps if not s.ok)
    print(f"SOLL import summary: ok={ok} fail={ko} total={len(steps)}")
    for s in steps:
        status = "ok" if s.ok else "fail"
        print(f"- {s.tool}: {status} :: {s.note}")
    return 0 if ko == 0 else 2


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Bulk SOLL import via MCP.",
    )
    parser.add_argument("--input", required=True, help="Input file path (md/json/ndjson/yaml).")
    parser.add_argument(
        "--format",
        choices=["md", "json", "ndjson", "yaml"],
        default="json",
        help="Input format.",
    )
    parser.add_argument(
        "--url",
        default=DEFAULT_MCP_URL,
        help=f"MCP URL (default: {DEFAULT_MCP_URL})",
    )
    parser.add_argument("--project", default="AXO", help="Project slug for generated IDs.")
    parser.add_argument("--author", default=os.getenv("USER", "codex"), help="Author for revision commits.")
    parser.add_argument("--dry-run", action="store_true", help="No write operations.")
    parser.add_argument("--strict", action="store_true", help="Stop on first tool error.")
    parser.add_argument("--timeout", type=int, default=60, help="HTTP timeout seconds.")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    path = args.input
    if not os.path.isfile(path):
        print(f"Input file not found: {path}", file=sys.stderr)
        return 1

    if args.format == "md":
        if args.dry_run:
            print("SOLL import summary: ok=1 fail=0 total=1")
            print(f"- restore_soll: ok :: DRY-RUN would restore from {path}")
            return 0
        resp = rpc_call(args.url, "restore_soll", {"path": path}, timeout=args.timeout)
        steps = [StepResult("restore_soll", not is_error(resp), extract_text(resp)[:400], resp)]
        return print_summary(steps)

    payload = load_payload(path, args.format)
    steps = run_structured_import(
        url=args.url,
        payload=payload,
        project_slug=args.project,
        author=args.author,
        dry_run=args.dry_run,
        strict=args.strict,
    )
    return print_summary(steps)


if __name__ == "__main__":
    raise SystemExit(main())
