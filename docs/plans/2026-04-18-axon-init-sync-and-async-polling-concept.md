# Axon MCP Client UX Hardening: `axon_init_project` Sync + Async Polling Guidance

## Context

Client feedback showed a recurring failure mode in Axon MCP:

- a public tool is visible
- the server returns a job identifier
- the LLM does not know exactly what to do next
- the LLM stops, or improvises an incorrect fallback path

The immediate trigger was `axon_init_project`, but the underlying issue is broader:

- short operations should not force async orchestration
- true async operations need a stronger follow-up contract than `job_id` alone

## Decision

### 1. `axon_init_project` becomes synchronously canonical

`axon_init_project` is:

- rare
- short
- identity-bearing
- almost always called because the client wants `project_code` immediately

Therefore it should not be routed through mutation jobs.

The public product surface should expose exactly one canonical tool:

- `axon_init_project`

It must return directly:

- `project_code`
- `project_name`
- `project_path`

The temporary wrapper `axon_init_project_and_wait` should be removed.

Guardrail:

- if `axon_init_project` later stops being predictably short or accumulates materially long side effects, this product decision must be revisited explicitly rather than drifting back to async implicitly

### 2. Async stays only for materially long operations

Async remains justified for tools with real runtime duration or variable latency, notably:

- `soll_manager`
- `soll_apply_plan`
- `soll_commit_revision`
- other genuinely long or heavy mutation paths

The guiding rule is:

- sync by default for short deterministic actions
- async only when runtime cost justifies it

### 3. Async acceptance must teach the LLM how to proceed

For true async public mutations, `job_id` is not enough.

The acceptance payload must include machine-usable polling guidance:

- canonical follow-up tool
- how soon to poll
- how often to poll
- terminal states
- what to read on success
- what to read on failure

This guidance must be explicit enough that an LLM can continue without guessing.

Normative rule:

- `next_action` remains the canonical machine-action field
- `polling_guidance` is supplemental guidance that explains how to use `next_action` correctly

## Constraints

- Keep the change minimal.
- Do not redesign the whole mutation framework.
- Preserve the existing async model for long mutations.
- Improve contract clarity for both server truth and client follow-up behavior.
- Keep `job_status` as the single canonical follow-up tool for async jobs.

## Non-goals

- No transport redesign.
- No client-specific binding enforcement system.
- No broad reclassification of MCP tools in this change.
- No promotion to live in this phase.

## Reuse vs Change

### Reuse

- existing direct implementation of `axon_init_project`
- existing async job infrastructure
- existing `job_status`
- existing validator and MCP test harness

### Change

- remove public sync wrapper duplication
- stop treating `axon_init_project` as a mutation job candidate
- enrich async acceptance payload with stronger polling instructions
- realign docs, tests, and validator with the simplified contract

## Expected Outcome

After this change:

- `axon_init_project` is the only public init tool
- it returns canonical identity immediately
- true async tools remain async
- their acceptance payload tells the LLM exactly how to proceed next
- the product surface becomes simpler and more fluid for clients
- the normal happy path does not require `mcp_surface_diagnostics`
