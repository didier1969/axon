# Implementation Plan: `axon_init_project` Sync + Async Polling Guidance

## Scope

Apply the approved concept to the Axon MCP product surface:

- remove the duplicate public sync wrapper
- keep `axon_init_project` synchronous
- improve the async follow-up contract for genuinely long mutations

## Dependency Order

1. Simplify dispatch and catalog
2. Remove duplicate wrapper tool and its references
3. Realign diagnostics and status truth
4. Strengthen async acceptance contract
5. Update tests
6. Update validator
7. Update skill / operator docs
8. Validate
9. CDD review of implementation

## Tasks

### Task 1: Dispatch / Catalog Simplification

- remove `axon_init_project_and_wait` from:
  - catalog
  - dispatch
  - public surface expectations
- ensure `axon_init_project` is not treated as a mutation job tool

### Task 2: Async Contract Guidance

For true async job acceptance responses, add a structured `polling_guidance` block containing:

- `when_to_poll`
- `poll_interval_seconds`
- `until_states`
- `max_wait_hint_seconds`
- `on_completed`
- `on_failed`

Keep:

- `job_id`
- `state`
- `next_action`
- `result_contract`
- `recovery_hint`

Rule:

- `next_action` stays canonical
- `polling_guidance` is additive guidance layered on top of the existing async envelope

### Task 3: Diagnostics / Status Truth

- update `status.async_contract` so it remains truthful after wrapper removal
- update `mcp_surface_diagnostics` so it no longer references the removed wrapper and still points clients toward the canonical identity and async-follow-up flow

### Task 4: Tests

Update tests to reflect the new canonical surface:

- remove wrapper expectations
- assert `AXON_MCP_MUTATION_JOBS=true` no longer affects `axon_init_project`
- assert `axon_init_project` returns canonical identity directly in `data`
- assert async mutation tools still expose:
  - `next_action.tool = job_status`
  - `polling_guidance`

### Task 5: Validator

Update `scripts/mcp_validate.py`:

- stop expecting `axon_init_project_and_wait`
- validate the stronger async acceptance contract for true async tools
- validate `status.async_contract` remains coherent

### Task 6: Skill / Docs

Update Axon skill:

- remove `axon_init_project_and_wait`
- make `axon_init_project` the one-shot canonical init tool
- state that async tools must be followed through `job_status` using the returned polling guidance
- ensure no public doc, catalog, validator, or test still references `axon_init_project_and_wait`

## Validation Matrix

### Code-level validation

- targeted Rust MCP tests for:
  - tool listing
  - sync `axon_init_project`
  - async mutation contract
  - diagnostics/status async guidance

### Runtime-level validation

On `dev`, once runtime is healthy:

- MCP validation on core surface
- explicit check that `axon_init_project` returns identity directly
- explicit check that async tools return `polling_guidance`

## Risks

### Risk 1: Drift in async guidance

Mitigation:

- centralize polling guidance generation in `mcp.rs`
- test exact fields

### Risk 2: Hidden dependency on async `axon_init_project`

Mitigation:

- keep direct implementation unchanged
- validate catalog, tests, and validator together

### Risk 3: False completion claim without runtime proof

Mitigation:

- separate code-level success from runtime-level success
- do not claim full completion if `dev` runtime remains degraded

## Rollback / Containment

- Changes are localized to MCP surface and validation layers.
- If needed, revert the catalog/dispatch/docs/tests/validator wave together as one unit.
