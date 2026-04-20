# Greenfield Truth Convergence Implementation Plan

## Phase 1. Canonical completeness model

Define one canonical server-side completeness evaluator and make it the named reference for greenfield project truth:
- concept completeness
- implementation completeness
- heuristic anomaly overlays

This model must become the reference for:
- `soll_validate`
- `soll_verify_requirements`
- `soll_work_plan`
- SOLL-related portions of `anomalies`

Deliverables:
- canonical field names or shared payload shape for the completeness axes
- explicit identification of which surface owns canonical structural truth and which surfaces project or explain divergence from it

## Phase 2. Evidence normalization

Audit and normalize:
- accepted `entity_type` values for `soll_attach_evidence`
- casing/normalization policy
- counting semantics for evidence across read surfaces
- visibility rules for attached traceability
- eliminate duplicated requirement-evidence counting logic in `soll_verify_requirements` and `soll_work_plan`, or route both through the same normalized helper

Deliverable:
- one canonical normalization path for SOLL traceability entities

## Phase 3. Surface convergence

Realign the read surfaces:

### `soll_validate`
- remain the canonical structural invariant checker
- explicitly report:
  - concept completeness
  - implementation completeness readiness

### `soll_verify_requirements`
- align requirement state with the canonical evidence model
- ensure its counts cannot silently contradict `soll_validate`

### `soll_work_plan`
- derive work-plan blockers from the same completeness model

### `anomalies`
- separate:
  - heuristic anomaly findings
  - canonical concept-baseline violations
- downgrade `orphan_intent` to an explicitly heuristic class unless it is proven by the canonical completeness model
- do not report SOLL-facing anomalies in a way that contradicts the canonical SOLL state without explanation
- limit this wave to SOLL/greenfield-facing findings; broader code-smell anomaly heuristics remain out of scope

## Phase 4. Validation matrix

Validation must include:
- one greenfield project with healthy concept baseline
- one project with intentionally missing evidence
- one project with intentionally broken structural links

For each:
- compare `soll_validate`
- compare `soll_verify_requirements`
- compare `soll_work_plan`
- compare `anomalies`

Expected result:
- convergence or explicit justified divergence
- differences must be visible in tool payloads, not only inferable from prose or external docs
- for the same project snapshot, `soll_work_plan.validation_gates`, `soll_verify_requirements`, and SOLL-facing `anomalies` point to the same underlying completeness state or carry an explicit semantic-boundary explanation

## Phase 5. Higher-level workflow follow-up

Once truth convergence is verified, open the next wave for authoring workflows such as:
- create project foundation
- add requirement under pillar
- add decision that solves requirement
- add validation for requirement
- attach evidence pack

These should depend on the converged truth model and not define it ad hoc.

## Risks

1. Hidden backward-compatibility assumptions in anomaly heuristics
- mitigate with targeted tests and explicit downgrade of heuristic confidence where needed

2. Evidence normalization could reveal dirty legacy data
- mitigate with tolerant normalization on read first, then stricter write normalization

3. Over-coupling all surfaces too quickly
- mitigate by sharing semantics, not necessarily code paths, in the first step

## Review Gates

Gate A. Concept review
- SOLL/runtime reviewer
- agent UX / greenfield workflow reviewer

Gate B. Plan review
- same two reviewers confirm order and scope discipline

Gate C. Post-implementation review
- verify no silent contradiction remains

## Exit Criteria

1. The canonical greenfield truth model is documented.
2. Evidence normalization is explicit and enforced or normalized on read.
3. Contradictory read surfaces are eliminated or explained.
4. The next workflow wave can build on this model with confidence.
