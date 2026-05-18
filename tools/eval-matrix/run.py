#!/usr/bin/env python3
"""Axon eval-matrix orchestrator.

REQ-AXO-91585 reframed for Claude-Code self-eval. Reads SKI cases, emits the
subject prompt to stdout, blocks on stdin for the operator-pasted Claude
response, stores raw markdown, then runs mechanical contract validation +
rubric scoring.

No external API. Python 3.11+ stdlib only (jsonschema optional).
"""
from __future__ import annotations

import argparse
import json
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
CASES_DIR = HERE / "cases"
RUBRICS_DIR = HERE / "rubrics"
RAW_DIR = HERE / "raw"
REPORT_OUT = HERE / "report" / "out"

END_MARKER = "__END__"


def list_cases() -> list[Path]:
    return sorted(CASES_DIR.glob("SKI-*.json"))


def load_case(case_path: Path) -> dict:
    return json.loads(case_path.read_text(encoding="utf-8"))


def render_prompt(case: dict) -> str:
    """Build the subject prompt that the operator pastes into Claude Code."""
    ski_id = case["ski_id"]
    task_description = case.get("task_description", "")
    params = case.get("params", {})
    output_contract = case.get("output_contract", {})
    invocation_hint = case.get(
        "invocation_hint",
        f"Invoke SKI `{ski_id}` and follow its procedure for the task below.",
    )

    contract_str = json.dumps(output_contract, indent=2, ensure_ascii=False)
    params_str = json.dumps(params, indent=2, ensure_ascii=False)

    return f"""[EVAL-MATRIX-RUN: {ski_id}]

{invocation_hint}

## Task
{task_description}

## Parameters (typed sidecar per CPT-AXO-90017)
```json
{params_str}
```

## Output contract
```json
{contract_str}
```

Reply ONLY with the deliverable per the output_contract. End your response with the literal marker `{END_MARKER}` on its own line so the harness knows where to stop.
"""


def capture_response(case_id: str, run_idx: int, prompt: str, dry_run: bool) -> Path:
    """Print the prompt, capture operator-pasted response, persist raw."""
    print("=" * 80)
    print(f"# SUBJECT PROMPT — paste the following into Claude Code as a fresh user message")
    print("=" * 80)
    print(prompt)
    print("=" * 80)
    print(
        f"# PASTE Claude's response below ; end with `{END_MARKER}` on its own line."
    )
    print("=" * 80)

    raw_path = RAW_DIR / f"{case_id}_run{run_idx}.md"
    if dry_run:
        print(f"[DRY RUN] would await stdin and write to {raw_path}")
        return raw_path

    lines: list[str] = []
    try:
        while True:
            line = input()
            if line.strip() == END_MARKER:
                break
            lines.append(line)
    except EOFError:
        pass

    RAW_DIR.mkdir(parents=True, exist_ok=True)
    raw_path.write_text("\n".join(lines), encoding="utf-8")
    return raw_path


def run_case(case_path: Path, runs: int, dry_run: bool) -> list[Path]:
    case = load_case(case_path)
    case_id = case["ski_id"]
    prompt = render_prompt(case)
    out: list[Path] = []
    for i in range(1, runs + 1):
        print(f"\n>>> {case_id} run {i}/{runs} <<<\n")
        raw_path = capture_response(case_id, i, prompt, dry_run)
        out.append(raw_path)
    return out


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--list-cases", action="store_true", help="List loaded SKI cases and exit")
    ap.add_argument("--ski", help="Single SKI id to run (e.g. SKI-PRO-999)")
    ap.add_argument("--all", action="store_true", help="Run every SKI case")
    ap.add_argument("--runs", type=int, default=1, help="Runs per SKI (default 1)")
    ap.add_argument("--dry-run", action="store_true", help="Print prompts without awaiting stdin")
    args = ap.parse_args()

    cases = list_cases()
    if args.list_cases:
        print(f"Loaded {len(cases)} cases from {CASES_DIR}")
        for c in cases:
            data = load_case(c)
            print(f"  - {data['ski_id']} :: {data.get('title', '(no title)')}")
        return 0

    target: list[Path] = []
    if args.all:
        target = cases
    elif args.ski:
        target = [c for c in cases if c.stem.startswith(args.ski + "_")]
        if not target:
            print(f"No case found for SKI id {args.ski}", file=sys.stderr)
            return 1
    else:
        ap.print_help()
        return 0

    REPORT_OUT.mkdir(parents=True, exist_ok=True)
    started = time.time()
    captured: list[Path] = []
    for case_path in target:
        captured.extend(run_case(case_path, args.runs, args.dry_run))
    print(f"\nCaptured {len(captured)} raw response(s) in {time.time() - started:.1f}s")
    return 0


if __name__ == "__main__":
    sys.exit(main())
