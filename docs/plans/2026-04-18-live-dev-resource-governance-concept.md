# Concept: Live/Dev Resource Governance

## Context

Axon now runs as two parallel instances on the same machine:

- `live`
  - stable truth runtime
  - serves daily MCP traffic
  - should stay responsive under operator and LLM load
- `dev`
  - isolated development runtime
  - used for coding, migrations, qualification, replay, and experiments

The identity split is now explicit.
The next problem is resource contention.

Two healthy runtimes on the same machine can still interfere through shared:

- CPU
- RAM
- GPU
- disk I/O
- filesystem watcher pressure

If both run at full intensity, `live` can lose responsiveness even though ports, databases, and sockets are already isolated.

## Goal

Define a governance model where:

- `live` keeps MCP responsiveness and operational stability
- `dev` remains useful for development
- contention is managed intentionally, not left to chance
- the system reuses Axon's existing runtime tuning and pressure controls instead of creating a second orchestration stack

## Non-Goals

This concept does not aim to:

- create a distributed scheduler
- add Kubernetes-style resource orchestration
- hard-require cgroups/systemd slices in phase 1
- duplicate runtime tuning logic per instance
- make `live` and `dev` equally privileged

The design is intentionally asymmetric.

## Core Principle

`live` is first-class.
`dev` degrades first.

This must hold under:

- heavy indexing
- semantic/vector backlog drain
- GPU memory pressure
- large file churn
- concurrent MCP traffic

## Existing Mechanisms To Reuse

Axon already contains a substantial resource-control substrate:

- runtime auto-sizing in [runtime_profile.rs](/home/dstadel/projects/axon/src/axon-core/src/runtime_profile.rs)
  - CPU/RAM/GPU detection
  - worker recommendations
  - queue capacity sizing
  - embedding lane sizing
- runtime modes in [runtime_mode.rs](/home/dstadel/projects/axon/src/axon-core/src/runtime_mode.rs)
  - `full`
  - `graph_only`
  - `read_only`
  - `mcp_only`
- memory-budgeted queue admission in [queue.rs](/home/dstadel/projects/axon/src/axon-core/src/queue.rs)
- interactive pressure and degraded-state tracking in [service_guard.rs](/home/dstadel/projects/axon/src/axon-core/src/service_guard.rs)
- vector/backlog throttling in [vector_control.rs](/home/dstadel/projects/axon/src/axon-core/src/vector_control.rs)
- runtime telemetry and host snapshots in:
  - [main_telemetry.rs](/home/dstadel/projects/axon/src/axon-core/src/main_telemetry.rs)
  - [tools_system.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp/tools_system.rs)

The right direction is not a new scheduler.
It is an instance-aware policy layer on top of these mechanisms.

## Target Model

### 1. Instance Roles

Each instance must declare a resource role:

- `live`
  - latency-sensitive
  - truth-serving
  - conservative about background work
- `dev`
  - throughput-tolerant
  - degradable
  - allowed to pause or downshift under contention

This role should be derived from `AXON_INSTANCE_KIND`, not inferred from ports.

### 2. Instance Resource Policy

Each instance should expose and obey a compact policy:

- `resource_priority`
  - `critical` for `live`
  - `best_effort` for `dev`
- `background_budget_class`
  - `conservative`, `balanced`, `aggressive`
- `gpu_access_policy`
  - `preferred`, `shared`, `avoid`
- `watcher_policy`
  - `full`, `bounded`, `off`

Phase 1 should keep this policy mostly in scripts/env, not in a large new runtime schema.

### 3. Default Runtime Shape

The default stance should be:

- `live`
  - default runtime mode: `full`
  - lower semantic concurrency than raw hardware maximum
  - watcher enabled
  - vector drain allowed, but guarded aggressively by interactive pressure
- `dev`
  - default runtime mode: `graph_only` or `full` depending on explicit operator choice
  - lower worker caps than `live`
  - vectorization can be paused or clamped first
  - watcher can be narrowed or disabled when seeded from `live`

This means both instances stay useful, but `dev` pays the first cost when the host is stressed.

### 4. Contention Domains

Resource governance must reason about four domains separately.

#### CPU

Risk:

- MCP latency regression when both runtimes saturate workers
- watcher and indexing compete with query serving

Control:

- instance-specific worker caps
- instance-specific runtime mode defaults
- `live` keeps a guaranteed headroom margin
- `dev` uses lower default caps and yields first under pressure

#### RAM

Risk:

- queue over-admission
- DuckDB memory growth
- ONNX model residency plus indexing buffers

Control:

- preserve queue memory budgets
- add instance-specific budget scaling
- `dev` gets a smaller queue memory allowance by default
- `dev` should enter degraded mode before `live`

#### GPU

Risk:

- two runtimes competing for model residency and VRAM
- one runtime destabilizing the other through repeated CUDA pressure

Control:

- only one instance should be GPU-preferred by default
- phase 1 default should be:
  - `live`: `preferred`
  - `dev`: `shared` or `avoid`
- `dev` should fall back to CPU-first semantic work when GPU pressure is active on `live`

#### Disk I/O and Watchers

Risk:

- duplicate scans of the same trees
- competing WAL/checkpoint pressure
- broad watcher activity during development rebuilds

Control:

- distinct DB roots remain mandatory
- watcher scope should become instance-aware
- `dev` watcher defaults should be narrower than `live`
- seeding from `live` should reduce the need for broad initial `dev` scans

## Governance Strategy

### Policy A: Live-First Degradation

When host or service pressure rises:

1. clamp `dev` semantic throughput first
2. pause `dev` background vectorization first
3. reduce `dev` watcher aggressiveness first
4. only then degrade `live` background work if pressure remains critical

This is the central invariant.

### Policy B: No Silent Equality

If both instances are configured identically, the system should consider that an operator smell.

`live` and `dev` are not peers.
Defaults should encode that explicitly.

### Policy C: Reuse Runtime Modes As First Lever

The first control plane should be existing runtime modes and worker caps, not a new complex runtime scheduler.

The recommended progression is:

1. choose a mode per instance
2. choose worker/budget caps per instance
3. expose observability
4. only then add automatic contention reactions

### Policy D: Observability Before Automation

Before full auto-throttling, the operator must be able to see:

- which instance is consuming pressure
- whether pressure is CPU, RAM, GPU, or I/O dominated
- whether `dev` was throttled or paused
- whether `live` still meets MCP quality targets

### Policy E: Temporary Role Rebalancing Must Stay Explicit and Reversible

There are legitimate periods where one instance should temporarily yield more aggressively:

- performance work on `dev`
- migration rehearsal on `dev`
- heavy qualification on `dev`
- rare live maintenance or rebuild activity

The first mechanism should be explicit runtime-mode switching, not ad hoc process killing.

Examples:

- temporarily run `live` as `mcp_only` or `read_only` to preserve truth-serving while freeing indexing and semantic capacity for `dev`
- temporarily run `dev` in a reduced mode while `live` performs an exceptional rebuild

This must be:

- explicit
- operator-visible
- easy to reverse
- safe for data integrity

The system should prefer reversible mode transitions over destructive restarts or manual background-process surgery.

## Phase-1 Design Direction

Phase 1 should be deliberately modest.

### Phase 1A: Static Default Separation

Add explicit defaults:

- `live`
  - higher priority
  - balanced worker caps
  - GPU preferred
  - watcher full
- `dev`
  - lower worker caps
  - smaller queue memory budget
  - GPU shared/avoid
  - watcher bounded

### Phase 1B: Shared Runtime Identity + Policy Reporting

Expose in `status` or operator diagnostics:

- `instance_kind`
- `resource_priority`
- `runtime_mode`
- worker caps
- queue memory budget
- GPU access policy
- watcher policy

### Phase 1C: Measured Qualification Under Parallel Load

Introduce a qualification scenario where:

- `live` serves MCP checks
- `dev` runs background load
- the system verifies that `live` remains within acceptable latency and degradation thresholds

This should become the proof that dual-instance operation is operationally safe.

### Phase 1D: Controlled Temporary Mode Rebalancing

Phase 1 should also support an operator workflow for temporary rebalancing:

- downgrade `live` to `mcp_only` or `read_only`
- run an intensive `dev` campaign
- restore `live` to `full`

And, when needed, the inverse:

- temporarily reduce `dev`
- allow `live` to consume more host capacity

This should be modeled as an operator-grade workflow with preconditions, status visibility, and explicit restoration.

## What This Does Not Yet Solve

Even after phase 1, the system may still need later work for:

- OS-level CPU scheduling or cgroup isolation
- explicit GPU lock coordination
- dynamic watcher backoff under host pressure
- automatic live-to-dev re-seeding policy
- policy tuning by hardware tier

Those are phase-2 or later concerns.

## Recommended Next Step

Turn this concept into a plan with these workstreams:

1. instance resource policy contract
2. instance-aware defaults in scripts/startup
3. runtime/status exposure of resource policy
4. qualification under parallel load
5. optional later automation for adaptive throttling

## Judgment

This is a real problem, not a speculative one.

The dual-instance split solved identity and safety of data roots.
It did not solve shared-host contention.

The good news is that Axon already contains most of the low-level machinery needed.
The missing layer is a clear, asymmetric policy:

- `live` is protected
- `dev` remains useful
- `dev` yields first
