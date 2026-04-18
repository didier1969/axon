# LLM-Facing MCP UX Hardening Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

Date: 2026-04-18
Status: draft-for-review
Scope: Public Axon MCP product experience for external LLM clients on `axon-live` and `axon-dev`

## Goal

Make the public Axon MCP surface sufficiently truthful, self-guiding, and enforceable that a general-purpose LLM can complete normal developer workflows without:

- guessing hidden tools
- constructing undeclared MCP calls
- reading Axon source to reconstruct tool behavior
- using `/sql` as a normal recovery path

## Design Rules

1. Public contracts must match runtime reality.
2. Public async operations must have a public continuation.
3. Public/internal separation must be enforced at dispatch, not only hidden in discovery.
4. The normal MCP transport is not the problem; undeclared hidden calls are.
5. `status` is mandatory at:
   - session start
   - reconnect
   - capability/runtime change
   - after degraded/unknown-mode errors
6. Public tools must be sufficient for normal work.
7. The server must carry the burden of next-step guidance.
8. Expert diagnostics may remain, but must be clearly exceptional.
9. Qualification must test experience fluency, not only raw availability.
10. Public sync vs async behavior must not be hidden behind environment drift; if it remains configurable, it must be advertised and tested as a capability.

## Phase 1: Audit and Normalize the Public Contract

### Task 1.1: Build a public contract inventory

**Files**
- Create: `docs/plans/2026-04-18-llm-facing-mcp-public-contract-matrix.md`
- Read/compare:
  - [src/axon-core/src/mcp/catalog.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp/catalog.rs)
  - [src/axon-core/src/mcp.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp.rs)
  - [src/axon-core/src/mcp/tools_soll.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp/tools_soll.rs)
  - [docs/skills/axon-engineering-protocol/SKILL.md](/home/dstadel/projects/axon/docs/skills/axon-engineering-protocol/SKILL.md)

**Intent**
- enumerate each public tool
- compare declared contract vs runtime behavior
- flag sync/async divergence
- flag missing continuation semantics

**First-wave public tools**
- `status`
- `project_status`
- `axon_init_project`
- `axon_apply_guidelines`
- `axon_commit_work`
- `soll_manager`
- `soll_apply_plan`
- `soll_commit_revision`
- `soll_rollback_revision`
- `soll_attach_evidence`
- `soll_export`
- `restore_soll`
- `soll_query_context`
- `soll_validate`
- `soll_verify_requirements`

**Acceptance criteria**
- every public tool is classified as:
  - sync
  - async
  - env-gated sync/async
- each mismatch is recorded with an implementation owner
- `AXON_MCP_MUTATION_JOBS` is either removed from the public contract or made explicit in the matrix and later `status` metadata

### Task 1.2: Normalize catalog and skill wording

**Files**
- Modify: [src/axon-core/src/mcp/catalog.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp/catalog.rs)
- Modify: [docs/skills/axon-engineering-protocol/SKILL.md](/home/dstadel/projects/axon/docs/skills/axon-engineering-protocol/SKILL.md)

**Intent**
- remove any public description that over-promises immediate data on async tools
- describe the canonical continuation explicitly

**Acceptance criteria**
- public descriptions no longer imply sync results when the runtime is async
- `axon_init_project` wording is truthful in both catalog and skill

## Phase 2: Define the Policy Carrier and Public Async Pattern

### Task 2.0: Decide the execution-policy architecture

**Files**
- Modify: [src/axon-core/src/mcp.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp.rs)
- Modify: [src/axon-core/src/mcp_http.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp_http.rs)
- Modify: [src/axon-core/src/mcp/tools_framework.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp/tools_framework.rs)
- Tests: [src/axon-core/src/mcp/tests.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp/tests.rs)

**Intent**
Decide the authoritative policy carrier for:
- public mode
- expert mode
- `/sql` eligibility
- sync vs async mutation behavior

**Required decisions**
- whether policy is runtime-wide, session-scoped, request-scoped, or endpoint-scoped
- how `tools/call` sees the active policy
- how `/sql` sees the active policy
- how `status` advertises the active policy

**Acceptance criteria**
- enforcement phases have a concrete policy carrier
- status can describe the active execution policy to any client

### Task 2.1: Define a canonical async acceptance envelope

**Files**
- Modify: [src/axon-core/src/mcp.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp.rs)
- Tests: [src/axon-core/src/mcp/tests.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp/tests.rs)

**Intent**
Every public async mutation response must include:
- `accepted`
- `job_id`
- `tool_name`
- `status`
- `reserved_ids`
- `next_action`
- `result_contract`
- `recovery_hint`

**Required shape**
- `next_action.tool` must name a public follow-up tool
- `next_action.arguments` must be machine-usable
- `result_contract` must say what final data shape to expect

**Acceptance criteria**
- all public async mutation tools return the same envelope family
- no async response leaves the next step implicit

### Task 2.1b: Migrate harnesses and tests off hidden `job_status`

**Files**
- Modify: [scripts/mcp_validate.py](/home/dstadel/projects/axon/scripts/mcp_validate.py)
- Modify: [src/axon-core/src/mcp/tests.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp/tests.rs)

**Intent**
Prepare current hidden-tool-dependent harnesses and tests to consume the new public follow-up tool in the same implementation wave.

**Acceptance criteria**
- the migration path away from hidden `job_status` is implemented together with the new public follow-up tool
- qualification and core server tests no longer require hidden follow-up in public-flow cases once that wave lands

### Task 2.2: Expose a public follow-up tool

**Files**
- Modify: [src/axon-core/src/mcp/catalog.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp/catalog.rs)
- Modify: [src/axon-core/src/mcp.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp.rs)
- Tests: [src/axon-core/src/mcp/tests.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp/tests.rs)

**Intent**
Introduce one canonical public follow-up:
- either `mutation_status`
- or rename/promote the current `job_status` under a public contract

**Rules**
- the public follow-up tool must be discoverable in normal `tools/list`
- the public follow-up tool must return stable machine-readable state
- the tool must safely expose final result data for async public mutations

**Acceptance criteria**
- an external client can finish `axon_init_project` without guessing hidden tools
- `job_status` is no longer a shadow dependency of public flows

### Task 2.3: Make `axon_init_project` publicly coherent

**Files**
- Modify: [src/axon-core/src/mcp.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp.rs)
- Modify: [src/axon-core/src/mcp/tools_soll.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp/tools_soll.rs)
- Tests: [src/axon-core/src/mcp/tests.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp/tests.rs)

**Intent**
Choose one truthful public model:
- synchronous return with `project_code`
- or async return with canonical public continuation

**Acceptance criteria**
- the public contract is unambiguous
- the returned or eventual `project_code` is obtainable through public tools only

## Phase 3: Expose Capabilities and Enforce Public/Internal Boundaries

### Task 3.1: Expose capability metadata in `status`

**Files**
- Modify: [src/axon-core/src/mcp/tools_framework.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp/tools_framework.rs)
- Tests: [src/axon-core/src/mcp/tests.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp/tests.rs)

**Intent**
Make the authoritative policy discoverable by any MCP client.

**Required `status` fields**
- `surface_mode`
- `expert_mode_available`
- `expert_mode_active`
- `sql_mode`
- `mutation_mode`
- `async_pattern_version`
- `public_follow_up_tool`
- `public_tool_policy`
- `mutation_jobs_mode`

**Acceptance criteria**
- no client needs repo-local documentation to discover the active policy
- sync vs async mutation mode is machine-visible

### Task 3.2: Add dispatch-time enforcement for public mode

**Files**
- Modify: [src/axon-core/src/mcp.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp.rs)
- Modify: [src/axon-core/src/mcp/catalog.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp/catalog.rs)
- Tests: [src/axon-core/src/mcp/tests.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp/tests.rs)

**Intent**
Prevent normal public calls from executing hidden/internal tools even if the client guesses the name.

**Rules**
- normal public mode may execute only tools surfaced by public `tools/list`
- internal tools require explicit expert mode or equivalent server-side flag

**Acceptance criteria**
- guessed hidden-tool calls fail cleanly in public mode
- public/internal distinction is enforced by runtime behavior

### Task 3.3: Implement and expose expert-mode negotiation

**Files**
- Modify: [src/axon-core/src/mcp/catalog.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp/catalog.rs)
- Modify: [src/axon-core/src/mcp.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp.rs)
- Modify: [src/axon-core/src/mcp/tools_framework.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp/tools_framework.rs)
- Tests: [src/axon-core/src/mcp/tests.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp/tests.rs)

**Intent**
Implement the architecture chosen in Task 2.0 so public vs expert mode becomes explicit and machine-readable.

**Required decisions**
- how expert mode is advertised
- how expert mode is enabled
- whether it is per runtime, per session, or per request
- what capability deltas apply under expert mode

**Acceptance criteria**
- a client can tell from `status` whether it is in public or expert mode
- expert-mode behavior is documented and testable

### Task 3.4: Contain `/sql` as diagnostic-only

**Files**
- Modify: [src/axon-core/src/mcp_http.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp_http.rs)
- Docs: operator notes if needed

**Intent**
Reduce `/sql` as a normal LLM escape hatch.

**Options to implement**
- restrict `/sql` by runtime mode
- restrict `/sql` by explicit expert/diagnostic env flag
- keep `/sql` available only for trusted local operator flows

**Acceptance criteria**
- routine public Axon workflows no longer depend on `/sql`
- diagnostic use remains available under explicit operator control

## Phase 4: Upgrade Qualification from Availability to Fluency

### Task 4.1: Extend qualification scenarios for public async UX

**Files**
- Modify: [scripts/mcp_validate.py](/home/dstadel/projects/axon/scripts/mcp_validate.py)
- Modify: [scripts/qualify_mcp.py](/home/dstadel/projects/axon/scripts/qualify_mcp.py)
- Add/update scenario files under `scripts/mcp_scenarios/`

**Intent**
Measure:
- contract truthfulness
- continuation discoverability
- public-only completion of async workflows
- hidden-tool leakage
- public-mode policy visibility

**Required checks**
- async acceptance includes `next_action`
- follow-up tool is public and works
- guessed hidden-tool invocation fails in public mode
- `axon_init_project` can be completed without source reading or `/sql`
- `status` exposes enough metadata for a generic client to obey policy

**Acceptance criteria**
- qualification fails when public async flows are ambiguous
- qualification fails when hidden tools remain callable in public mode

### Task 4.2: Add end-to-end public-flow regression tests

**Files**
- Modify: [src/axon-core/src/mcp/tests.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp/tests.rs)

**Intent**
Encode the LLM-facing promises as server tests.

**Required cases**
- `tools/list` public surface excludes internal tools
- internal guessed-name call is rejected in public mode
- async mutation returns continuation metadata
- public follow-up completes successfully
- `status` reports sufficient capability metadata for client policy and mode negotiation

**Acceptance criteria**
- the server test suite catches future regressions in fluency and boundary enforcement

## Phase 5: Optional Client Adapters, Documentation, and Rollout

### Task 5.1: Create global Axon client adapters

**Files**
- Create or update central skills/adapters for supported clients
- Update Axon-facing docs in repo

**Intent**
Teach supported clients to obey the server-advertised Axon policy automatically. This is an adapter layer, not the primary enforcement mechanism.

**Acceptance criteria**
- supported clients can consume the policy with minimal prompt overhead
- removal of a client adapter would not break server-side enforcement

### Task 5.2: Update operator doctrine

**Files**
- Modify: [docs/skills/axon-engineering-protocol/SKILL.md](/home/dstadel/projects/axon/docs/skills/axon-engineering-protocol/SKILL.md)
- Add/update operator notes under `docs/plans/` or `docs/operations/`

**Intent**
Make the public/expert model and async public pattern explicit for operators and skill authors.

### Task 5.3: Roll out in safe order

**Order**
1. contract inventory
2. wording normalization
3. execution-policy source of truth
4. async public pattern
5. harness/test migration off hidden follow-up
6. capability metadata in `status`
7. dispatch enforcement
8. `/sql` containment
9. qualification upgrades
10. optional client adapters/docs
11. live/dev requalification

## Validation Matrix

- Rust tests for:
  - contract truthfulness
  - public/internal enforcement
  - async continuation
- `./scripts/axon qualify-mcp --surface core --checks quality,guidance`
- `./scripts/axon qualify-mcp --surface soll --checks quality --mutations dry-run`
- explicit adversarial probes:
  - hidden tool guessed by name in public mode
  - async flow without source reading
  - no `/sql` dependency in normal path

## Risks

1. Existing expert workflows may depend on `job_status` or `/sql`.
   - Mitigation: preserve expert path explicitly while creating a clean public path.

2. Tightening dispatch could break internal validation scripts.
   - Mitigation: add explicit expert mode or `include_internal` execution path before enforcement.

3. Qualification may initially fail on many existing flows.
   - Mitigation: sequence work by highest-friction tools first.

4. Env-gated mutation behavior may keep the public contract ambiguous.
   - Mitigation: either eliminate `AXON_MCP_MUTATION_JOBS` as a public-visible bifurcation or advertise/test both modes explicitly.

## Completion Criteria

This initiative is complete when all of the following are true:

1. A general-purpose external LLM can initialize and mutate via Axon using public tools only.
2. Public async flows always disclose the next legal action.
3. Hidden tools are not executable in normal public mode.
4. `/sql` is no longer a normal escape hatch.
5. Qualification explicitly tests fluency and anti-bypass behavior.
