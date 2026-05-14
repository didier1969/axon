# Canonical Projection Policy Strengthening Implementation Plan

Date: 2026-04-19
Status: plan
Method: idea-to-delivery

## Objective

Strengthen Axon’s canonical structural policy so that derived SOLL documentation can eventually project from canonical schema/policy semantics instead of UI-local hierarchy rules, while keeping current docs quality high throughout the transition.

## Delivery Strategy

Use a phased strategy:

- Phase A: preserve immediate documentation quality
- Phase B: enrich canonical policy
- Phase C: introduce schema-driven projection in parallel
- Phase D: prove parity
- Phase E: remove UI-local hierarchy logic

This is explicitly not a big-bang rewrite.

## Phase A: Stabilize the Existing High-Quality Output

### A1. Freeze the current derived docs UI contract

Treat the current 3-pane derived docs UI as the baseline contract.

Capture in tests:

- `GLO` global root
- project root hierarchy page
- focus node pages
- canonical HTML navigation
- pane collapse/resize controls
- left tree semantics

Purpose:

- protect user-visible quality while the structural layer evolves

### A2. Mark the current hierarchy helper as transitional

Document clearly in code and docs:

- current hierarchy helper is transitional bootstrap logic
- it must not expand casually while canonical policy strengthening is pending

## Phase B: Enrich Canonical Structural Policy

### B1. Introduce a canonical projection metadata model

Extend the relation policy model so it can carry:

- `projection_role`
  - `primary`
  - `lateral`
  - `supporting`
- `parent_preference_rank`
- `breadcrumb_eligible`
- `root_eligible`
- `child_order_rank`

This should stay inside the canonical structural source, not in a separate docs config.

### B2. Keep validity and projection distinct

Refactor the internal relation policy representation to separate:

- validity semantics
- projection semantics

The API may still expose one object, but its fields must be conceptually separated.

### B3. Expose projection metadata through a canonical helper

Create one canonical server-side helper that returns:

- valid relations
- default relation
- projection semantics

This helper becomes the source for:

- `soll_relation_schema`
- docs projection
- future higher-level navigation/guidance tools

## Phase C: Add Schema-Driven Projection Without Removing the Bootstrap

### C1. Build a projection adapter over canonical policy

Create a dedicated projection adapter that consumes:

- canonical projection metadata
- actual SOLL nodes
- actual SOLL edges

and yields:

- preferred parents
- visible children
- breadcrumb parents
- lateral/supporting detail links
- tree inclusion/exclusion decisions

Discipline:

- keep the canonical helper minimal
- do not collapse mutation guidance, validation, and docs projection into one opaque super-helper
- prefer:
  - enriched canonical policy
  - small canonical read helper
  - thin derived projection adapter

### C2. Keep current UI shell unchanged

Do not rewrite the user-facing docs UI in this phase.

Only swap where the structure comes from.

That way:

- layout stays stable
- tests stay focused
- migration risk is lower

### C3. Preserve full fallback behavior

If projection metadata is incomplete for some types:

- fallback to the current bootstrap helper
- but emit explicit internal diagnostics in tests/logging

Fallback policy:

- deterministic
- temporary by design
- instrumented so its usage is visible in tests and runtime diagnostics
- not exposed as confusing reader-facing noise in the docs UI

Do not fail open silently.

## Phase D: Parity and Validation

### D1. Add parity tests

For representative projects, compare:

- current bootstrap projection output
- schema-driven projection output

Check at least:

- root pages
- tree ancestry
- breadcrumb parents
- parent/child focus pages
- lateral/supporting detail placement

User-facing parity must explicitly mean:

- same breadcrumb path
- same parent focus page
- same first-level children set
- same lateral/supporting placement
- same project root reading path

### D2. Add stability guarantees

Introduce tests that ensure:

- the same node does not oscillate between parents across regenerations
- deterministic ordering is preserved
- multi-parent nodes remain stable

### D3. Validate on real project surfaces

Run comparative validation on:

- `AXO`
- at least one second non-trivial project with different SOLL topology

The parity gate must fail closed if fallback usage remains unexpectedly high after canonical enrichment.

## Phase E: Remove the Docs-Only Hierarchy Truth

### E1. Delete UI-local hierarchy hardcoding

After parity is proven, remove:

- docs-only hierarchy type tables
- docs-only parent selection rules

The generator should then rely only on:

- canonical structural policy
- projection adapter

### E2. Update skill and operator docs

Realign:

- `axon-engineering-protocol` skill
- relevant plan docs
- any docs describing `soll_generate_docs`

to state that hierarchy is canonical-policy-driven.

## Validation Matrix

### Code-level

- canonical relation policy tests
- projection metadata tests
- `soll_relation_schema` tests
- docs generation tests
- parity tests

### Product-level

- navigation remains coherent
- no regression in derived docs readability
- `GLO` remains derived unless canon explicitly models a portfolio layer
- projects with no docs do not become broken links in the tree

### Runtime-level

- `soll_generate_docs` still works incrementally
- auto-refresh after mutation still works
- root and project manifests remain truthful

## Open Design Decisions To Resolve During Execution

### 1. Where projection metadata lives

Options:

- enrich `RelationPolicy` directly
- or define a small companion metadata structure returned by the canonical helper

Preferred default:

- keep one canonical helper API, even if internally split

### 2. How to represent root eligibility

Need to decide whether:

- `Project` and maybe some future types are marked root-eligible canonically
- `GLO` remains permanently outside canon as a reader-level portfolio root

Default recommendation:

- keep `GLO` derived until there is an explicit canonical portfolio model

## Risks

### 1. Overloading the canonical policy

Mitigation:

- keep projection metadata minimal
- do not push UI styling concerns into canon

### 2. Migration drift

Mitigation:

- run parity comparisons before switching over

### 3. Partial rollout confusion

Mitigation:

- one generator
- one UI shell
- one migration flag internally if needed
- no split public contract

## Completion Criteria

This plan is complete when:

- canonical policy carries enough projection metadata
- docs projection can be derived from canonical structure
- parity with current high-quality docs is proven
- UI-local hierarchy logic is removed
- documentation quality remains high throughout
