# Orchestrator Unification And SOLL-First Implementation Plan

## Objective

Execute a bounded migration that:

1. makes runtime orchestration authority explicit and canonical
2. makes SOLL the primary durable documentation surface for Axon

## Delivery Mode

This is a `full` idea-to-delivery theme:

- architecture-sensitive
- runtime-sensitive
- documentation-governance-sensitive

Execution should proceed in phases with explicit review gates.

## Phase 1: Runtime Parameter Inventory

### Goal

Produce a complete classification of runtime parameters:

- fixed
- bounded
- computed

### Tasks

1. Inventory runtime-relevant parameters across:
   - `runtime_profile`
   - `embedder`
   - `vector_control`
   - optimizer/governor layers
   - startup env wiring
2. Tag each parameter:
   - model contract
   - hard safety bound
   - seed/default
   - computed runtime target
3. Identify contradictions:
   - strategic override in lower layers
   - hidden caps
   - duplicate scheduling authority
4. For each dynamic parameter, record:
   - `computed_by`
   - `applied_by`
   - `may_clamp_if`

### Outputs

- parameter inventory document
- contradiction list
- proposed ownership map
- executable authority map for dynamic parameters

## Phase 2: Canonical Runtime Control Contract

### Goal

Define one runtime control plane that computes the live operating point.

### Tasks

1. Define canonical controller inputs:
   - host resources
   - backlog depth
   - queue health
   - MCP latency pressure
   - VRAM pressure
   - RAM pressure
   - idle/quiescent indicators
2. Define canonical controller outputs:
   - vector workers
   - graph workers
   - batch sizes
   - files-per-cycle
   - ready/persist targets
   - idle backoff profile
3. Define local hard-guard contract:
   - which lower-layer checks remain allowed
   - which strategic decisions are forbidden outside the controller
4. Define visibility contract:
   - if effective value differs from orchestrator target, Axon must expose:
     - target value
     - applied value
     - clamp reason
5. Define scheduler relationship explicitly:
   - the utility-first scheduler becomes a sub-policy of the canonical controller
   - it must not remain a parallel strategic authority

### Outputs

- canonical control contract
- safety-bound contract
- migration map from current layers
- non-override observability contract

## Phase 3: Quiescent Mode Design

### Goal

Reduce unnecessary heat, noise, and idle resource churn without harming responsiveness.

### Tasks

1. Audit always-on loops:
   - watcher
   - ingress promoter
   - vector maintenance
   - finalize/persist loops
   - optimizer loop
   - watchdog/reclaimer loops
2. Classify:
   - must stay hot
   - may back off
   - may sleep deeply
3. Define quiescent policy:
   - idle detection conditions
   - progressive backoff
   - wake-up triggers
   - acceptable resume latency

### Outputs

- quiescent-mode design
- idle loop backoff policy
- measurement expectations for reduced idle heat/noise

## Phase 4: Runtime Migration

### Goal

Implement the controller unification incrementally.

### Tasks

1. Move current strategic caps out of low-level layers where inappropriate
2. Rewire runtime sizing so orchestrator outputs are authoritative
3. Keep hard safety bounds explicit and tested
4. Add observability proving:
   - controller target
   - effective applied target
   - reason for any remaining clamp
5. Add quiescent behavior instrumentation

### Validation

- parity tests for existing safe cases
- no regression in MCP latency gates
- better explainability of throughput decisions
- measurable reduction in idle churn

## Phase 5: SOLL-First Documentation Policy

### Goal

Make Axon’s durable project documentation policy explicit.

### Tasks

1. Define what must live in SOLL:
   - durable intent
   - durable constraints
   - durable architecture choices
   - durable evidence
2. Define what may remain outside SOLL:
   - transient implementation plans
   - operator notes
   - generated derived docs
   - temporary investigations
3. Define promotion rules:
   - when a plan/note must be mirrored or elevated into SOLL
4. Define anti-double-truth rule:
   - every durable markdown artifact must be explicitly tagged:
     - `canonical`
     - `derived`
     - `transitional`
   - `transitional` markdown must declare its exit condition

### Outputs

- SOLL-first documentation policy
- mapping table from markdown artifact type to SOLL or non-SOLL role
- transition tagging rule for markdown durability

## Phase 6: SOLL Migration For Axon Core Truth

### Goal

Move the most important Axon durable truths into SOLL-first form.

### Initial targets

- runtime orchestration doctrine
- graph-first / vector-second product rationale
- safety constraints around VRAM/RAM
- quiescent-mode intent
- release/promotion doctrine when durable

### Validation

- Axon project can be understood from SOLL first
- derived docs remain navigable
- no duplicate conflicting truth between SOLL and markdown

## Review Gates

### Gate A: Concept Convergence

Requires two independent reviewers to agree that:

- runtime unification is the right target
- SOLL-first policy is the right documentation doctrine

### Gate B: Control Contract Convergence

Requires agreement that:

- the ownership map is coherent
- hard bounds remain explicit
- no second hidden strategic authority remains

### Gate C: Quiescent Design Convergence

Requires agreement that:

- idle cooling is real
- wake-up latency remains acceptable

### Gate D: SOLL Governance Convergence

Requires agreement that:

- SOLL-first is enforceable
- markdown still has a narrow, clear role

## Risks

- too much migration at once
- under-specified controller authority boundaries
- quiescent mode hurting responsiveness
- documentation duplication during transition

## Risk Controls

- phase runtime and documentation separately but under one doctrine
- require explicit parity checks
- require observability before removing old clamps
- keep migration of existing docs selective and value-driven

## Recommended Execution Order

1. parameter inventory
2. canonical control contract
3. quiescent design
4. runtime migration batch 1
5. SOLL-first policy
6. selected SOLL migration for Axon core truths
7. runtime migration batch 2 and cleanup

## Success Criteria

Runtime:

- one declared strategic authority
- no hidden strategic override
- measurable reduction in idle heat/noise
- better vector throughput scaling under safe bounds

Documentation:

- SOLL-first policy explicit and applied
- Axon durable truths primarily stored in SOLL
- markdown role narrowed and clear

## Immediate Next Step

Start with a bounded sub-wave:

- parameter inventory
- authority map
- SOLL-first policy draft

That is the lowest-risk entry point and gives the rest of the migration a stable frame.
