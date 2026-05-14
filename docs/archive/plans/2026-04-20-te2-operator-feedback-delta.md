# TE2 Operator Feedback Delta For Axon MCP

Date: 2026-04-20
Status: executed
Scope: Axon MCP operator ergonomics for SOLL-heavy reconstruction sessions
Primary source: [axon-mcp-feedback-2026-04-19.md](/home/dstadel/projects/trader-elixir-v2/docs/axon-mcp-feedback-2026-04-19.md)
Depends on:
- [2026-04-18-axon-mcp-client-feedback-response-plan.md](/home/dstadel/projects/axon/docs/plans/2026-04-18-axon-mcp-client-feedback-response-plan.md)
- [2026-04-18-llm-facing-mcp-ux-hardening-implementation-plan.md](/home/dstadel/projects/axon/docs/plans/2026-04-18-llm-facing-mcp-ux-hardening-implementation-plan.md)

## Execution Outcome

This delta is now implemented across the main operator-facing surfaces.

Completed waves:

- `T1` topology contract
- `T2` mutation result contract
- `T3` requirement completeness explainability
- `T4` active reconstruction digest
- `T5` derived-docs diagnostics
- `T6` retrieval prioritization for rationale queries

Primary implementation surfaces:

- [tools_soll.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp/tools_soll.rs)
- [tools_context.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp/tools_context.rs)
- [tests.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp/tests.rs)

Operational result:

- SOLL topology is now more self-explaining
- grouped mutations are more machine-usable
- requirement verification is requirement-level and corrective
- `soll_query_context` is usable as a reconstruction digest
- derived docs now expose operator diagnostics instead of only presentation
- rationale retrieval prefers linked evidence, then canonical project docs, then broader workspace material

## Purpose

This document does not replace the existing MCP client-feedback response plan.
It extends it with the operator-specific gaps exposed by the full `TE2` SOLL reconstruction session.

The earlier plan already covers:

- public async contract
- project identity discovery
- stable mutation continuation
- public-tool wording and qualification

The `TE2` report adds a second cluster of needs:

- topology guidance
- requirement completion diagnostics
- projection explainability
- retrieval prioritization during SOLL work
- mutation safety guidance that is concrete enough for autonomous correction

## What The TE2 Report Changes

The original client-feedback response plan is still valid, but incomplete for high-trust SOLL work.

The `TE2` session shows that Axon now needs stronger support in five specific areas:

1. relation policy must be explicit, directional, and prescriptive
2. grouped mutation results must return operationally useful ID mappings
3. derived docs must become diagnosable from an operator standpoint
4. requirement verification must explain partial vs done at entity level
5. SOLL/rationale retrieval must prefer canonical project evidence over workspace noise

## Consolidated Problem Clusters

### Cluster A: Topology Guidance

Observed in the report:

- `soll_relation_schema` is too opaque
- milestone topology is underdocumented
- invalid-link errors are rejective but not corrective
- operators must empirically discover legal directions

Required outcome:

- relation schema becomes machine-readable
- invalid relation errors propose the likely valid direction when one exists
- milestone topology is either explicitly standalone or explicitly linked with legal edges

### Cluster B: Mutation Trust

Observed in the report:

- `soll_apply_plan` and grouped mutation flows are not operationally explicit enough
- fallback to atomic `soll_manager create` remains safer than batching
- change-safety surfaces name risk but not remediation in concrete operator terms

Required outcome:

- batch and plan mutations return stable created/updated/linked/skipped/error envelopes
- logical keys are mapped to canonical IDs
- safety tools return mutation-class recommendations, not only risk labels

### Cluster C: Context And Completeness Diagnostics

Observed in the report:

- `soll_query_context` is too compact for active reconstruction
- requirement verification exposes aggregate counts but not per-requirement reasons
- operators cannot tell why a requirement remains partial

Required outcome:

- `soll_query_context` returns a compact operational digest
- `soll_verify_requirements` returns per-requirement missing dimensions and next actions
- progress across mutation waves becomes visible without repeated export diffing

### Cluster D: Projection Explainability

Observed in the report:

- generated docs are attractive but weak as diagnostics
- node pages do not clearly distinguish:
  - canonical vs derived
  - primary vs supporting
  - score-bearing vs non-score-bearing
- subtree inclusion reasons remain too implicit

Required outcome:

- node pages expose operator diagnostics
- each visible relation is tagged with semantic class
- subtree presence explains the exact canonical edge responsible

### Cluster E: Retrieval Discipline During SOLL Work

Observed in the report:

- `why` and retrieval surfaces can drift into irrelevant workspace/tool noise
- obvious project plans and root docs lose priority during rationale queries

Required outcome:

- rationale retrieval becomes `linked_evidence_first`
- then canonical project-root docs
- then broader workspace material

## Priority Order

### P0: High-Leverage Operator Corrections

Implement first:

1. explicit `soll_relation_schema`
2. prescriptive forbidden-relation errors
3. strong result contract for `soll_apply_plan` and grouped mutations
4. per-requirement diagnostics in `soll_verify_requirements`

Reason:

These are the changes that most directly reduce trial-and-error during autonomous graph work.

### P1: Situational Awareness And Safety Guidance

Implement next:

1. actionable `soll_query_context`
2. concrete remediation output in `change_safety`
3. milestone-topology clarification

Reason:

These changes reduce blind enrichment and make recovery paths faster.

### P2: Projection And Retrieval Hardening

Implement after P0/P1:

1. operator-diagnostic block in derived docs
2. canonical-vs-derived relation tagging
3. retrieval prioritization for SOLL rationale questions

Reason:

Important, but slightly less blocking than topology and mutation trust.

## Proposed Execution Waves

### Wave T1: Topology Contract

Status: delivered

Surface:

- `soll_relation_schema`
- invalid relation errors
- milestone semantics

Acceptance:

- relation schema returns allowed targets, relation names, directions, and projection role
- forbidden-relation errors return:
  - attempted link
  - reason
  - `did_you_mean` when possible
- milestone mode is explicit

### Wave T2: Mutation Result Contract

Status: delivered

Surface:

- `soll_apply_plan`
- grouped mutation/batch paths
- `soll_manager` grouped create/link helpers if present

Acceptance:

- mutation response returns stable result records:
  - `created`
  - `updated`
  - `linked`
  - `skipped`
  - `errors`
- logical keys resolve to canonical IDs immediately
- high-trust grouped mutation becomes viable again

### Wave T3: Requirement Completeness Explainability

Status: delivered

Surface:

- `soll_verify_requirements`
- optionally `change_safety`

Acceptance:

- each requirement can explain why it is `done`, `partial`, or `missing`
- missing dimensions are explicit
- next actions are mutation-class specific

### Wave T4: Active Reconstruction Digest

Status: delivered

Surface:

- `soll_query_context`

Acceptance:

- entity counts by type
- topology/orphan summary
- requirement coverage summary
- last meaningful revision metadata when available

### Wave T5: Derived-Docs Diagnostics

Status: delivered

Surface:

- `soll_generate_docs`

Acceptance:

- node page includes operator-diagnostics block
- relation rows distinguish:
  - canonical vs derived
  - primary vs supporting
  - score-bearing vs non-score-bearing
- subtree inclusion reason is explicit

### Wave T6: Retrieval Prioritization For Intent Queries

Status: delivered

Surface:

- `why`
- rationale/retrieval helpers

Acceptance:

- SOLL rationale questions prefer:
  1. linked evidence
  2. canonical project docs
  3. broader workspace docs
- noisy tool/workspace folders are deprioritized

## Recommended Mapping To Existing Plans

Use the existing plan set as follows:

- keep the 2026-04-18 client-feedback response plan as the base for async/public contract work
- attach `Wave T1` and `Wave T2` to the existing MCP UX hardening implementation stream
- treat `Wave T3`, `Wave T4`, and `Wave T5` as SOLL operator-surface hardening
- treat `Wave T6` as retrieval-policy hardening

This avoids creating a second disconnected delivery line.

## Delivery Notes

The TE2 wave did not replace the broader MCP hardening plan.
It closed the operator-facing SOLL gaps that were most costly during autonomous reconstruction sessions.

Remaining broader MCP work, if resumed later, belongs mainly to:

- async/public-contract hardening
- capability enforcement
- public/expert policy separation
- qualification expansion outside the SOLL-heavy operator path

## Recommended Immediate Next Slice

If only one slice is taken next, it should be:

1. `soll_relation_schema` explicit response
2. corrective forbidden-relation errors
3. strong `soll_apply_plan` result envelope

That is the shortest path to materially improving autonomous SOLL operator performance.

## Verdict

The `TE2` report does not contradict the current Axon direction.
It sharpens it.

Axon's next gains are not primarily in raw capability.
They are in:

- self-explaining topology
- trustworthy mutation results
- diagnosable completeness
- retrieval discipline
- operator-facing projection truth
