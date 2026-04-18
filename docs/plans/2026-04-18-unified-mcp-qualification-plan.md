# Unified MCP Qualification Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Introduce a single operator-facing MCP qualification command that unifies quality, latency, robustness, guidance, and SOLL surface qualification without deleting the specialized scripts that already work.

**Architecture:** Add one new Python orchestrator behind `./scripts/axon qualify-mcp`, keep existing Python probes and validators as internal engines, and progressively demote legacy operator entrypoints to expert/compatibility status. The new orchestrator produces one aggregated `summary.json`, one operator-readable verdict, and explicit sub-verdicts by check family and surface.

**Tech Stack:** Bash wrapper `scripts/axon`, Python orchestration scripts under `scripts/`, existing MCP validators/measurements/guidance harnesses, JSON summary artifacts.

---

### Task 1: Freeze the operator contract

**Files:**
- Create: `docs/plans/2026-04-18-unified-mcp-qualification-plan.md`
- Modify: `scripts/axon`
- Test: manual `./scripts/axon --help`

**Step 1: Define the canonical operator entrypoint**

Canonical command:
```bash
./scripts/axon qualify-mcp [options]
```

**Step 2: Freeze the supported options**

Required options:
```text
--surface core|soll|all
--checks quality,latency,robustness,guidance
--mode cold|steady-state|both
--mutations off|dry-run|safe-live|full
--project <CODE>
--baseline <summary.json>|auto
--strict
--json-out <path>
--label <run-label>
```

Optional options:
```text
--scenario-file <json>
--skip-regression
--timeout <sec>
--top-slowest <N>
--name-pattern <substring>
--keep-running
--artifacts-root <dir>
```

**Step 3: Freeze the mutation semantics**

Rules:
- `off`: read-only, no write-capable probes
- `dry-run`: non-committing previews/simulations only
- `safe-live`: bounded and reversible live probes only
- `full`: explicit real mutations, never implied

**Step 4: Freeze the surface semantics**

Rules:
- `core`: public MCP read/analysis surface
- `soll`: SOLL read + optional mutation qualification according to `--mutations`
- `all`: union with separate sub-verdicts

**Step 5: Commit**

```bash
git add docs/plans/2026-04-18-unified-mcp-qualification-plan.md
git commit -m "docs: define unified MCP qualification contract"
```

### Task 2: Reclassify existing scripts by role

**Files:**
- Modify: `scripts/axon`
- Test: `./scripts/axon --help`

**Step 1: Mark the new primary interface**

New primary operator flow:
```text
qualify-mcp
```

**Step 2: Reclassify legacy entrypoints as expert/internal**

Keep exposed but document as secondary:
- `validate-mcp`
- `robustness-mcp`
- `measure-mcp`
- `compare-mcp`
- `quality-mcp`
- `qualify-guidance`

Keep distinct as SOLL tools:
- `soll-import`
- `work-plan`

**Step 3: Do not reclassify internal probes as operator commands**

Keep internal only:
- `scripts/measure_mcp_core_latency.py`
- `scripts/measure_project_status_stack.py`
- `scripts/measure_symbol_flow_tools.py`
- `scripts/mcp_probe_common.py`

**Step 4: Update wrapper help text to reflect taxonomy**

Desired taxonomy in help:
- `qualify`
- `measure`
- `soll`
- `identity`

**Step 5: Commit**

```bash
git add scripts/axon
git commit -m "refactor: reclassify MCP qualification entrypoints"
```

### Task 3: Add the new Python orchestrator skeleton

**Files:**
- Create: `scripts/qualify_mcp.py`
- Modify: `scripts/axon`
- Test: `python3 scripts/qualify_mcp.py --help`

**Step 1: Write the failing invocation test contract**

Document expected CLI behavior:
```bash
python3 scripts/qualify_mcp.py --surface core --checks quality,latency --mode steady-state --baseline auto --project AXO
```

Expected:
- parses flags
- prints selected plan
- exits non-zero on invalid combinations

**Step 2: Implement minimal CLI parsing**

Required validations:
- reject unknown `--surface`
- reject unknown `--mode`
- reject unknown `--mutations`
- reject `--mutations != off` for surfaces/check combinations that do not support writes
- reject `--baseline` when latency/regression is not selected unless explicitly allowed

**Step 3: Add `scripts/axon qualify-mcp` wrapper case**

Minimal routing:
```bash
qualify-mcp)
  exec python3 scripts/qualify_mcp.py "$@"
  ;;
```

**Step 4: Verify help output**

Run:
```bash
python3 scripts/qualify_mcp.py --help
./scripts/axon qualify-mcp --help
```

Expected:
- one coherent interface
- mutation semantics documented

**Step 5: Commit**

```bash
git add scripts/qualify_mcp.py scripts/axon
git commit -m "feat: add unified MCP qualification CLI skeleton"
```

### Task 4: Implement check-to-engine mapping

**Files:**
- Modify: `scripts/qualify_mcp.py`
- Test: `python3 scripts/qualify_mcp.py --surface core --checks quality --project AXO --json-out /tmp/qualify-core-quality.json`

**Step 1: Map `quality` to existing validator**

Engine:
```text
scripts/mcp_validate.py
```

Rules:
- use dedicated scenario files per surface when available
- pass `--strict` through
- pass mutation mode through safely

**Step 2: Map `latency` to existing measurement suite**

Engine:
```text
scripts/measure_mcp_suite.py
```

Rules:
- pass `--mode`
- pass `--project`
- set label automatically if absent

**Step 3: Map `regression` implicitly from latency**

Engine:
```text
scripts/compare_mcp_runs.py
```

Rules:
- only run if latency artifacts exist
- support `--baseline auto`

**Step 4: Map `robustness` and `guidance`**

Engines:
```text
scripts/qualify_mcp_robustness.py
scripts/qualify_mcp_guidance.py
```

Rules:
- `guidance` supports fixture/live source selection
- `robustness` remains opt-in due runtime cost

**Step 5: Commit**

```bash
git add scripts/qualify_mcp.py
git commit -m "feat: map unified MCP checks to existing engines"
```

### Task 5: Add surface-aware scenarios and safety guards

**Files:**
- Create: `scripts/mcp_scenarios/core_qualification.json`
- Create: `scripts/mcp_scenarios/soll_readonly_qualification.json`
- Create: `scripts/mcp_scenarios/soll_dry_run_qualification.json`
- Modify: `scripts/qualify_mcp.py`
- Test: `python3 scripts/qualify_mcp.py --surface soll --checks quality --mutations dry-run --project AXO`

**Step 1: Create explicit `core` scenario coverage**

Cover:
- `status`
- `project_status`
- `query`
- `inspect`
- `retrieve_context`
- `why`
- `path`
- `impact`
- `change_safety`

**Step 2: Create explicit SOLL read-only scenario coverage**

Cover:
- `soll_query_context`
- `soll_work_plan`
- `soll_validate`
- `soll_verify_requirements`

**Step 3: Create explicit SOLL dry-run scenario coverage**

Cover only if safe:
- `soll_apply_plan` dry-run
- controlled preview path if already supported

**Step 4: Encode safety guards in the orchestrator**

Rules:
- `--surface soll --mutations off` must never call mutators
- `--surface soll --mutations dry-run` must only call safe preview probes
- `safe-live` and `full` require explicit operator opt-in

**Step 5: Commit**

```bash
git add scripts/mcp_scenarios/core_qualification.json scripts/mcp_scenarios/soll_readonly_qualification.json scripts/mcp_scenarios/soll_dry_run_qualification.json scripts/qualify_mcp.py
git commit -m "feat: add surface-aware qualification scenarios"
```

### Task 6: Produce one aggregated summary format

**Files:**
- Modify: `scripts/qualify_mcp.py`
- Test: inspect `/tmp/qualify-summary.json`

**Step 1: Define the summary schema**

Required fields:
```json
{
  "verdict": "ok|warn|fail",
  "surface": "core|soll|all",
  "checks": ["quality", "latency"],
  "mode": "cold|steady_state|both",
  "mutations": "off|dry-run|safe-live|full",
  "subverdicts": {
    "quality": "ok|warn|fail",
    "latency": "ok|warn|fail",
    "robustness": "ok|warn|fail",
    "guidance": "ok|warn|fail"
  },
  "artifacts": {},
  "load_state": {},
  "highlights": {
    "slowest_tools": [],
    "fragile_checks": [],
    "operator_summary": ""
  }
}
```

**Step 2: Normalize outputs from existing engines**

Rules:
- do not leak raw heterogenous script output as the main contract
- preserve raw artifacts under `artifacts`
- expose only normalized verdicts at top level

**Step 3: Add operator-readable stdout summary**

Required stdout sections:
- global verdict
- per-check verdict
- mutation mode
- slowest tools
- artifact locations

**Step 4: Verify both machine and operator outputs**

Run:
```bash
python3 scripts/qualify_mcp.py --surface core --checks quality,latency --project AXO --json-out /tmp/qualify-summary.json
```

Expected:
- valid JSON
- concise human summary

**Step 5: Commit**

```bash
git add scripts/qualify_mcp.py
git commit -m "feat: aggregate unified MCP qualification summaries"
```

### Task 7: Turn legacy quality entrypoint into a thin compatibility wrapper

**Files:**
- Modify: `scripts/axon`
- Modify: `scripts/mcp_quality_gate.sh`
- Test: `./scripts/axon quality-mcp`

**Step 1: Preserve compatibility**

Keep:
```bash
./scripts/axon quality-mcp
```

But make it call:
```bash
./scripts/axon qualify-mcp --surface core --checks quality,latency --mode steady-state --baseline auto --strict
```

**Step 2: Reduce duplicated orchestration**

Rules:
- `mcp_quality_gate.sh` becomes a thin wrapper or compatibility shim
- no new business logic should be added there

**Step 3: Keep expert entrypoints callable**

Do not remove direct access to:
- `validate-mcp`
- `measure-mcp`
- `compare-mcp`
- `robustness-mcp`
- `qualify-guidance`

**Step 4: Verify compatibility**

Run:
```bash
./scripts/axon quality-mcp
./scripts/axon validate-mcp --project BookingSystem --strict
./scripts/axon measure-mcp --project AXO --label smoke
```

Expected:
- old workflows still run
- new workflow is the documented default

**Step 5: Commit**

```bash
git add scripts/axon scripts/mcp_quality_gate.sh
git commit -m "refactor: route legacy quality gate through unified qualifier"
```

### Task 8: Document the new operator flow

**Files:**
- Modify: `docs/skills/axon-engineering-protocol/SKILL.md`
- Modify: `scripts/axon`
- Create: `docs/plans/2026-04-18-mcp-qualification-operator-notes.md`
- Test: `./scripts/axon --help`

**Step 1: Update operator documentation**

Document:
- `qualify-mcp` as primary qualification interface
- legacy entrypoints as expert/compatibility flows
- mutation levels and safety semantics

**Step 2: Keep skill scope correct**

Only document visible MCP qualification workflow, not internal script plumbing beyond what an operator needs.

**Step 3: Add short operator notes**

Include examples:
```bash
./scripts/axon qualify-mcp --surface core --checks quality,latency --mode steady-state --project AXO
./scripts/axon qualify-mcp --surface soll --checks quality --mutations off --project AXO
./scripts/axon qualify-mcp --surface soll --checks quality,guidance --mutations dry-run --project AXO
```

**Step 4: Verify help text and docs coherence**

Expected:
- one primary entrypoint
- no ambiguity about when to use legacy commands

**Step 5: Commit**

```bash
git add docs/skills/axon-engineering-protocol/SKILL.md docs/plans/2026-04-18-mcp-qualification-operator-notes.md scripts/axon
git commit -m "docs: document unified MCP qualification workflow"
```

### Task 9: Certify with targeted end-to-end checks

**Files:**
- Modify: `scripts/qualify_mcp.py` if needed
- Test: end-to-end command matrix

**Step 1: Verify core steady-state path**

Run:
```bash
./scripts/axon qualify-mcp --surface core --checks quality,latency --mode steady-state --project AXO --baseline auto
```

**Step 2: Verify core deep path**

Run:
```bash
./scripts/axon qualify-mcp --surface core --checks quality,latency,robustness,guidance --mode both --project AXO --baseline auto
```

**Step 3: Verify SOLL read-only path**

Run:
```bash
./scripts/axon qualify-mcp --surface soll --checks quality --mutations off --project AXO
```

**Step 4: Verify SOLL dry-run path**

Run:
```bash
./scripts/axon qualify-mcp --surface soll --checks quality,guidance --mutations dry-run --project AXO
```

**Step 5: Commit**

```bash
git add scripts/qualify_mcp.py scripts/axon
git commit -m "test: certify unified MCP qualification flows"
```

### Task 10: Deprecation follow-up, not immediate removal

**Files:**
- Modify: `scripts/axon`
- Modify: `scripts/mcp_quality_gate.sh`
- Test: `./scripts/axon --help`

**Step 1: Add deprecation hints**

For:
- `quality-mcp`
- `measure-mcp`
- `compare-mcp`
- `validate-mcp`
- `robustness-mcp`
- `qualify-guidance`

Message:
- still supported
- prefer `qualify-mcp` for standard operator use

**Step 2: Do not remove expert flows yet**

Removal gate:
- only after repeated successful use of `qualify-mcp`
- only after docs and CI rely on the new flow

**Step 3: Verify user-facing messaging**

Expected:
- no abrupt break
- no ambiguity about the canonical entrypoint

**Step 4: Final review**

Check:
- no duplicated orchestration logic remains in two primary places
- no specialized probes were promoted accidentally to operator-first commands

**Step 5: Commit**

```bash
git add scripts/axon scripts/mcp_quality_gate.sh
git commit -m "chore: add MCP qualification deprecation guidance"
```
