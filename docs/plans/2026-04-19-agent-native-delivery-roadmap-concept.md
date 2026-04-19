# Agent-Native Delivery Roadmap Concept

## Scope

This roadmap implements the first delivery-grade wave from the April 19 feedback:

- implicit capture then confirmation
- higher-level mutation primitives
- richer post-mutation feedback

It does **not** implement the full roadmap in one batch.

## First-Wave Decision

The first wave should stay inside the existing SOLL/MCP surface instead of introducing a second orchestration system.

The smallest high-value additive surface is:

- `infer_soll_mutation`
- `entrench_nuance`
- richer mutation result contracts on existing SOLL mutations

## Why This Wave First

These three items reinforce each other:

- `infer_soll_mutation` reduces modeling friction
- `entrench_nuance` gives a delivery-language mutation entrypoint
- richer mutation feedback closes the operator loop after each change

Together they cover the highest-leverage parts of:

- P1 implicit capture then confirmation
- P4 mutation primitives at a higher level
- P5 better post-mutation feedback

## Reuse Over Reinvention

The implementation must reuse existing server truth:

- canonical project identity
- `soll_manager`
- `soll_completeness_snapshot`
- derived docs refresh hooks

The first wave must not create:

- a parallel mutation registry
- a parallel graph-quality engine
- a second notion of project intent truth

## Product Contract

### `infer_soll_mutation`

Read-only assistive analysis that returns:

- candidate entity type
- impacted canonical IDs
- proposed operation
- confidence
- ambiguity warnings
- next best actions

Contract constraints:

- advisory only
- no reserved IDs
- no side effects
- project scope must be resolved canonically before any proposal is returned

### `entrench_nuance`

High-level workflow for stabilizing a nuance.

Behavior:

- default path: propose first, require confirmation
- confirmed path: apply bounded updates to existing canonical nodes
- use server-owned IDs only

Contract constraints:

- synchronous in wave 1
- proposal response is explicit and machine-readable
- confirmation is an explicit MCP argument, not an implicit retry
- bounded to updates on existing canonical SOLL entities in wave 1
- must not silently create new entities in wave 1
- must not create a new ontology, hidden lifecycle, or metadata-only shadow truth
- any nuance persisted in metadata must use an explicitly canonical metadata shape owned by the server

### Rich Mutation Feedback

After successful SOLL mutations, return:

- `changed_entities`
- `topology_delta`
- `newly_unblocked`
- `remaining_blockers`
- `next_best_actions`
- before/after completeness summary derived from canonical SOLL validation

Contract constraints:

- derived from one shared canonical helper, not recomputed independently per mutation path
- fields must be stable and machine-readable
- absent dimensions must be explicit (`[]` or `null`), not omitted arbitrarily

## Non-Goals For This Wave

Deferred roadmap items:

- native `delivery_batch` objects
- intent-to-delivery projection views
- mode-aware operator sessions
- full execution-proof workflow family
- any broad meta-tool that overlaps with `soll_manager` without a bounded mutation contract

Those remain later waves because they add schema and lifecycle complexity beyond the first leverage point.
