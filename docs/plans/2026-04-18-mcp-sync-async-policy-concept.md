# Axon MCP Product Policy: Two-Flow `sync` / `async`

## Context

Axon MCP currently mixes two patterns:

- short operations that should behave like immediate RPCs
- longer operations that require job orchestration

Client feedback and recent implementation work show that this mixed behavior creates unnecessary friction when short operations are exposed as async jobs.

The product should expose exactly two behavioral flows:

- `sync`
- `async`

No third pattern, no per-tool improvisation, no duplicated public wrappers.

## Product Decision

### 1. `sync` is the default

A tool should be synchronous when it is:

- lightweight by nature
- bounded
- directly useful to the LLM in the current turn
- not dependent on queue / pipeline / mass processing

### 2. `async` is exceptional

A tool should be asynchronous only when it:

- triggers mass processing
- triggers deep analytics with materially variable runtime
- relies on queue-backed or pipeline-backed work
- can reasonably exceed interactive latency expectations

### 3. Single latency gate

The product threshold is conservative:

- target `sync` only when `p95 < 200 ms`

This threshold must be evaluated using:

- real measurements where available
- semantic knowledge of the operation
- worst reasonable interactive case, not the best warmed-cache case

Measurement doctrine:

- evaluate with explicit cache state
  - cold when possible
  - steady-state when relevant
- prefer runtime evidence from the target server behavior, not isolated micro-benchmarks
- consider realistic contention, not only an idle machine
- use representative payload size for the tool

### 4. Intermediate range is not auto-sync

For tools in the range:

- `200 ms <= p95 <= 500 ms`

the default decision is:

- review explicitly

They can still remain synchronous if:

- the operation is transactionally simple
- no queue/pipeline is involved
- cold-start behavior is still acceptable

Decision rule:

- semantic hard-async triggers override latency
- otherwise measured latency decides
- if semantic reasoning and measured latency disagree, the tool becomes a review case rather than an automatic classification

### 5. Hard async triggers

A tool is async even if current measurements are occasionally low when it:

- performs batch application
- restores / imports state
- resumes vectorization or indexing
- fans out into background work

## Proposed Classification

### Async

- `soll_apply_plan`
- `restore_soll`
- `resume_vectorization`

### Sync

All other current public MCP tools, including:

- all read/query/inspection tools
- `axon_init_project`
- `soll_manager`
- `soll_attach_evidence`
- `soll_commit_revision`
- `soll_rollback_revision`
- `soll_export`
- `axon_apply_guidelines`
- `axon_pre_flight_check`
- `axon_commit_work`
- `job_status`

## Async Contract

For async tools, the server must always return:

- `job_id`
- `state`
- `next_action.tool = job_status`
- `next_action.arguments.job_id`
- `result_contract`
- `polling_guidance`
- `recovery_hint`

Normative rule:

- `next_action` is the canonical machine-action field
- `polling_guidance` is explanatory guidance layered on top

## Constraints

- Keep the surface simple.
- No public duplicate wrappers just to compensate for a bad sync/async decision.
- Preserve server-owned identity rules.
- Prefer product coherence over historical implementation convenience.

Ownership:

- the authoritative sync/async classification must live in one central server-side allowlist
- docs and diagnostics must reflect that allowlist, not shadow it

## Non-goals

- no transport redesign
- no client-specific SDK redesign
- no speculative performance rewrite in this phase

## Expected Outcome

After this policy is applied:

- short commands behave like normal immediate tools
- only a small, justified set remains async
- the LLM has one clear follow-up protocol for all async tools
- the MCP surface becomes more predictable and easier to use correctly

Reclassification rule:

- if a synchronous tool repeatedly fails qualification under the agreed measurement doctrine, it must return to review instead of silently staying sync
