# Agent-Native Delivery Roadmap Implementation Plan

## Wave 1

### 1. Add assistive inference surface

Implement `infer_soll_mutation` as a read-only tool that:

- resolves canonical project scope
- scores likely impacted nodes from the statement
- proposes update-vs-create guidance
- returns confidence and ambiguity warnings
- returns next-action guidance
- never reserves IDs or mutates state

### 2. Add a high-level mutation primitive

Implement `entrench_nuance` as a bounded wrapper that:

- proposes first by default
- requires explicit confirmation for write mode
- updates existing canonical nodes only in wave 1
- stores nuance only in canonical fields or in an explicitly canonical server-owned metadata shape
- returns the same mutation feedback contract as native SOLL mutations

Wave-1 contract:

- `confirm=false|omitted` -> proposal only
- `confirm=true` -> apply bounded updates
- no implicit write on proposal replay
- no silent entity creation in wave 1

### 3. Enrich native mutation contracts

For successful `soll_manager` mutations:

- capture canonical SOLL completeness before and after
- compute unblocked and remaining blocker sets
- attach a structured `mutation_feedback` payload

Minimum stable payload:

- `changed_entities`
- `topology_delta`
- `newly_unblocked`
- `remaining_blockers`
- `next_best_actions`
- `completeness_before`
- `completeness_after`

Implementation rule:

- compute these fields through one shared canonical helper

### 4. Documentation and skill alignment

Update:

- MCP catalog descriptions
- Axon skill routing and delivery guidance

### 5. Validation

Add targeted tests for:

- project resolution for inference
- ambiguity surfacing for inference
- usefulness of returned next actions
- confirmation-required entrenchment flow
- confirmed entrenchment update
- enriched mutation feedback on create/link/update
- one end-to-end propose -> confirm -> mutate -> feedback loop on a real project-scoped example

## Explicit Later Waves

### Wave 2

- native `delivery_batch`
- `open_delivery_batch`
- `close_delivery_batch`
- `review_delivery_batch`

### Wave 3

- `intent_to_delivery_projection`
- execution/evidence projection views

### Wave 4

- explicit operator/agent session modes
- proof-sync and retrospective workflow helpers
