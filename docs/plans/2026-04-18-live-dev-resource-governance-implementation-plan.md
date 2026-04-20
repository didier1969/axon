# Live/Dev Resource Governance Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Protect `live` responsiveness and operational stability when `live` and `dev` run in parallel on the same machine, while keeping `dev` effective for development and qualification.

**Architecture:** Reuse Axon's existing runtime modes, worker auto-sizing, memory-budgeted admission, service pressure tracking, and vector throttling. Add an instance-aware resource policy layer in scripts and status first, then add qualification scenarios and optional adaptive reactions.

**Tech Stack:** Bash scripts, Python qualification scripts, Rust runtime (`axon-core`), tmux, DuckDB-backed IST/SOLL, existing MCP/status tooling.

---

## Design Rules

1. `live` has priority over `dev`.
2. `dev` degrades first under contention.
3. Use existing runtime modes and worker/budget caps as the first lever.
4. Do not invent a second scheduler in phase 1.
5. Any temporary rebalancing between instances must be explicit and reversible.
6. Observability must precede automation.
7. Qualification must prove that `live` remains usable while `dev` is active.
8. Phase 1 is Axon-level governance on a shared machine, not OS-level isolation or full scheduler control.
9. Policy-decision logic must live in shell/operator policy resolution, not be duplicated inside Rust runtime logic.

## Phase 1: Define the Resource Policy Contract

### Task 1: Add an explicit instance resource policy model

**Files:**
- Create: [scripts/lib/axon-resource-policy.sh](/home/dstadel/projects/axon/scripts/lib/axon-resource-policy.sh)
- Modify: [scripts/lib/axon-instance.sh](/home/dstadel/projects/axon/scripts/lib/axon-instance.sh)
- Modify: [scripts/start.sh](/home/dstadel/projects/axon/scripts/start.sh)
- Modify: [scripts/status.sh](/home/dstadel/projects/axon/scripts/status.sh)
- Modify: [docs/operations/2026-04-18-live-dev-runtime-operations.md](/home/dstadel/projects/axon/docs/operations/2026-04-18-live-dev-runtime-operations.md)

**Intent:**
Define one shared source of truth for instance resource policy.

**Contract to introduce:**
- `AXON_RESOURCE_PRIORITY=critical|best_effort`
- `AXON_BACKGROUND_BUDGET_CLASS=conservative|balanced|aggressive`
- `AXON_GPU_ACCESS_POLICY=preferred|shared|avoid`
- `AXON_WATCHER_POLICY=full|bounded|off`

**Acceptance criteria:**
- both `live` and `dev` resolve their resource policy through a shared helper
- policy defaults are asymmetric by instance
- operator scripts can print the effective policy
- no second policy-decision layer is introduced in Rust

### Task 2: Encode conservative default resource profiles

**Files:**
- Modify: [scripts/start.sh](/home/dstadel/projects/axon/scripts/start.sh)
- Modify: [scripts/axon-live](/home/dstadel/projects/axon/scripts/axon-live)
- Modify: [scripts/axon-dev](/home/dstadel/projects/axon/scripts/axon-dev)
- Modify: [scripts/start-live.sh](/home/dstadel/projects/axon/scripts/start-live.sh)
- Modify: [scripts/start-dev.sh](/home/dstadel/projects/axon/scripts/start-dev.sh)

**Intent:**
Make `live` and `dev` start with different resource defaults, even on the same hardware.

**Defaults to encode in phase 1:**
- `live`
  - priority `critical`
  - budget class `balanced`
  - GPU policy `preferred`
  - watcher policy `full`
- `dev`
  - priority `best_effort`
  - budget class `conservative`
  - GPU policy `shared` or `avoid`
  - watcher policy `bounded`

**Note:**
If `dev` defaults to `gpu_access_policy=avoid`, phase 1 may already project that into `AXON_EMBEDDING_PROVIDER=cpu` at startup, as long as the behavior is explicit, documented, and status-visible.

**Acceptance criteria:**
- `live` and `dev` no longer inherit identical worker/budget defaults
- defaults can still be overridden explicitly by the operator
- the defaults are documented

## Phase 2: Bind Policy to Runtime Knobs

### Task 3: Map policy to worker caps and queue budgets

**Files:**
- Modify: [scripts/start.sh](/home/dstadel/projects/axon/scripts/start.sh)
- Modify: [src/axon-core/src/main.rs](/home/dstadel/projects/axon/src/axon-core/src/main.rs)
- Modify: [src/axon-core/src/runtime_profile.rs](/home/dstadel/projects/axon/src/axon-core/src/runtime_profile.rs)
- Test: relevant Rust tests in [src/axon-core/src/main.rs](/home/dstadel/projects/axon/src/axon-core/src/main.rs)

**Intent:**
Translate instance policy into actual runtime behavior without replacing the existing auto-sizing model.

**Phase-1 rule:**
- `scripts/lib/axon-resource-policy.sh` is the only place that resolves policy classes into concrete settings
- `scripts/start.sh` projects those settings into existing environment knobs
- Rust runtime changes are limited to consuming env/config and exposing observability, not re-interpreting the policy contract in parallel

**Phase-1 mapping shape:**
- clamp `MAX_AXON_WORKERS` differently per instance
- scale `AXON_QUEUE_MEMORY_BUDGET_BYTES` differently per instance
- optionally clamp `AXON_VECTOR_WORKERS` / `AXON_GRAPH_WORKERS` on `dev`

**Acceptance criteria:**
- policy changes produce predictable worker/budget differences
- `live` keeps more guaranteed headroom than `dev`
- current auto-sizing logic remains the base layer
- Rust does not become a second source of truth for policy-class resolution

### Task 4: Bind policy to watcher scope and behavior

**Files:**
- Modify: [scripts/start.sh](/home/dstadel/projects/axon/scripts/start.sh)
- Modify: relevant watcher/runtime bootstrap code if needed
- Update: [docs/operations/2026-04-18-live-dev-runtime-operations.md](/home/dstadel/projects/axon/docs/operations/2026-04-18-live-dev-runtime-operations.md)

**Intent:**
Reduce unnecessary file-system pressure from `dev`.

**Phase-1 behavior:**
- `live` watcher remains full by default
- `dev` watcher can be bounded or disabled by policy
- seeded `dev` should not require a broad watcher by default if the operator chooses otherwise

**Phase-1 boundary:**
- use only existing watcher/runtime control surfaces already available through startup env and runtime mode wiring
- do not introduce a broad watcher architecture refactor in this tranche
- if no existing lever cleanly matches the desired bounded behavior, stop at explicit runtime-mode wiring plus observability instead of inventing a second watcher framework

**Acceptance criteria:**
- watcher policy is visible and effective
- `dev` can run with reduced watcher pressure without breaking MCP serving
- the implementation remains a bounded control-plane change, not a watcher redesign

### Task 5: Bind policy to GPU stance

**Files:**
- Modify: [scripts/start.sh](/home/dstadel/projects/axon/scripts/start.sh)
- Modify: [src/axon-core/src/main.rs](/home/dstadel/projects/axon/src/axon-core/src/main.rs)
- Modify: any embedder/provider startup points as needed

**Intent:**
Avoid silent GPU equality between `live` and `dev`.

**Phase-1 behavior:**
- `live` may remain GPU-preferred
- `dev` can be CPU-first or GPU-shared depending on explicit policy

**Acceptance criteria:**
- `dev` can be started in a mode that avoids destabilizing `live` GPU workloads
- `status` and operator diagnostics show the chosen GPU stance

## Phase 3: Expose Resource Policy and Runtime State

### Task 6: Expose the effective resource policy in protocol-visible status

**Files:**
- Modify: [src/axon-core/src/mcp/tools_framework.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp/tools_framework.rs)
- Modify: [src/axon-core/src/mcp/tests.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp/tests.rs)
- Modify: [scripts/status.sh](/home/dstadel/projects/axon/scripts/status.sh)

**Intent:**
Make resource posture visible to operators and LLMs.

**Fields to expose:**
- `resource_priority`
- `background_budget_class`
- `gpu_access_policy`
- `embedding_provider`
- `watcher_policy`
- effective worker caps / queue budget
- runtime mode

**Acceptance criteria:**
- `status` makes the resource posture explicit
- operator shell status reflects the same contract at a local level

### Task 7: Surface resource contention signals cleanly

**Files:**
- Modify: [src/axon-core/src/mcp/tools_system.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp/tools_system.rs)
- Modify: qualification/reporting scripts if needed

**Intent:**
Give operators one place to see if contention is CPU, RAM, GPU, or I/O dominated.

**Signals to summarize explicitly in phase 1:**
- queue memory saturation / exhaustion ratio
- scanner or backlog growth
- watcher event pressure where available
- DuckDB/checkpoint/WAL pressure where available
- service pressure and degraded-state indicators

**Acceptance criteria:**
- existing telemetry is summarized at an operator-useful level
- it is possible to see whether `dev` was throttled, paused, or degraded first

## Phase 4: Qualification Under Parallel Load

### Task 8: Add a dual-instance qualification scenario

**Files:**
- Modify: [scripts/qualify_mcp.py](/home/dstadel/projects/axon/scripts/qualify_mcp.py)
- Create: scenario files under `scripts/mcp_scenarios/` if needed
- Update: [docs/operations/2026-04-18-live-dev-runtime-operations.md](/home/dstadel/projects/axon/docs/operations/2026-04-18-live-dev-runtime-operations.md)

**Intent:**
Make dual-instance safety measurable.

**Scenario shape:**
- `live` serves core MCP qualification traffic
- `dev` runs controlled background pressure
- evaluate whether `live` remains within acceptable latency and verdict thresholds

**Acceptance criteria:**
- a repeatable qualification scenario exists for parallel `live/dev`
- its summary distinguishes `live outcome` and `dev pressure profile`

### Task 9: Define acceptance thresholds for live-first safety

**Files:**
- Update: qualification scripts and docs

**Intent:**
Turn “live-first” from doctrine into a measurable gate.

**Examples of thresholds to define:**
- `live qualify-mcp` remains `ok`
- `live` latency stays within agreed bounds under a defined `dev` profile
- max tolerated `live` degraded or pressure indicators
- acceptable `dev` degradation envelope
- `dev` may degrade first without invalidating the run

**Acceptance criteria:**
- the gate is explicit enough to support future promotion confidence

## Phase 5: Controlled Temporary Rebalancing

### Task 10: Add explicit temporary mode-rebalancing commands as break-glass operator actions

**Gate:**
Do not implement this task until Tasks 8-9 are complete and acceptance thresholds are fixed.

**Files:**
- Modify: [scripts/axon](/home/dstadel/projects/axon/scripts/axon)
- Create or modify wrappers under `scripts/` for resource mode switching
- Update: [docs/operations/2026-04-18-live-dev-runtime-operations.md](/home/dstadel/projects/axon/docs/operations/2026-04-18-live-dev-runtime-operations.md)

**Intent:**
Support explicit, reversible emergency rebalancing such as:
- `live` -> `mcp_only`
- `live` -> `read_only`
- restore `live` -> `full`
- and the symmetric reductions for `dev`

**Phase-1 posture:**
- this is break-glass operator tooling
- it is not the normal contention-handling path
- the normal path remains: `dev` degrades first, `live` stays at its normal service level

**Requirements:**
- explicit command path
- preconditions printed before action
- clear restoration path
- no hidden mutation of database roots or version identity

**Acceptance criteria:**
- an operator can temporarily free host capacity without ad hoc process surgery
- restoration to the prior mode is explicit and documented

### Task 11: Record and expose rebalancing state

**Files:**
- Modify: [scripts/status.sh](/home/dstadel/projects/axon/scripts/status.sh)
- Modify: MCP `status` contract if needed

**Intent:**
Avoid confusion about whether an instance is temporarily downshifted.

**Acceptance criteria:**
- status output shows whether the instance is in a temporary reduced mode
- the operator can prove that `live` is intentionally in `mcp_only` or `read_only`, not accidentally degraded

## Phase 6: Hardening and Documentation

### Task 12: Document operator playbooks

**Files:**
- Update: [docs/operations/2026-04-18-live-dev-runtime-operations.md](/home/dstadel/projects/axon/docs/operations/2026-04-18-live-dev-runtime-operations.md)
- Possibly add a new operator note if needed

**Playbooks to include:**
- normal `live` + `dev` startup
- `dev` perf campaign with temporary `live` downgrade
- restoring `live` to full mode
- interpreting resource-policy status
- safe fallbacks when the host is overcommitted

**Acceptance criteria:**
- the operator can run the system without guessing policy semantics

### Task 13: Final verification and integration readiness

**Files:**
- verification only

**Run at minimum:**
- targeted Rust tests for policy/status behavior
- shell syntax checks for modified scripts
- `qualify-mcp` on `live`
- `qualify-mcp` on `dev`
- dual-instance qualification scenario

**Acceptance criteria:**
- behavior is verified, not asserted
- residual risks are stated explicitly
- the branch is ready for integration review
