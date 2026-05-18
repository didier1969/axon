# Axon Eval Matrix — Claude-Code-driven self-eval

REQ-AXO-91585/86/87 reframed for the no-external-API world : Claude (the Claude Code session
that runs Axon as its primary tool) **is** the LLM under test AND the judge. No Anthropic /
OpenAI / Google API keys are required to operate this harness.

## Why this exists

The Axon methodology platform (SKI-PRO-N + PRT-PRO-N + GUI-PRO-N) defines how an LLM should
behave on engineering tasks. We need empirical evidence that an LLM **does** behave that way
when asked to follow each skill. The classical answer is a cross-LLM benchmark — fire each
task at Claude / GPT-4o / Gemini, score the responses, build a compliance matrix. That
requires API keys + budget + an external orchestrator.

This harness takes a different path : the *same* Claude that runs in Claude Code (the
operator's daily development assistant) sits at one end (subject) and the other end (judge),
both via interactive prompts driven by a Python orchestrator that has no API access. The
trade-off is documented self-eval bias ; the gain is **zero API cost** and **immediate
observability of the methodology surface working in its real environment**.

## Architecture

```
tools/eval-matrix/
  README.md                       # This file
  run.py                          # Orchestrator (Python 3.11+, no external deps required)
  contract_validator.py           # Mechanical validation : output_contract format checks
  rubric_scorer.py                # Per-criterion rubric scoring (LLM-judge OR human-rated)
  cases/                          # 1 JSON file per SKI-PRO-N (8 minimum)
    SKI-PRO-999_red-green-refactor.json
    SKI-PRO-1000_grill-design-tree.json
    ...
  rubrics/                        # Rubric definitions per SKI
    SKI-PRO-999.json
    ...
  raw/                            # Captured Claude responses (gitignored)
    SKI-PRO-999_run1.md
  report/
    matrix.py                     # Aggregate per (SKI, run, criterion)
    render.py                     # Markdown matrix + cost telemetry
    out/                          # Generated reports (timestamped, gitignored)
```

## How to run

The harness is interactive. You drive it from Claude Code itself.

1. **Setup** (one-shot) :
   ```
   cd tools/eval-matrix
   python3 run.py --list-cases   # confirm 8 cases load
   ```

2. **Single SKI run** :
   ```
   python3 run.py --ski SKI-PRO-999 --runs 1
   ```
   The orchestrator prints a prompt block. Copy-paste it into Claude Code as a fresh user
   message. Claude responds. Copy-paste the response back into the orchestrator's stdin
   (the script blocks on `input()` until you finish with an EOF marker `__END__`).

3. **Full sweep** :
   ```
   python3 run.py --all --runs 5
   ```
   Iterates every SKI × 5 runs. Stores raw responses in `raw/<SKI>_runN.md`. Generates
   `report/out/matrix-<timestamp>.md` with pass/fail per criterion.

4. **Inspect** :
   ```
   python3 report/render.py --latest
   ```

## Scoring pipeline

For each (SKI, run) :

1. **Mechanical validation** (`contract_validator.py`) :
   - Output format matches `output_contract.format` (FREE_TEXT / JSON / CHECKLIST / DIFF / CODE)
   - Min/max token / item counts respected
   - If JSON-schema present : validate against it (uses `jsonschema` lib if installed,
     else falls back to format-only)

2. **Rubric scoring** (`rubric_scorer.py`) :
   - For each criterion in the rubric (weighted 0..1), produce a score 0..1
   - Two modes : `--mode auto` uses simple regex / keyword heuristics ; `--mode claude`
     emits a second prompt asking Claude to self-judge against the rubric (the operator
     drives this same as the subject prompt)
   - Aggregate : `weighted_score = sum(weight_i * score_i)`
   - Pass threshold : `weighted_score >= 0.7` (configurable per rubric)

3. **Aggregate** (`report/matrix.py`) :
   - Per (SKI, run) : mech_pass (bool), rubric_score (0..1), pass (bool)
   - Per SKI across runs : pass_rate, mean rubric_score, std-dev
   - Output : markdown matrix + JSON snapshot for diffing

## Bias caveat

Same-LLM judge-subject inherently produces positive bias (Claude judges Claude favourably).
This is acceptable for **methodology compliance verification** (does Claude follow the skill
when asked ?) but NOT for **comparative LLM ranking** (which model is better ?). For the
latter, a cross-LLM harness with API keys is required (deferred to a future REQ when
operator provides keys + budget).

To partly counter the bias :
- Rubric criteria are mechanical wherever possible (regex / keyword presence / count)
- The judge prompt explicitly lists failure modes the subject MUST NOT exhibit
- 5 runs per SKI give a variance signal (if Claude is "lucky", the std-dev surfaces it)

## Acceptance (REQ-AXO-91585 reframed)

- [ ] 8 SKI-PRO-N cases JSON authored
- [ ] 8 rubrics JSON authored
- [ ] `python3 run.py --list-cases` reports 8 cases without error
- [ ] `python3 run.py --ski SKI-PRO-999 --runs 1 --dry-run` prints the prompt block without invoking input()
- [ ] `python3 contract_validator.py --case SKI-PRO-999 --raw raw/SKI-PRO-999_run1.md` passes/fails deterministically
- [ ] `python3 report/render.py --latest` produces a readable markdown matrix
- [ ] README.md documents the doctrine "no external API ; Claude-Code self-eval"
- [ ] Self-eval bias caveat documented (this section)

## References

- REQ-AXO-91585 — Cross-LLM eval matrix (reframed Claude-self-eval, session 46)
- REQ-AXO-91586 — Compliance benchmark (50 tasks × 3 conditions × 5 runs)
- REQ-AXO-91587 — Drift benchmark (20 tasks × {30, 100, 300} turns)
- CPT-AXO-90017 — PRT schema with output_contract / golden_examples / evaluation_refs
- DEC-AXO-135 — SKI/PRT entity separation rationale
