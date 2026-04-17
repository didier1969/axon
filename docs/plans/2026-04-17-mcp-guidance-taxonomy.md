# MCP Guidance Taxonomy

## Purpose

This document freezes the phase-1 MCP guidance contract and taxonomy.

Scope of phase 1:

- tools covered: `query`, `inspect`
- surface covered: public MCP response only
- rollout shape: shadow mode first, then authoritative only if validation is clean

This document is intentionally narrower than the concept document.

## Phase-1 Response Contract

### Core Fields

These are the only fields that phase 1 should treat as first-class guidance fields:

- `status`
- `problem_class`
- `summary`
- `next_best_actions`
- `confidence`

### Optional Fields

These fields may appear when they add real value:

- `scope`
- `canonical_sources`
- `likely_cause`
- `soll`

### SOLL Block

The `soll` block is optional and only appears when a materially relevant SOLL gap exists.

Allowed subfields:

- `recommended_action`
- `update_kind`
- `reason`
- `requires_authorization`

Rules:

- no SOLL block on clean success
- no implicit SOLL mutation
- any SOLL recommendation must say whether authorization is required

## Phase-1 Problem Classes

Only these classes are in scope for phase 1:

- `none`
- `input_not_found`
- `input_ambiguous`
- `wrong_project_scope`
- `tool_unavailable`
- `index_incomplete`
- `vectorization_incomplete`
- `missing_rationale_in_soll`
- `intent_missing_in_soll`
- `backend_pressure`

### Precedence Rule

When more than one class seems plausible, phase 1 must choose one primary class deterministically.

Recommended precedence:

1. `tool_unavailable`
2. `wrong_project_scope`
3. `input_ambiguous`
4. `input_not_found`
5. `backend_pressure`
6. `index_incomplete`
7. `vectorization_incomplete`
8. `intent_missing_in_soll`
9. `missing_rationale_in_soll`
10. `none`

Meaning:

- scope errors outrank symbol-resolution errors
- ambiguity outranks absence
- backend or index degradation should not hide a cleaner scope or resolution diagnosis
- SOLL gaps are secondary to successful target resolution and should not mask a more immediate tool-input failure

### Intended Meaning

- `none`
  Guidance is absent. The base response stands on its own.

- `input_not_found`
  The requested symbol or target could not be resolved in the current scope, but the server may still have suggestions or a nearby canonical target.

- `input_ambiguous`
  More than one plausible target exists, and the LLM must disambiguate before proceeding.

- `wrong_project_scope`
  The requested project scope is invalid, non-canonical, or mismatched with the evidence actually found.

- `tool_unavailable`
  The tool is not available in the current runtime mode or operational profile.

- `index_incomplete`
  The graph/indexing layer is incomplete enough that the result must be treated as partial.

- `vectorization_incomplete`
  The semantic layer is incomplete enough that the result must be treated as partial.

- `missing_rationale_in_soll`
  Code evidence exists, but maintained rationale is missing or under-specified in SOLL.

- `intent_missing_in_soll`
  Code evidence exists, but a meaningful intentional anchor is absent from SOLL.

- `backend_pressure`
  Runtime pressure or temporary backend conditions reduce confidence or completeness.

### Vision Handling In Phase 1

`Vision` remains strategically important, but it is not a dedicated phase-1 guidance class.

Rules:

- do not introduce a `missing_vision` class in phase 1
- do not recommend `Vision` updates by default in `query` or `inspect`
- if a phase-1 case reveals a project-purpose gap, map it under `intent_missing_in_soll` only when the evidence is strong and the recommendation remains authorization-gated
- otherwise keep `Vision` concerns outside the phase-1 public guidance envelope

## Guidance Emission Rules

### Guidance Must Be Absent By Default

Guidance should be omitted when:

- the tool succeeded cleanly
- the target is exact and anchored
- there is no material follow-up needed
- there is no material SOLL gap

### Guidance Should Appear When It Adds Real Orientation

Guidance should appear when:

- the input is empty, missing, ambiguous, or mis-scoped
- the result is degraded or partial
- the tool is unavailable in the current runtime profile
- code evidence exists but a material SOLL gap remains

### Success Is Not Enough To Justify Guidance

Successful responses should remain compact unless:

- the next best action is materially important
- the result is only partially trustworthy
- there is a real and material SOLL maintenance recommendation

## Action Semantics

Phase 1 guidance should emit compact action phrases, not long explanations.

Good action examples:

- `retry with suggested symbol`
- `use returned canonical project code`
- `run project_status`
- `use query to broaden recall`
- `review current SOLL context`
- `update Decision or Requirement if authorized`

Bad action examples:

- long multi-step narratives
- speculative repair advice without evidence
- automatic intent mutation language

## Presence Examples

### Guidance Absent

Case:

- `inspect` resolves an exact symbol in canonical scope with anchored evidence

Expected:

- `status = ok`
- no `problem_class`
- no `next_best_actions`
- no `soll`

### Guidance Present

Case:

- `query` misses the exact symbol, but finds a strong canonical suggestion

Expected:

- `status = warn_input_not_found` or equivalent warning status
- `problem_class = input_not_found`
- `next_best_actions = [retry with suggested symbol, use query to broaden recall]`

### Guidance Present With SOLL

Case:

- `inspect` finds the code target, but no maintained rationale is attached in SOLL

Expected:

- `status = ok`
- `problem_class = missing_rationale_in_soll`
- `soll.recommended_action = recommend_update`
- `soll.update_kind = decision_or_requirement`
- `soll.requires_authorization = true`

## Non-Goals

Phase 1 does not try to do any of the following:

- cover every MCP tool
- turn MCP into a second protocol
- auto-mutate SOLL by default
- move wording or rendering into the classifier
- replace DuckDB or current runtime truth
- prove Datalog is mandatory before a narrow classifier works

## Acceptance Criteria

Phase 1 is acceptable only if:

- the contract stays compact
- guidance is absent on clean success
- guidance is semantically stable on degraded or ambiguous cases
- SOLL recommendations are explicit, bounded, and authorization-aware
- the taxonomy is good enough to support goldens and shadow-mode replay
