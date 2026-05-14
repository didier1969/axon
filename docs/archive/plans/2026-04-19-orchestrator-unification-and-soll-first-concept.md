# Orchestrator Unification And SOLL-First Concept

## Intent

Axon should converge toward one coherent runtime doctrine and one coherent documentation doctrine:

- runtime doctrine:
  - one canonical orchestrator computes dynamic operating points from host constraints and live pipeline signals
  - other layers execute or enforce hard safety bounds, but do not silently override strategic decisions
- documentation doctrine:
  - SOLL becomes the primary way Axon documents intention, constraints, decisions, evidence, and progress
  - markdown plans and operator notes remain secondary, derived, or temporary

This concept joins both because they are the same class of problem:

- too many partial truths
- not enough canonical authority

## Why This Theme Exists

Recent runtime analysis showed:

- Axon already understands the correct problem shape:
  - `graph_ready` has high early value for LLM utility
  - vectorization belongs on GPU
  - VRAM and RAM protection matter more than naive throughput
  - token/batch coherence matters
  - host adaptation matters
- but the implementation is still split across multiple authorities:
  - `runtime_profile`
  - `embedder`
  - `vector_control`
  - optimizer/governor layers
- this allows contradictions:
  - a dynamic controller can recommend one direction
  - a lower layer can still clamp it away

Recent documentation evolution showed the same pattern:

- SOLL is already rich enough to express most durable truth about Axon
- derived docs, plans, and notes are useful
- but Axon still documents too many durable truths outside SOLL first

## Problem Statement

Axon is not missing the idea.

Axon is missing:

1. a single runtime authority for dynamic decisions
2. a single intentional authority for durable project knowledge

Without this:

- runtime tuning remains partially fragmented
- performance debugging remains harder than necessary
- documentation truth can drift across surfaces

## Target State

### Runtime

Axon should behave like a bounded self-balancing flow system.

Fixed or effectively fixed:

- model contract
  - embedding dimension
  - supported input shape
  - hard model limits
- hard safety bounds
  - VRAM ceiling
  - RAM ceiling
  - max batch bytes
  - max chunks per file
  - max acceptable interactive latency envelope

Computed dynamically:

- vector worker fan-out
- graph worker fan-out
- batch sizes
- files-per-cycle
- ready/persist/prepare queue targets
- graph-first vs semantic-refill vs balanced scheduling
- quiescent/idle behavior

Canonical rule:

- one runtime controller computes
- all other layers obey or apply declared local hard guards only

### Documentation

SOLL should become the primary project documentation system for Axon.

Durable truths should live in SOLL first:

- `Vision`
- `Pillar`
- `Requirement`
- `Decision`
- `Guideline`
- `Concept`
- `Milestone`
- `Validation`

Markdown remains for:

- temporary plans
- operator notes
- derived human-friendly presentations
- migration handoffs

Transition rule:

- no durable markdown truth may remain implicitly canonical
- every durable markdown artifact kept during migration must be marked as:
  - `canonical`
  - `derived`
  - `transitional`
- `transitional` markdown must have an explicit exit path toward SOLL or archival retirement

Derived docs must stay explicitly non-canonical.

## Scope

In scope:

- identify every runtime parameter as:
  - fixed
  - bounded
  - computed
- define one canonical runtime orchestration authority
- remove strategic contradictions between runtime layers
- define quiescent behavior expectations
- define Axon `SOLL-first` documentation policy
- identify what must move into SOLL or be mirrored there

Out of scope for this concept:

- immediate full migration of every existing markdown note into SOLL
- replacing all operator docs with SOLL-only flows overnight
- inventing a second orchestrator beside the current governor stack

## Non-Goals

- no big bang rewrite
- no “all heuristic, no hard limits” design
- no removal of safety guards that protect VRAM/RAM stability
- no claim that every text artifact should disappear

## Key Architectural Decision

Recommended direction:

- keep the existing runtime architecture family
- unify authority, not ownership of every code path

Concretely:

- `runtime_profile` becomes seed and initial host interpretation
- the runtime orchestrator becomes the canonical strategic decider
  - this does not require a brand-new engine first
  - it may initially be a unifying authority layer above existing `optimizer` + `vector_control`
- `embedder` and other low-level layers keep local hard safety guards only
- `vector_control` becomes execution logic for orchestrator decisions, not a competing authority

Required runtime split:

- strategic decision
- local safety guards
- recovery override

This split must stay explicit so Axon does not replace fragmented authority with one opaque super-controller.

Documentation equivalent:

- SOLL becomes canonical intent truth
- generated docs remain derived
- plans remain transitional unless promoted into SOLL concepts, decisions, requirements, guidelines, or validations

## Evidence So Far

Observed runtime evidence supports this concept:

- graph value is already front-loaded and largely correct
- vector throughput is still structurally under-realized
- under-utilized GPU plus large backlog exposes authority fragmentation
- current documentation already proves SOLL can hold much more of Axon's durable truth

## Risks

Runtime risks:

- centralizing authority too abruptly may destabilize safe behavior
- quiescent mode may accidentally slow hot-path wake-up if designed poorly

Documentation risks:

- forcing all docs into SOLL too quickly may create migration fatigue
- poor discipline may duplicate truth instead of reducing it

## Mitigations

- migrate authority in steps
- keep explicit safety bounds visible and tested
- require parity checks between old and new runtime decision paths
- define a simple SOLL-first policy before mass migration
- treat markdown as transitional unless explicitly canonicalized

## Success Criteria

Runtime:

- one declared canonical source of dynamic decision-making
- no hidden strategic clamp outside declared safety bounds
- measurable quiescent mode with lower idle noise/heat
- throughput behavior explainable from one control plane

Documentation:

- explicit SOLL-first policy for Axon
- durable project truths primarily represented in SOLL
- derived docs and markdown clearly marked by role

## Verdict

This is the correct next architecture theme.

Not because Axon is far from the original vision,
but because Axon is close enough that fragmentation, not ignorance, is now the main limiter.
