#!/usr/bin/env python3
"""Axon eval-matrix orchestrator.

REQ-AXO-91585 reframed for Claude-Code self-eval. Two modes:

1. **Interactive** (default, legacy) — Reads SKI cases, emits the subject
   prompt to stdout, blocks on stdin for the operator-pasted Claude
   response, stores raw markdown, then runs mechanical contract validation
   + rubric scoring.

2. **Batch** (REQ-AXO-91586 protocol δ, added 2026-05-19) — Pre-export
   prompt templates per (case × condition × run) via `--export-prompts`,
   operator collects responses by pasting prompts into N separate Claude
   sessions (one per condition), saves replies under
   `{DIR}/{case_id}__{condition}__{run_n}.txt`. Then re-invoke with
   `--batch-input {DIR} --output {OUT}` to score everything mechanically.

No external API. Python 3.11+ stdlib only (jsonschema optional).

Conditions (REQ-AXO-91586 design) :
  - `bare` : LLM with no skills, no MCP (control)
  - `axon` : Claude Code with Axon MCP enabled (subject)
  - `sota` : Claude Code with filesystem SKILL.md only (baseline)

Per-case × per-condition × per-run output JSON :
  {
    "ski_id": "SKI-PRO-999",
    "condition": "axon",
    "run": 1,
    "response_md": "<raw>",
    "contract_pass": bool,
    "contract_violations": [...],
    "rubric_score": {dim: int, ...},
    "rubric_total": int
  }
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
DEFAULT_CONDITIONS = ("bare", "axon", "sota")


def list_cases() -> list[Path]:
    return sorted(CASES_DIR.glob("SKI-*.json"))


def load_case(case_path: Path) -> dict:
    return json.loads(case_path.read_text(encoding="utf-8"))


def render_prompt(case: dict, condition: str | None = None, run_idx: int | None = None) -> str:
    """Build the subject prompt that the operator pastes into Claude Code.

    `condition` and `run_idx` only appear in the header tag so each Claude
    session gets a slightly unique marker (helps the operator track which
    paste belongs to which condition without leaking conditional content
    that would bias the subject).
    """
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

    header = f"[EVAL-MATRIX-RUN: {ski_id}"
    if condition is not None:
        header += f" cond={condition}"
    if run_idx is not None:
        header += f" run={run_idx}"
    header += "]"

    return f"""{header}

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


# ---------- Interactive mode (legacy) ----------


def capture_response_interactive(case_id: str, run_idx: int, prompt: str, dry_run: bool) -> Path:
    """Print the prompt, capture operator-pasted response from stdin, persist raw."""
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


def run_case_interactive(case_path: Path, runs: int, dry_run: bool) -> list[Path]:
    case = load_case(case_path)
    case_id = case["ski_id"]
    prompt = render_prompt(case)
    out: list[Path] = []
    for i in range(1, runs + 1):
        print(f"\n>>> {case_id} run {i}/{runs} <<<\n")
        raw_path = capture_response_interactive(case_id, i, prompt, dry_run)
        out.append(raw_path)
    return out


# ---------- Batch mode (REQ-AXO-91586 protocol δ) ----------


def export_prompts(target_cases: list[Path], conditions: list[str], runs: int, export_dir: Path) -> int:
    """Emit one prompt file per (case, condition, run) under {export_dir}/prompts/.

    Operator pastes each `.prompt.txt` into the corresponding Claude session
    (one Claude session per condition) and saves the reply as
    `{export_dir}/{case_id}__{condition}__{run}.txt` (drop the `prompt.`
    prefix). The batch input directory is the same `export_dir`.
    """
    prompts_dir = export_dir / "prompts"
    prompts_dir.mkdir(parents=True, exist_ok=True)
    count = 0
    for case_path in target_cases:
        case = load_case(case_path)
        case_id = case["ski_id"]
        for cond in conditions:
            for run_idx in range(1, runs + 1):
                prompt = render_prompt(case, condition=cond, run_idx=run_idx)
                out_path = prompts_dir / f"{case_id}__{cond}__{run_idx}.prompt.txt"
                out_path.write_text(prompt, encoding="utf-8")
                count += 1
    return count


def discover_batch_responses(batch_input_dir: Path) -> list[tuple[str, str, int, Path]]:
    """Enumerate operator-collected response files in batch_input_dir.

    Expected filename : `{ski_id}__{condition}__{run_idx}.txt`. Files under
    the `prompts/` subdirectory are skipped (those are the prompts emitted
    by --export-prompts, not the responses).

    Returns a list of (ski_id, condition, run_idx, path) tuples sorted by
    (ski_id, condition, run_idx) for deterministic processing.
    """
    items: list[tuple[str, str, int, Path]] = []
    for path in batch_input_dir.glob("*.txt"):
        # Skip the prompts subtree explicitly (glob is non-recursive but
        # belt-and-suspenders for future cases).
        if "prompts" in path.parts:
            continue
        stem = path.stem
        parts = stem.split("__")
        if len(parts) != 3:
            print(f"[WARN] skip {path.name} : expected `{{ski}}__{{cond}}__{{run}}.txt`", file=sys.stderr)
            continue
        ski_id, condition, run_str = parts
        try:
            run_idx = int(run_str)
        except ValueError:
            print(f"[WARN] skip {path.name} : run segment `{run_str}` not int", file=sys.stderr)
            continue
        items.append((ski_id, condition, run_idx, path))
    items.sort(key=lambda t: (t[0], t[1], t[2]))
    return items


def load_response(path: Path) -> str:
    """Read response markdown ; strip trailing END_MARKER if present."""
    raw = path.read_text(encoding="utf-8")
    lines = raw.splitlines()
    while lines and lines[-1].strip() == END_MARKER:
        lines.pop()
    return "\n".join(lines)


def score_response(case: dict, response_md: str) -> dict:
    """Run contract validation + rubric scoring on a captured response.

    Returns a dict combining both signal sources. Failure modes are
    surfaced as `contract_pass=False` + `contract_violations=[reason]`
    so the operator can re-inspect raw markdown without the harness
    crashing.
    """
    # Lazy import : contract_validator + rubric_scorer are sibling modules.
    try:
        from contract_validator import validate as validate_contract  # type: ignore
    except ImportError:
        validate_contract = None
    try:
        from rubric_scorer import score as score_rubric  # type: ignore
    except ImportError:
        score_rubric = None

    output_contract = case.get("output_contract", {})
    rubric_path = RUBRICS_DIR / f"{case['ski_id']}.json"
    rubric = json.loads(rubric_path.read_text(encoding="utf-8")) if rubric_path.exists() else {}

    contract_result = {"pass": True, "violations": []}
    if validate_contract is not None and output_contract:
        try:
            contract_result = validate_contract(response_md, output_contract)
        except Exception as e:  # noqa: BLE001
            contract_result = {"pass": False, "violations": [f"validator_error:{e}"]}

    rubric_result = {"score": {}, "total": 0}
    if score_rubric is not None and rubric:
        try:
            rubric_result = score_rubric(response_md, rubric)
        except Exception as e:  # noqa: BLE001
            rubric_result = {"score": {}, "total": 0, "error": str(e)}

    return {
        "contract_pass": contract_result.get("pass", False),
        "contract_violations": contract_result.get("violations", []),
        "rubric_score": rubric_result.get("score", {}),
        "rubric_total": rubric_result.get("total", 0),
    }


def run_batch(batch_input_dir: Path, output_dir: Path) -> tuple[int, int]:
    """Score every response file in batch_input_dir. Returns (processed, errors)."""
    cases_by_id = {load_case(p)["ski_id"]: load_case(p) for p in list_cases()}
    output_dir.mkdir(parents=True, exist_ok=True)

    items = discover_batch_responses(batch_input_dir)
    processed = 0
    errors = 0
    for ski_id, condition, run_idx, path in items:
        case = cases_by_id.get(ski_id)
        if case is None:
            print(f"[ERROR] {path.name} : no case loaded for ski_id={ski_id}", file=sys.stderr)
            errors += 1
            continue
        response_md = load_response(path)
        result = score_response(case, response_md)
        record = {
            "ski_id": ski_id,
            "condition": condition,
            "run": run_idx,
            "response_md": response_md,
            **result,
        }
        out_name = f"{ski_id}__{condition}__{run_idx}.json"
        (output_dir / out_name).write_text(
            json.dumps(record, indent=2, ensure_ascii=False), encoding="utf-8"
        )
        processed += 1
        print(
            f"[OK] {ski_id} cond={condition} run={run_idx} "
            f"contract={'PASS' if result['contract_pass'] else 'FAIL'} "
            f"rubric_total={result['rubric_total']}"
        )
    return processed, errors


# ---------- Main entrypoint ----------


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--list-cases", action="store_true", help="List loaded SKI cases and exit")
    ap.add_argument("--ski", help="Single SKI id to run (e.g. SKI-PRO-999)")
    ap.add_argument("--all", action="store_true", help="Run every SKI case")
    ap.add_argument("--runs", type=int, default=1, help="Runs per SKI (default 1)")
    ap.add_argument("--dry-run", action="store_true", help="Print prompts without awaiting stdin")
    ap.add_argument(
        "--conditions",
        default=",".join(DEFAULT_CONDITIONS),
        help=f"Comma-separated conditions for batch mode (default `{','.join(DEFAULT_CONDITIONS)}`)",
    )
    ap.add_argument(
        "--export-prompts",
        metavar="DIR",
        help="Write one .prompt.txt per (case × condition × run) under DIR/prompts/ and exit",
    )
    ap.add_argument(
        "--batch-input",
        metavar="DIR",
        help="Score pre-collected responses in DIR/{ski}__{cond}__{run}.txt instead of awaiting stdin",
    )
    ap.add_argument(
        "--output",
        metavar="DIR",
        help="Output directory for scored JSON records (batch mode). Default: report/out/",
    )
    args = ap.parse_args()

    cases = list_cases()
    if args.list_cases:
        print(f"Loaded {len(cases)} cases from {CASES_DIR}")
        for c in cases:
            data = load_case(c)
            print(f"  - {data['ski_id']} :: {data.get('title', '(no title)')}")
        return 0

    conditions = [c.strip() for c in args.conditions.split(",") if c.strip()]
    if not conditions:
        conditions = list(DEFAULT_CONDITIONS)

    # Resolve target cases (used by --all / --ski / --export-prompts)
    target: list[Path] = []
    if args.all:
        target = cases
    elif args.ski:
        target = [c for c in cases if c.stem.startswith(args.ski + "_")]
        if not target and (args.export_prompts or not args.batch_input):
            print(f"No case found for SKI id {args.ski}", file=sys.stderr)
            return 1

    # Mode A : export prompt templates and exit
    if args.export_prompts:
        if not target:
            target = cases  # default to all if no --ski / --all passed alongside
        export_dir = Path(args.export_prompts).resolve()
        n = export_prompts(target, conditions, args.runs, export_dir)
        print(
            f"Exported {n} prompt files ({len(target)} cases × {len(conditions)} conditions × {args.runs} runs) "
            f"under {export_dir / 'prompts'}/"
        )
        print(
            f"\nNext: paste each prompts/{{ski}}__{{cond}}__{{run}}.prompt.txt into a Claude session "
            f"for that condition, save the reply as {export_dir}/{{ski}}__{{cond}}__{{run}}.txt, "
            f"then re-run with `--batch-input {export_dir} --output OUT_DIR`."
        )
        return 0

    # Mode B : score collected responses
    if args.batch_input:
        batch_dir = Path(args.batch_input).resolve()
        if not batch_dir.is_dir():
            print(f"--batch-input DIR not found: {batch_dir}", file=sys.stderr)
            return 1
        output_dir = Path(args.output).resolve() if args.output else REPORT_OUT
        started = time.time()
        processed, errors = run_batch(batch_dir, output_dir)
        elapsed = time.time() - started
        print(
            f"\nBatch scored {processed} response(s) ({errors} error(s)) "
            f"in {elapsed:.1f}s — output in {output_dir}/"
        )
        return 0 if errors == 0 else 1

    # Mode C : legacy interactive
    if not target:
        ap.print_help()
        return 0

    REPORT_OUT.mkdir(parents=True, exist_ok=True)
    started = time.time()
    captured: list[Path] = []
    for case_path in target:
        captured.extend(run_case_interactive(case_path, args.runs, args.dry_run))
    print(f"\nCaptured {len(captured)} raw response(s) in {time.time() - started:.1f}s")
    return 0


if __name__ == "__main__":
    sys.exit(main())
