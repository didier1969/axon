# Implementation Plan: MCP `sync` / `async` Policy

## Scope

Apply the two-flow product rule to the public Axon MCP surface:

- almost everything synchronous
- only a small allowlist asynchronous

## Dependency Order

1. Freeze the policy and classification
2. Encode the async allowlist centrally
3. Expose the active policy in server truth
4. Remove any residual async treatment from tools that should be sync
5. Keep the async contract uniform for the remaining async tools
6. Update tests
7. Update validation scripts
8. Update the Axon skill / docs
9. Requalify the MCP surface

## Tasks

### Task 1: Central Async Allowlist

Replace broad mutation-job routing with an explicit async allowlist.

Initial allowlist target:

- `soll_apply_plan`
- `restore_soll`
- `resume_vectorization`

Potential review candidate:

- keep `soll_commit_revision` sync unless runtime evidence contradicts that choice

Rule:

- this allowlist is the authoritative routing mechanism
- latency measurements inform review, but do not override semantic hard-async triggers by themselves

### Task 2: Expose Policy in Server Truth

- update `status` and any related diagnostics surface so clients can discover the active async policy from the server itself
- make it possible to verify that no public mutation tool outside the allowlist still returns `job_id`

### Task 3: Force Sync for Lightweight Mutations

Ensure these remain synchronous even if mutation jobs are globally enabled:

- `axon_init_project`
- `soll_manager`
- `soll_attach_evidence`
- `soll_commit_revision`
- `soll_rollback_revision`
- `soll_export`
- `axon_apply_guidelines`
- `axon_pre_flight_check`
- `axon_commit_work`

### Task 4: Uniform Async Contract

For the allowlisted async tools only, preserve and validate:

- `job_id`
- `state`
- `next_action`
- `result_contract`
- `polling_guidance`
- `recovery_hint`

### Task 5: Tests

Add or adjust tests for:

- sync tools staying sync under mutation-job mode
- async allowlist tools still returning job acceptance
- `job_status` remaining the canonical follow-up
- `polling_guidance` content
- no public mutation tool outside the allowlist returning `job_id`

### Task 6: Validation

Update `scripts/mcp_validate.py` to:

- treat only the allowlisted tools as async
- assert sync behavior for the lightweight mutation tools
- assert the full async envelope for the allowlisted tools

Measurement doctrine for validation:

- use the agreed target runtime behavior where possible
- distinguish cold vs steady-state when relevant
- treat repeated budget failure on a sync tool as a reclassification trigger

### Task 7: Docs / Skill

Update the Axon skill to:

- document the two-flow rule
- list the async exceptions explicitly
- state that all other tools are immediate

## Validation Matrix

### Code-level

- targeted Rust tests for routing and contract behavior
- validator updates checked locally

### Runtime-level

On `dev`, once healthy:

- full MCP validation on core + SOLL surfaces
- explicit checks for sync mutation tools
- explicit checks for async allowlist tools
- explicit checks that `soll_commit_revision` still deserves its sync classification

## Risks

### Risk 1: Over-synchronizing a tool with hidden runtime variance

Mitigation:

- use both semantic classification and measured latency
- keep `review` status for borderline tools

### Risk 2: Inconsistent async handling

Mitigation:

- centralize async allowlist
- centralize polling guidance generation

### Risk 3: Drift between product doctrine and validator/docs

Mitigation:

- change code, tests, validator, and skill in one wave

## Rollback

- revert the allowlist/routing wave as one unit if runtime requalification contradicts the policy
