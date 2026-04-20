# Axon MCP Client Feedback Response Plan

Date: 2026-04-18
Status: draft
Scope: external LLM agent experience on `axon-live` and `axon-dev`
Source: [feedback-axon-mcp.md](/home/dstadel/projects/nutri-opti/feedback-axon-mcp.md)

## Executive Summary

The client feedback is materially correct.

Axon does not need a platform rewrite.
It needs a tighter LLM-facing contract around:

- structured async continuation
- canonical identity discovery
- stable machine-readable result envelopes
- simpler happy-path wrappers for common mutation workflows

The minimum viable correction is additive and local.
It does not require replacing the current MCP engine, SQL gateway, or job infrastructure.

## What Is Already Good

- the MCP transport is now usable and negotiates the current protocol version
- the public tool surface is rich and mostly discoverable
- `job_status` already exists and is public
- `axon_init_project` already has a synchronous implementation path that can return:
  - `project_code`
  - `project_name`
  - `project_path`
- the server already has a unified job store via `soll.McpJob`
- live/dev runtime identity is already explicit through `status`

So the engine is not the problem.
The main problem is orchestration fluency.

## What The Feedback Gets Right

### 1. `axon_init_project` is still ergonomically ambiguous

The direct implementation returns `data.project_code`, but in job mode the public response can collapse to:

- `accepted`
- `job_id`
- `status`

This forces the agent to infer too much.

### 2. The async contract is not explicit enough

Agents need one stable pattern:

- immediate accepted payload
- one canonical follow-up tool
- stable terminal result envelope

Axon has the pieces, but not yet the uniform public contract.

### 3. Structured data is still not privileged enough

Some responses are still optimized for prose first and structured fields second.
That is acceptable for humans, but suboptimal for generic agent clients.

### 4. Project identity discovery is missing as a first-class tool

The agent should not need to infer `project_code` through search or context accidents.

### 5. The happy path is too long

The server is powerful, but normal agent workflows still require too much orchestration knowledge.

## What Does Not Need To Change

Do not:

- rewrite the MCP transport
- replace `soll.McpJob`
- remove async jobs
- collapse live/dev into one runtime
- redesign the full tool catalog
- remove low-level diagnostic tools

Those would be high-cost changes with low leverage against the actual complaint.

## Minimal-Change Strategy

This strategy is intentionally narrower than the broader LLM-facing MCP hardening program.

It targets the client complaint first:

- truthful async continuation
- stable machine-readable results
- explicit project identity discovery
- short public happy path

It does not try to solve in the same wave:

- full public/expert policy architecture
- dispatch-time policy enforcement
- expert-mode negotiation
- `/sql` containment redesign

Those remain legitimate follow-up workstreams, but they are not required for the minimum safe response to this client.

## Wave 1: Fix The Public Async Contract

### Goal

Make all public async mutations feel predictable without changing the underlying execution engine.

### Changes

1. Standardize the acceptance envelope for every async public mutation:
   - `accepted`
   - `job_id`
   - `tool_name`
   - `status`
   - `reserved_ids`
   - `known_ids`
   - `next_action`
   - `result_contract`
   - `recovery_hint`

2. Guarantee that `job_status(job_id)` is the canonical follow-up everywhere.

3. Ensure every terminal `job_status` result contains:
   - `status`
   - `started_at`
   - `finished_at`
   - `error_text`
   - `result`

4. Ensure critical identifiers appear in `data` immediately whenever already known at acceptance time.

Examples:

- `axon_init_project`
  - `project_code`
  - `project_name`
  - `project_path`
- `soll_apply_plan`
  - `preview_id` when already reserved
- `soll_commit_revision`
  - `revision_id` when already reserved

### Expected impact

High.
This alone removes most of the ambiguity described by the client.

## Wave 2: Make Identity Discovery First-Class

### Goal

Let agents resolve projects without inference.

### Changes

Add one public tool:

- `project_registry_lookup`

Supported lookups:

- by `project_code`
- by `project_name`
- by `project_path`

Structured response:

- `project_code`
- `project_name`
- `project_path`
- `found`
- `matches`

### Expected impact

High.
This closes the identity gap without touching existing mutation tools.
It also removes a large class of search detours and guessed identifiers.

## Wave 3: Add Agent-Happy-Path Composite Tools

### Goal

Reduce orchestration burden for the most common agent workflows.

### Changes

Add wrappers:

- `init_project_and_wait`
- `apply_soll_plan_and_wait`
- `commit_revision_and_wait`

These are convenience tools only.
They should reuse existing public primitives:

- `axon_init_project`
- `soll_apply_plan`
- `soll_commit_revision`
- `job_status`

### Expected impact

Medium to high.
Not strictly necessary for correctness, but very strong for DX.

## Wave 4: Tighten Catalog Wording And Qualification

### Goal

Align public docs, public metadata, and qualification scenarios.

### Changes

1. Update [catalog.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp/catalog.rs)
   - no over-promises on sync results when a tool may be async
   - explicit continuation semantics

2. Update [SKILL.md](/home/dstadel/projects/axon/docs/skills/axon-engineering-protocol/SKILL.md)
   - explicit `init -> job_status -> result` pattern

3. Extend MCP qualification:
   - async acceptance contains `next_action`
   - `job_status` terminal result shape is stable
   - project identity lookup works
   - `init_project_and_wait` works if present
   - a fresh external MCP client can complete `init -> job_status -> result`
   - no normal flow requires raw `/sql`
   - no normal flow requires source reading
   - no normal flow requires guessed hidden-tool names

### Expected impact

Medium.
This converts the fix into a stable product contract.

## Recommendation

Implement Waves 1 and 2 first.

That is the real minimum viable correction.
It is mostly additive, backward-compatible, and directly addresses the client feedback.

Then implement Wave 3 if we want a truly ergonomic agent API.

Wave 4 should happen in the same delivery wave as 1 and 2 or immediately after.

## Explicit Staging Decision

The implementation order should be:

1. make the current public contract truthful
2. make async continuation uniform and machine-readable
3. add explicit project lookup
4. upgrade qualification and external-binding checks
5. only then add convenience wrappers

This staging keeps the first correction small and prevents coupling it to a larger runtime policy redesign.

## Concrete Answer To The Client

The correct product answer is:

- no, Axon does not need to be rebuilt
- yes, the client identified a real friction in the agent contract
- the right fix is to standardize async continuation, expose explicit project lookup, and privilege stable structured data over prose

## Acceptance Criteria

This feedback is considered addressed when:

1. An agent can call `axon_init_project` and always complete the flow through public tools only.
2. An agent can resolve a project identity directly through a dedicated lookup tool.
3. Every async public mutation advertises its next legal action in machine-readable form.
4. Critical IDs are returned immediately in `data` whenever they are already known at acceptance time.
5. No normal workflow requires source reading, raw `/sql`, or hidden-name guessing.
6. A fresh external MCP client can complete the canonical async flow without repository-local knowledge.
7. Qualification scenarios assert those guarantees.
