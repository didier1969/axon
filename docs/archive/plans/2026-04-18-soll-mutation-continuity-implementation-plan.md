# Implementation Plan: SOLL Mutation Continuity

## Scope

Fix the remaining real-project SOLL server-side frictions:

- canonical project identity accepted consistently
- `soll_apply_plan` reliable in normal agent flows
- relation policy discoverable and actionable

## Dependency Order

1. Reproduce and isolate the canonical `project_code` failure path
2. Fix `soll_apply_plan` identity continuity
3. Enrich `soll_manager link` rejection guidance
4. Add explicit relation discovery tool if needed
5. Update tests
6. Update skill/docs
7. Validate on a realistic project flow

## Tasks

### Task 1: Reproduce The `soll_apply_plan` Identity Failure

Add or identify a test that models the real sequence:

1. initialize project
2. resolve project via `project_registry_lookup`
3. call `soll_apply_plan` with the returned canonical `project_code`
4. confirm preview creation succeeds

The test must capture the exact failure class seen by the client.
It must also cover:

- same-process continuity
- fresh-runtime continuity

### Task 2: Fix Identity Continuity In `soll_apply_plan`

Inspect and correct any mismatch between:

- `require_registered_mutation_project_code`
- `resolve_canonical_project_identity_for_mutation`
- `next_server_numeric_id`
- preview persistence payload

Rule:

- once a project code is accepted by lookup and mutation registration, the batch path must not reject it later in the same logical flow

### Task 3: Enrich `soll_manager link` Rejection Guidance

When `link` fails, return structured error data describing:

- `source_kind`
- `target_kind`
- `requested_relation`
- `allowed_relations`
- `default_relation`
- `suggested_next_actions`

These fields must be machine-readable and not prose-only.

Differentiate clearly:

- illegal pair
- legal pair but forbidden requested relation
- legal pair but explicit relation required

### Task 4: Add Relation Discovery Tool

Add a public tool such as `soll_relation_schema`.

Minimum capabilities:

- lookup by `source_type` + `target_type`
- lookup by `source_id` + `target_id`
- list allowed target kinds and default relations from a given source kind

If this tool is not implemented in this wave, validation must prove that enriched `soll_manager link` errors are sufficient to complete one-step correction without trial-and-error.

### Task 5: Post-Create Guidance

For created SOLL nodes, consider returning lightweight guidance:

- created id
- node type
- canonical next link hints

This should be additive and not break existing clients.
These hints remain advisory and must derive from the same enforced relation policy truth.

### Task 6: Tests

Add or update tests for:

- `soll_apply_plan` succeeding for a freshly initialized project with canonical code
- `soll_manager link` structured rejection hints
- relation-schema discovery outputs
- consistency between relation discovery and actual link enforcement

### Task 7: Docs / Skill

Update:

- Axon skill
- tool catalog if needed
- operator/client notes if the new relation discovery surface is introduced

## Validation Matrix

### Code-level

- targeted Rust tests for identity continuity and relation guidance

### Runtime-level

- real MCP call sequence on `dev`:
  - `axon_init_project`
  - `project_registry_lookup`
  - `soll_apply_plan`
  - `soll_manager link` error guidance
  - relation discovery tool if added

### Product-level

- verify the agent can seed a project concept without falling back to blind trial-and-error

## Risks

### Risk 1: Overfitting to one project sequence

Mitigation:

- test both freshly initialized and already registered projects

### Risk 2: Relation guidance drifting from real enforcement

Mitigation:

- generate guidance from the same policy source used by link enforcement

### Risk 3: Adding too much prose instead of machine-readable guidance

Mitigation:

- prioritize structured fields first
- keep prose secondary

## Rollback

- revert the SOLL continuity wave as one unit if it causes mutation regressions
- preserve canonical identity enforcement and relation policy source of truth
