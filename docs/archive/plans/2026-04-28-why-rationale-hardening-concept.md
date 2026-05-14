# Why Rationale Hardening Concept

## Summary

The current `why` surface is already useful, but it still mixes three different classes of truth:

- canonical governing intent
- generic supporting context
- weak repository correlation

This creates a product risk: low-grade retrieval artifacts can be rendered with nearly the same rhetorical weight as canonical rationale. The effect is visible in current `live` MCP output:

- strong cases such as `axon_soll_work_plan` can join meaningful governing decisions
- weaker cases such as `axon_why` and `runtime_topology_snapshot` still render with generic doc anchors and medium confidence
- some supporting chunks come from non-governing files such as `benchmark.py`, which is noise for rationale

The goal is to harden `why` so that it explains **why a symbol exists** using explicit evidence classes and an honest quality model, instead of presenting all retrieval neighbors as if they were equally meaningful.

This will be delivered as **one single improvement phase**, not as an isolated `why` patch. That phase includes:

- `why` hardening as the primary surface change
- Axon skill realignment so agents read the new contract correctly
- targeted MCP contract-test strengthening for neighboring rationale surfaces
- one bounded structural reduction pass on `tools_framework.rs`
- one additional bounded structural reduction pass on `tools_soll.rs`
- explicit capture of follow-up observations for `retrieve_context`, `inspect`, and `conception_view`

## Problem Statement

The current `why` response has four coupled weaknesses:

1. governing intent is flattened into `Relevant SOLL entities`
2. weak or generic supporting chunks are not clearly demoted
3. answer text is phrased as retrieval narration rather than causal rationale
4. the response does not distinguish between:
   - direct intent linkage
   - inferred support
   - weak repo correlation

This makes `why` less trustworthy for refactor steering, delivery traceability, and architectural review.

## Evidence

Observed from `live` MCP:

- `why(symbol="axon_soll_work_plan")`
  - credible decisions joined: `DEC-AXO-008`, `DEC-AXO-014`
  - but still includes a `benchmark.py` supporting chunk
- `why(symbol="runtime_topology_snapshot")`
  - confidence only `medium`
  - `anchored_chunks_selected = 0`
  - still renders as a normal rationale packet
- `why(symbol="axon_why")`
  - ties mostly to `REQ-AXO-015`, which appears too narrow for the tool itself
- `why(symbol="axon_soll_attach_evidence")`
  - also includes `benchmark.py` as support, which is non-governing noise

Code evidence:

- [tools_framework.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp/tools_framework.rs:93)
  - `summarize_why_response(...)` aggregates packet content into a flattened `why` summary
- [tools_context.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp/tools_context.rs:2625)
  - `render_evidence_packet(...)` renders direct evidence, supporting chunks, neighbors, and SOLL entities with limited evidence-class separation
- [tools_context.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp/tools_context.rs:2580)
  - confidence is currently mostly structural/route-based, not rationale-quality-based

## Desired Outcome

`why` should become a product-grade rationale surface with:

- explicit evidence classes
- explicit evidence-state semantics
- honest explanation quality
- better causal wording
- aggressive demotion of retrieval noise

The ideal operator reading should answer:

- what this symbol exists to satisfy
- which requirement or decision governs it
- which evidence is direct vs inferred
- how trustworthy the current rationale packet is

## Scope

In scope:

- `why` response structure
- `why` textual rendering
- evidence-class separation in packet summarization
- rationale-quality scoring
- filtering or demoting non-governing support artifacts
- targeted skill realignment for the Axon operator protocol so agents interpret the new `why` contract correctly
- MCP contract-test strengthening around adjacent rationale surfaces
- bounded structural cleanup in `tools_framework.rs`
- bounded structural cleanup in `tools_soll.rs`
- explicit audit notes for adjacent retrieval-derived surfaces

Out of scope:

- broad rewrite of retrieval routing
- full traceability ontology redesign
- large `tools_framework.rs` decomposition as part of the same change
- broad multi-surface retrieval redesign
- TensorRT build/qualification work

## Product Contract

The new `why` contract must separate two axes that are currently blended:

1. `authority_class`
   - `governing`
   - `supporting`
   - `correlated`
   - `unknown`

2. `evidence_provenance`
   - `soll_requirement`
   - `soll_decision`
   - `soll_guideline`
   - `doc`
   - `code_symbol`
   - `code_chunk`
   - `test`
   - `benchmark`
   - `script`

Every returned rationale item must declare both axes.

In addition, rationale links must declare their link mode:

- `direct`
- `inferred`
- `weak_correlation`
- `unknown`

This is the minimum honesty contract. No quality label or prose layer should be computed before these states exist.

## Proposed Changes

### 1. Separate evidence classes explicitly

Add explicit groups to `why` output:

- `governing_requirements`
- `governing_decisions`
- `supporting_guidelines`
- `supporting_docs`
- `direct_code_evidence`
- `supporting_code_context`

`Relevant SOLL entities` should stop being the only intent bucket. It should either disappear or be retained only as a compatibility view derived from the new groups.

Each element in these groups must also carry:

- `authority_class`
- `evidence_provenance`
- `link_mode`
- `inclusion_reason`

### 2. Introduce explicit evidence-state semantics first

Before any top-level quality label, the response must expose machine-visible negative and degraded states such as:

- `missing_governing_intent`
- `no_direct_traceability`
- `retrieval_degraded`
- `support_only`
- `correlation_only`

These states must be first-class output, not inferred only from prose.

### 3. Derive rationale quality from evidence states

Add a structured quality model, for example:

- `rationale_quality = strong | mixed | weak`
- `confidence_reason`
- `intent_link_mode = direct | inferred | weak_correlation`

This quality signal should depend on:

- presence of governing intent links
- presence of anchored direct evidence
- proportion of weak support artifacts
- retrieval diagnostics such as anchored vs unanchored chunk selection

Constraint:

- `rationale_quality` is informational for operators
- it is not a stable automation contract in v1
- the canonical machine contract is the evidence-state layer above

### 4. Filter or demote noisy support artifacts safely

Default behavior should demote or exclude rationale support from:

- benchmarks
- fixtures
- incidental tests
- auxiliary scripts

These should not appear as ordinary support unless they are the only available evidence and are explicitly marked as weak correlation.

Safety rule:

- filtered artifacts must remain inspectable
- exclusion or demotion must be visible in `excluded_because`
- demotion must be role-driven, not just path-driven
- tests and benchmarks may still appear when they are the only behavioral evidence, but never as unqualified governing evidence

### 5. Rewrite the answer sketch around causality

Replace retrieval-centric phrasing with a causal contract, for example:

- `exists_to`
- `governed_by`
- `implemented_at`
- `confidence_reason`

The answer should explain intent first, retrieval mechanics second.

Constraint:

- each causal field must carry or derive from `link_mode`
- when evidence is missing, the prose must say so directly instead of laundering inference into fact

### 6. Preserve compatibility with an explicit transition policy

The initial implementation should preserve the existing `why` surface shape enough to avoid breaking clients immediately.

Approach:

- add the new fields first
- keep legacy summary fields temporarily
- improve text rendering to prefer the new groups

Transition policy:

- v1 of this change adds new structured fields without removing legacy fields
- legacy fields remain derived views, not competing truth sources
- no semantic divergence is allowed between legacy and new fields during the transition
- legacy field removal is out of scope for this wave and must happen in a separate compatibility cleanup

## Non-Goals

- do not solve all weak traceability in one pass
- do not reindex the whole repository as part of the first delivery wave
- do not attempt a major architectural split of `tools_framework.rs` in the same change
- do not silently upgrade weak rationale to strong rationale by wording only
- do not patch broad traceability mappings in `live` as part of this first bounded wave

## Constraints

- MCP output remains authoritative
- response changes must stay intelligible in text mode
- compatibility with current MCP clients should degrade minimally
- implementation should stay bounded to the `why` flow, not become a broad retrieval rewrite
- the Axon operator skill must be updated in the same delivery wave if the `why` contract changes materially

## Reuse vs Change

Reuse:

- existing packet construction
- existing confidence plumbing
- existing direct/supporting/neighbor buckets
- existing SOLL joins

Change:

- response classification
- response rendering
- quality semantics
- evidence filtering/demotion rules

## Risks

1. Over-filtering
   - we could hide useful support in edge cases

2. False precision
   - a new quality label could imply more certainty than the data deserves

3. Client drift
   - some clients may depend on the old text shape

4. Scope creep
   - this can easily expand into a broader retrieval-system redesign if not bounded

5. Refactor coupling
   - `tools_framework.rs` and neighboring MCP surfaces are coupled enough that a cleanup pass can accidentally widen the blast radius

## Risk Controls

- add new fields before removing old ones
- mark weak evidence explicitly instead of deleting everything at once
- validate against representative symbols:
  - `axon_soll_work_plan`
  - `runtime_topology_snapshot`
  - `axon_why`
  - `axon_soll_attach_evidence`
- keep the first wave focused on `why` only
- keep adjacent cleanup work explicitly bounded to support this same phase, not to redesign retrieval generally

## Single Delivery Phase

This work should be delivered as one bounded phase with six ordered workstreams:

1. `why` product-surface hardening
2. compatibility-preserving summary rebuild
3. Axon skill realignment
4. MCP contract-test strengthening for adjacent rationale surfaces
5. one bounded structural cleanup pass in `tools_framework.rs`
6. one bounded structural cleanup pass in `tools_soll.rs`

The phase is complete only when all six workstreams have been validated together.

### Workstream A: `why` product-surface hardening

- add structured evidence classes
- add evidence-state semantics
- add rationale-quality semantics
- improve `Answer sketch`
- keep compatibility fields

### Workstream B: Adjacent rationale-surface strengthening

- add contract tests around neighboring surfaces that can suffer similar false-certainty issues
- at minimum cover:
  - `retrieve_context`
  - `inspect`
  - `conception_view`
- the goal is not to redesign them in this phase, but to make their current limitations visible and testable

### Workstream C: Bounded `tools_framework.rs` cleanup

- perform one small structural extraction or internal re-bucketing directly motivated by the `why` implementation
- keep the cleanup within the `why`/rationale cluster
- do not turn this into a broad runtime/status split

### Workstream D: Bounded `tools_soll.rs` cleanup

- perform one additional extraction if a directly adjacent rationale/traceability subdomain still inflates the parent file
- keep this tied to the same phase’s traceability improvements

### Workstream E: Skill realignment

- update the Axon operator skill to match the new `why` contract
- strengthen its guidance on degraded evidence and escalation paths

### Workstream F: Explicit follow-up capture

- record what this phase reveals about:
  - `retrieve_context`
  - `inspect`
  - `conception_view`
  - remaining `tools_framework.rs` hotspots
- capture them as explicit residuals, not implicit tribal knowledge

## Skill Realignment

The Axon operator skill must be updated as part of v1 because it already recommends `why` as a first-choice surface.

Required skill updates:

- document the new `authority_class` semantics:
  - `governing`
  - `supporting`
  - `correlated`
  - `unknown`
- document the new `link_mode` semantics:
  - `direct`
  - `inferred`
  - `weak_correlation`
  - `unknown`
- tell agents to trust machine evidence-state fields before prose
- state explicitly that `rationale_quality` is informational, not an automation-grade truth contract
- teach the correct reading of degraded or missing states such as:
  - `missing_governing_intent`
  - `no_direct_traceability`
  - `retrieval_degraded`
  - `support_only`

## Initial Acceptance Criteria

1. every returned rationale item exposes:
   - `authority_class`
   - `evidence_provenance`
   - `link_mode`
2. `why` distinguishes governing requirements and decisions from generic support
3. `why` exposes machine-visible degraded or missing-evidence states
4. `rationale_quality` is derived from evidence states and documented as informational only
5. legacy fields remain consistent derived views during the transition
6. `runtime_topology_snapshot` no longer presents a medium-confidence packet without explaining why confidence is limited
7. non-governing artifacts such as `benchmark.py` are no longer rendered as ordinary support without explicit surface qualification in v1, even before full filtering policy lands
8. the Axon operator skill documents the new `why` contract and degraded-state reading rules
9. adjacent MCP rationale surfaces have at least minimal contract coverage against false certainty
10. `tools_framework.rs` and `tools_soll.rs` each exit the phase with one additional bounded structural improvement tied to this work
