#!/usr/bin/env python3
"""Aggregate eval-matrix raw responses into a markdown matrix.

Reads cases + raws + rubrics, runs contract_validator + rubric_scorer for
each (SKI, run), produces a markdown table : rows=SKIs, cols=runs, cells=
pass/fail with weighted_score.

No external API. Python 3.11+ stdlib only.
"""
from __future__ import annotations

import argparse
import json
import sys
from datetime import datetime, timezone
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
sys.path.insert(0, str(ROOT))
from contract_validator import validate as validate_contract  # noqa: E402
from rubric_scorer import score_rubric  # noqa: E402


def collect_runs(ski_id: str) -> list[Path]:
    raw_dir = ROOT / "raw"
    if not raw_dir.exists():
        return []
    return sorted(raw_dir.glob(f"{ski_id}_run*.md"))


def aggregate(mode: str) -> dict:
    cases_dir = ROOT / "cases"
    rubrics_dir = ROOT / "rubrics"
    matrix = {}
    for case_path in sorted(cases_dir.glob("SKI-*.json")):
        case = json.loads(case_path.read_text(encoding="utf-8"))
        ski_id = case["ski_id"]
        contract = case.get("output_contract", {})
        rubric_path = rubrics_dir / f"{ski_id}.json"
        rubric = json.loads(rubric_path.read_text(encoding="utf-8")) if rubric_path.exists() else None
        runs = collect_runs(ski_id)
        row = {"ski_id": ski_id, "title": case.get("title", ""), "runs": []}
        for raw_path in runs:
            text = raw_path.read_text(encoding="utf-8")
            contract_res = validate_contract(text, contract)
            rubric_res = score_rubric(rubric, text, mode) if rubric else {"weighted_score": None, "pass": None}
            run_pass = contract_res["pass"] and (rubric_res.get("pass") is not False)
            row["runs"].append({
                "raw": str(raw_path.relative_to(ROOT)),
                "contract_pass": contract_res["pass"],
                "rubric_score": rubric_res.get("weighted_score"),
                "rubric_pass": rubric_res.get("pass"),
                "overall_pass": run_pass,
            })
        matrix[ski_id] = row
    return matrix


def render_markdown(matrix: dict) -> str:
    when = datetime.now(tz=timezone.utc).isoformat(timespec="seconds")
    lines = [
        f"# Eval matrix — {when}",
        "",
        "Single-LLM Claude-Code self-eval per REQ-AXO-91585 reframe.",
        "Bias caveat : same-model judge-subject produces positive bias. Read pass_rate as a methodology-compliance signal, not a comparative ranking.",
        "",
        "| SKI | Title | Runs | Pass-rate | Mean score |",
        "|---|---|---:|---:|---:|",
    ]
    for ski_id, row in matrix.items():
        n = len(row["runs"])
        if n == 0:
            lines.append(f"| {ski_id} | {row['title']} | 0 | — | — |")
            continue
        passes = sum(1 for r in row["runs"] if r.get("overall_pass"))
        scores = [r["rubric_score"] for r in row["runs"] if r.get("rubric_score") is not None]
        mean = sum(scores) / len(scores) if scores else None
        mean_s = f"{mean:.2f}" if mean is not None else "—"
        lines.append(f"| {ski_id} | {row['title']} | {n} | {passes}/{n} | {mean_s} |")
    lines.append("")
    lines.append("## Per-run detail")
    for ski_id, row in matrix.items():
        lines.append(f"\n### {ski_id} — {row['title']}")
        if not row["runs"]:
            lines.append("(no runs captured)")
            continue
        lines.append("| Run | Contract | Rubric | Overall |")
        lines.append("|---|---|---:|---|")
        for i, r in enumerate(row["runs"], 1):
            c = "✓" if r["contract_pass"] else "✗"
            rb = f"{r['rubric_score']:.2f}" if r["rubric_score"] is not None else "—"
            ov = "✓" if r["overall_pass"] else "✗"
            lines.append(f"| {i} ({r['raw']}) | {c} | {rb} | {ov} |")
    return "\n".join(lines)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--mode", choices=["auto", "claude"], default="auto")
    ap.add_argument("--latest", action="store_true", help="Print rendered matrix to stdout")
    ap.add_argument("--out", help="Write matrix to this path (default report/out/matrix-<ts>.md)")
    args = ap.parse_args()

    matrix = aggregate(args.mode)
    md = render_markdown(matrix)

    if args.latest:
        print(md)
        return 0

    out_dir = HERE / "out"
    out_dir.mkdir(parents=True, exist_ok=True)
    ts = datetime.now(tz=timezone.utc).strftime("%Y-%m-%dT%H-%M-%SZ")
    out_path = Path(args.out) if args.out else out_dir / f"matrix-{ts}.md"
    out_path.write_text(md, encoding="utf-8")
    print(f"Matrix written : {out_path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
