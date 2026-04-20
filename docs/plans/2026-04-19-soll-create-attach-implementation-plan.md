# SOLL Create+Attach Implementation Plan

Date: 2026-04-19
Branch: `feat/live-dev-dual-instance`
Status: CDD Phase 2

## Objective

Add a minimal create+canonical-attach workflow to `soll_manager create` by reusing the existing relation enforcement path.

## Phase 1. Contract Extension

1. Extend `soll_manager create` input handling to accept:
   - `data.attach_to`
   - `data.relation_hint`
   - explicitly documented as graph attach only, not evidence attachment
2. Define response fields:
   - `created_id`
   - `attached`
   - `attached_to`
   - `applied_relation`
   - `attach_attempted`
   - `attach_status`
   - `attach_guidance` when attach fails or needs a hint

## Phase 2. Runtime Implementation

1. Keep existing node creation logic unchanged.
2. After a successful create:
   - if no `attach_to`, return the current create contract
   - if `attach_to` exists:
     - resolve relation using `select_relation_type_for_link(new_id, attach_to, relation_hint)`
     - if valid, insert via `insert_validated_relation(...)`
     - if invalid or ambiguous, do not rollback the created node in this first wave
3. Return explicit outcome:
   - `attached = true` when edge creation succeeded
   - `attached = false` plus guidance when attach did not happen

## Phase 3. Tests

Add targeted tests for:

1. create requirement attached to pillar
   - edge created with `BELONGS_TO`
2. create pillar attached to vision
   - edge created with `EPITOMIZES`
3. create validation attached to requirement
   - edge created with `VERIFIES`
4. create decision attached to requirement without hint
   - create succeeds
   - attach is refused or paused with `attach_status = needs_relation_hint`
   - guidance exposes `SOLVES` and `REFINES`
5. create with invalid target kind
   - create succeeds
   - attach fails truthfully with guidance

## Phase 4. Skill / Validation

1. Update `axon-engineering-protocol`:
   - mention `soll_manager create` can optionally canonical-attach
2. Validate with:
   - targeted Rust tests
   - runtime MCP probe on `dev`
   - one real create+attach path against the endpoint

## Rollout Rule

First wave only:

- no rollback-on-attach-failure
- no async behavior
- no new tool

If qualification shows that partial success is too awkward, a later wave can add:

- `atomic_create_and_attach`
- or explicit transactional attach mode

But not in this wave unless evidence forces it.
