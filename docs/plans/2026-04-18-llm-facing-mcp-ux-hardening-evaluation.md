# LLM-Facing MCP UX Hardening Evaluation

Date: 2026-04-18
Status: draft-for-review
Scope: Axon MCP public surface as experienced by external LLM clients (`axon-live`, `axon-dev`)

## Summary

Axon currently exposes enough capability for expert operators, but not yet a sufficiently self-guiding public contract for general LLM clients.

The recent `axon_init_project` deviation was not an isolated agent mistake. It exposed a broader product gap:

- public tool contracts do not always match runtime reality
- async mutation flows are not yet first-class public UX
- public vs internal boundaries are partly cosmetic
- when the public surface becomes ambiguous, clients fall back to raw transport, raw SQL, or code spelunking

This is a product problem first, a client-policy problem second, and an agent-discipline problem third.

## Observed Reality

### 1. Public contract drift exists

`axon_init_project` is described as assigning and returning `project_code`.

Evidence:
- [catalog.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp/catalog.rs:116)
- [SKILL.md](/home/dstadel/projects/axon/docs/skills/axon-engineering-protocol/SKILL.md:28)

But the mutation wrapper used for public async execution returns only:
- `accepted`
- `job_id`
- `status`
- `reserved_ids`

Evidence:
- [mcp.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp.rs:520)

Meanwhile the direct implementation of `axon_init_project` is synchronous and does return:
- `data.project_code`
- `data.project_name`
- `data.project_path`

Evidence:
- [tools_soll.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp/tools_soll.rs:3573)

Conclusion:
- the public promise and the async runtime path diverge
- an LLM cannot reliably infer which contract is authoritative

Important nuance:
- mutation behavior is also environment-gated today via `AXON_MCP_MUTATION_JOBS`
- so some public tools are not purely sync or async by identity alone
- if this bifurcation remains, it must become explicit capability metadata, not hidden runtime variance

### 2. Async follow-up is not first-class public UX

The server accepts async mutation jobs but does not expose a canonical public follow-up flow.

Evidence:
- mutation jobs are queued in [mcp.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp.rs:487)
- `job_status` exists in the catalog, but is hidden from normal public discovery in [catalog.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp/catalog.rs:5)

The async acceptance payload does not tell the LLM:
- which public tool to call next
- what result shape to expect
- whether reserved identity is already known
- which recovery path to use on timeout/failure

Conclusion:
- the server creates async state but does not guide the public client through it

### 3. Public/internal separation is not fully enforced

`job_status` is hidden from public `tools/list`, but still callable if the client guesses the name.

Evidence:
- hidden at listing level in [catalog.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp/catalog.rs:5)
- still directly executable in [mcp.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp.rs:345)

Conclusion:
- current separation is visibility-only, not a true runtime barrier

Related gap:
- the system does not yet expose a concrete server-side policy carrier for `public` vs `expert` mode across `tools/call` and `/sql`
- that authority must be defined before enforcement can be made precise

### 4. Bypass routes remain too attractive

Axon exposes:
- MCP HTTP transport at `/mcp`
- raw SQL gateway at `/sql`

Evidence:
- [mcp_http.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp_http.rs:29)
- [mcp.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp.rs:293)

When the public tool contract becomes ambiguous, LLMs can reach for:
- direct construction of undeclared MCP calls
- guessed hidden-tool invocation
- raw `/sql`
- code reading

Conclusion:
- `/mcp` is the normal transport; the misuse is calling undeclared or hidden capabilities outside the discovered public surface
- expert diagnostics still need escape hatches
- it is not acceptable as the normal recovery path for routine LLM operation

### 5. Client guidance is not global enough

The Axon repo skill is strong, but repo-local. An external Codex session on another project can see Axon MCP servers without inheriting the full Axon behavioral contract.

Conclusion:
- Axon-aware behavior must not depend solely on being inside the Axon repository
- when `axon-live` or `axon-dev` are configured, a global usage policy is needed

## Root Cause

The root cause is a missing product-level doctrine for the full LLM-facing Axon experience:

1. every public tool must have a truthful contract
2. every async public operation must have a truthful public continuation
3. every internal capability must be either truly internal or safely public
4. every public error must guide the client to the next legal action
5. bypass routes must be clearly exceptional, not normal

## Target UX Doctrine

For any external LLM using Axon MCP:

1. `status` is mandatory at:
   - session start
   - reconnect
   - capability/runtime change
   - after an error indicating degraded or unknown mode
2. public tools are sufficient for normal work
3. async public tools always provide:
   - immediate status
   - next public action
   - machine-usable continuation metadata
   - expected result contract
   - failure recovery path
4. internal tools are not reachable in normal public mode
5. `/sql` and undeclared hidden-tool calls are expert diagnostics only
6. the server, not the client, carries the burden of fluency

## External Client Policy

When an external client is connected to Axon:

- it may call only tools returned by public `tools/list`, unless the server has explicitly elevated the session to expert mode
- source code and hidden tool names are non-authoritative for action selection
- if a public operation is ambiguous, the client must ask the server for the next legal action or follow the machine-readable continuation, not infer one from code
- direct `/sql` usage is diagnostic-only
- guessed or hand-constructed hidden-tool calls are diagnostic-only

This policy must become both:
- documented operator doctrine
- machine-checkable server behavior where possible

The primary enforcement layer must be server-side and client-agnostic.
Client-specific skills or adapters are secondary convenience layers, not the source of truth.

## Non-Goals

- do not remove expert diagnostics entirely
- do not eliminate async mutation infrastructure
- do not redesign the whole MCP protocol surface in one wave
- do not depend on client-specific hacks as the primary fix

## Required Workstreams

### A. Public contract audit

Audit every public tool for:
- declared contract
- actual runtime behavior
- async vs sync semantics
- follow-up affordances
- error guidance quality

Priority set:
- `status`
- `project_status`
- `axon_init_project`
- `axon_apply_guidelines`
- `soll_manager`
- `soll_apply_plan`
- `soll_commit_revision`
- `soll_rollback_revision`
- `soll_query_context`
- `soll_validate`
- `soll_verify_requirements`

### B. Async mutation UX

Define a canonical public async pattern:
- uniform acceptance payload
- public follow-up tool
- machine-usable `next_action`
- stable result envelope
- timeout/retry semantics

### C. Public/internal enforcement

Move from:
- hidden in `tools/list`

To:
- actually restricted by dispatch policy and runtime mode

### D. Bypass containment

Keep expert escape hatches, but:
- make them non-default
- document them as diagnostic-only
- prevent routine public flows from needing them
- distinguish normal MCP transport from misuse of undeclared or hidden calls

### E. Global client discipline

When Axon MCP servers are configured in a client:
- enforce `status` at the required session boundaries
- forbid undeclared hidden-tool calls and direct `/sql` in normal flow
- forbid reconstructing tool answers from source code when a public tool exists
- rely first on server-advertised capability metadata, then on client-side skills/adapters

## Reuse vs Change

Reuse:
- existing MCP tool surface
- existing job store (`soll.McpJob`)
- existing guidance work
- existing qualification harnesses

Change:
- public async contract
- dispatch-time public/internal enforcement
- machine-readable capability and continuation metadata
- guidance envelopes
- qualification coverage from “tool works” to “LLM experience is fluent”

## Decision

Proceed.

This must be treated as a cross-surface hardening initiative for Axon’s LLM-facing MCP product, not as a local fix for `axon_init_project`.
