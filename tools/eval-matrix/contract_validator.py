#!/usr/bin/env python3
"""Mechanical output_contract validation for eval-matrix raw responses.

Checks format adherence per CPT-AXO-90017 output_contract :
  - format ∈ {FREE_TEXT, JSON, CHECKLIST, DIFF, CODE}
  - min_tokens / max_tokens (approximate word count)
  - min_items / max_items (CHECKLIST or JSON arrays)
  - schema (JSON-schema if jsonschema lib installed, else format-only)

No external API. Python 3.11+ stdlib + optional jsonschema.
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path


def approximate_token_count(text: str) -> int:
    """~4 chars per token rough heuristic (matches Anthropic SDK approx)."""
    return max(len(text) // 4, len(text.split()))


def check_format(text: str, fmt: str) -> tuple[bool, str]:
    fmt = (fmt or "FREE_TEXT").upper()
    if fmt == "FREE_TEXT":
        return True, "free text accepted"
    if fmt == "JSON":
        try:
            json.loads(text.strip())
            return True, "valid JSON"
        except json.JSONDecodeError as e:
            return False, f"invalid JSON : {e.msg} at line {e.lineno}"
    if fmt == "CHECKLIST":
        # accept markdown task lists OR plain bullet lists
        bullet_re = re.compile(r"^\s*([-*]|\d+\.)\s+(\[[ xX]\]\s+)?", re.MULTILINE)
        items = bullet_re.findall(text)
        if not items:
            return False, "no checklist items detected"
        return True, f"{len(items)} checklist items detected"
    if fmt == "DIFF":
        if "---" in text and "+++" in text and ("@@" in text or "diff --git" in text):
            return True, "unified diff markers present"
        return False, "missing unified-diff markers (---/+++ /@@)"
    if fmt == "CODE":
        # fenced ``` block
        if "```" in text:
            return True, "fenced code block present"
        # heuristic : at least one line that looks like code
        if re.search(r"\b(fn|def|class|let|var|const|public|pub)\b", text):
            return True, "code keywords detected"
        return False, "no fenced block / code keywords"
    return False, f"unknown format {fmt}"


def check_token_window(text: str, contract: dict) -> tuple[bool, str]:
    n = approximate_token_count(text)
    min_t = contract.get("min_tokens")
    max_t = contract.get("max_tokens")
    if min_t is not None and n < min_t:
        return False, f"~{n} tokens < min {min_t}"
    if max_t is not None and n > max_t:
        return False, f"~{n} tokens > max {max_t}"
    return True, f"~{n} tokens within window"


def check_item_count(text: str, contract: dict) -> tuple[bool, str]:
    fmt = (contract.get("format") or "").upper()
    min_i = contract.get("min_items")
    max_i = contract.get("max_items")
    if min_i is None and max_i is None:
        return True, "no item count gate"

    items = 0
    if fmt == "CHECKLIST":
        bullet_re = re.compile(r"^\s*([-*]|\d+\.)\s+", re.MULTILINE)
        items = len(bullet_re.findall(text))
    elif fmt == "JSON":
        try:
            data = json.loads(text.strip())
            if isinstance(data, list):
                items = len(data)
            elif isinstance(data, dict):
                items = len(data)
        except Exception:
            return False, "JSON parse failed during item count"

    if min_i is not None and items < min_i:
        return False, f"{items} items < min {min_i}"
    if max_i is not None and items > max_i:
        return False, f"{items} items > max {max_i}"
    return True, f"{items} items within window"


def check_schema(text: str, contract: dict) -> tuple[bool, str]:
    schema = contract.get("schema")
    if not schema:
        return True, "no schema gate"
    try:
        data = json.loads(text.strip())
    except Exception as e:
        return False, f"text is not parseable JSON : {e}"
    try:
        import jsonschema  # type: ignore
    except ImportError:
        return True, "jsonschema lib not installed ; schema check skipped"
    try:
        jsonschema.validate(instance=data, schema=schema)
        return True, "schema validation passed"
    except jsonschema.ValidationError as e:
        return False, f"schema violation : {e.message}"


def validate(text: str, contract: dict) -> dict:
    checks = {
        "format": check_format(text, contract.get("format", "FREE_TEXT")),
        "tokens": check_token_window(text, contract),
        "items": check_item_count(text, contract),
        "schema": check_schema(text, contract),
    }
    all_pass = all(ok for ok, _ in checks.values())
    return {
        "pass": all_pass,
        "checks": {k: {"pass": ok, "note": note} for k, (ok, note) in checks.items()},
    }


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--case", required=True, help="SKI case id (e.g. SKI-PRO-999)")
    ap.add_argument("--raw", required=True, help="Path to raw response markdown")
    args = ap.parse_args()

    here = Path(__file__).resolve().parent
    case_paths = list((here / "cases").glob(f"{args.case}_*.json"))
    if not case_paths:
        print(f"No case file matching {args.case}", file=sys.stderr)
        return 1
    case = json.loads(case_paths[0].read_text(encoding="utf-8"))
    contract = case.get("output_contract", {})

    raw_path = Path(args.raw)
    if not raw_path.exists():
        print(f"Raw file not found : {raw_path}", file=sys.stderr)
        return 1
    text = raw_path.read_text(encoding="utf-8")

    result = validate(text, contract)
    print(json.dumps(result, indent=2, ensure_ascii=False))
    return 0 if result["pass"] else 2


if __name__ == "__main__":
    sys.exit(main())
