#!/usr/bin/env python3
"""Per-criterion rubric scoring for eval-matrix raw responses.

For each criterion in the rubric (weight, regex / keyword heuristics),
score 0..1 and aggregate to a weighted_score. Pass threshold defaults to 0.7.

Two modes :
  - auto : pure regex / keyword heuristics ; no human / LLM input needed
  - claude : emit a self-judge prompt for the operator to paste into Claude Code

No external API. Python 3.11+ stdlib only.
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path


def score_criterion_auto(text: str, criterion: dict) -> tuple[float, str]:
    """Auto-score using regex / keyword heuristics declared in the criterion."""
    must_match = criterion.get("must_match", [])
    must_not_match = criterion.get("must_not_match", [])
    keyword_threshold = criterion.get("keyword_threshold", 1)

    matched = 0
    missing = []
    for pattern in must_match:
        if re.search(pattern, text, re.IGNORECASE | re.MULTILINE):
            matched += 1
        else:
            missing.append(pattern)
    negative_hits = []
    for pattern in must_not_match:
        if re.search(pattern, text, re.IGNORECASE | re.MULTILINE):
            negative_hits.append(pattern)

    if not must_match and not must_not_match:
        return 1.0, "no patterns declared ; default pass"

    score = (matched / max(len(must_match), 1)) if must_match else 1.0
    # Penalise negative hits at half-weight each
    if negative_hits:
        penalty = 0.5 * (len(negative_hits) / max(len(must_not_match), 1))
        score = max(score - penalty, 0.0)

    if must_match and matched < keyword_threshold:
        score = min(score, 0.5)

    note = f"matched {matched}/{len(must_match)} ; negative_hits={len(negative_hits)} ; missing={missing[:3]}"
    return round(score, 2), note


def render_judge_prompt(case: dict, rubric: dict, raw: str) -> str:
    """Build a self-judge prompt for the operator to paste into Claude Code."""
    criteria_lines = []
    for c in rubric.get("criteria", []):
        criteria_lines.append(
            f"- **{c['name']}** (weight {c['weight']}) : {c.get('description', '(no description)')}"
        )
    criteria_block = "\n".join(criteria_lines)
    return f"""[EVAL-MATRIX-JUDGE: {case['ski_id']}]

You are evaluating a response that was produced when SKI `{case['ski_id']}` was invoked. Score each criterion 0..1 and reply with a single JSON object `{{"criterion_name": score, ...}}`. End with `__END__`.

## Rubric
{criteria_block}

## Original task
{case.get('task_description', '(no task description)')}

## Captured response
```
{raw}
```

Reply ONLY with the JSON object then `__END__`.
"""


def score_rubric(rubric: dict, text: str, mode: str) -> dict:
    threshold = rubric.get("pass_threshold", 0.7)
    criteria_scores = {}
    weighted_sum = 0.0
    total_weight = 0.0

    for c in rubric.get("criteria", []):
        weight = float(c.get("weight", 1))
        total_weight += weight
        if mode == "auto":
            s, note = score_criterion_auto(text, c)
        else:
            # claude mode : punt — operator does this manually via judge prompt
            s, note = 0.0, "claude mode : score not auto-computed ; paste judge response separately"
        criteria_scores[c["name"]] = {"score": s, "note": note, "weight": weight}
        weighted_sum += s * weight

    weighted = weighted_sum / total_weight if total_weight else 0.0
    return {
        "weighted_score": round(weighted, 3),
        "pass_threshold": threshold,
        "pass": weighted >= threshold,
        "criteria": criteria_scores,
    }


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--case", required=True, help="SKI case id (e.g. SKI-PRO-999)")
    ap.add_argument("--raw", required=True, help="Path to raw response markdown")
    ap.add_argument("--mode", choices=["auto", "claude"], default="auto")
    ap.add_argument("--print-judge-prompt", action="store_true")
    args = ap.parse_args()

    here = Path(__file__).resolve().parent
    case_paths = list((here / "cases").glob(f"{args.case}_*.json"))
    if not case_paths:
        print(f"No case file matching {args.case}", file=sys.stderr)
        return 1
    case = json.loads(case_paths[0].read_text(encoding="utf-8"))

    rubric_paths = list((here / "rubrics").glob(f"{args.case}.json"))
    if not rubric_paths:
        print(f"No rubric file matching {args.case}", file=sys.stderr)
        return 1
    rubric = json.loads(rubric_paths[0].read_text(encoding="utf-8"))

    raw_path = Path(args.raw)
    if not raw_path.exists():
        print(f"Raw file not found : {raw_path}", file=sys.stderr)
        return 1
    text = raw_path.read_text(encoding="utf-8")

    if args.print_judge_prompt:
        print(render_judge_prompt(case, rubric, text))
        return 0

    result = score_rubric(rubric, text, args.mode)
    print(json.dumps(result, indent=2, ensure_ascii=False))
    return 0 if result["pass"] else 2


if __name__ == "__main__":
    sys.exit(main())
