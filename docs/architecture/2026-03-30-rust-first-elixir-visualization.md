# Rust-First Runtime / Elixir Visualization

## Decision

Axon adopts the following target boundary:

- `Rust` is the canonical runtime plane
- `Elixir/Phoenix` is the visualization and operator plane

## Why

The current repository shows that Rust already owns most of the critical mechanics:

- discovery
- `IST` staging
- claiming
- in-memory queueing
- parsing
- structural commit
- chunk derivation
- embeddings
- MCP and SQL surfaces
- recovery and runtime regulation

Keeping ingestion control split across Rust and Elixir creates unnecessary complexity:

- duplicated orchestration
- duplicated queue semantics
- blurred truth ownership
- harder recovery reasoning
- more transition debt

## Consequence

The dashboard must not be treated as a source of runtime truth.

Phoenix/LiveView remains valuable for:

- visualization
- operator console
- telemetry display
- observability
- rich UI workflows

It does not remain responsible for:

- ingestion scheduling
- file staging as canonical backlog
- parser dispatch
- backpressure authority over canonical ingestion
- recovery semantics

## Target Ownership

### Rust-owned

- scan and `Axon Ignore`
- capability filtering
- `IST` lifecycle
- delta restart logic
- claiming and scheduling
- worker orchestration
- parsing
- indexing
- chunk and graph enrichment
- embeddings
- MCP and SQL
- canonical backpressure and service protection

### Elixir-owned

- cockpit and status screens
- operator-facing telemetry
- visual projections and read models
- UI subscriptions and rendering
- optional presentation caches

## Migration Strategy

### Phase 1. Freeze new Elixir runtime responsibilities

No new ingestion or canonical runtime authority should be added to Elixir.

### Phase 2. Make Rust the only source of runtime truth

Any critical ingestion, scheduling, recovery, and backpressure logic must converge into Rust.

### Phase 3. Turn Elixir into a pure consumer

Phoenix reads Rust-owned truth through SQL/MCP/API/SSE and presents it without owning the pipeline.

### Phase 4. Remove redundant Elixir control-paths

Only after the UI is preserved and validated.

## First Low-Pain Migration Slice

The least painful first slice is:

- keep the current dashboard
- stop treating Elixir watcher / staging / Oban / pool facade as canonical ingestion logic
- continue moving scheduling and regulation into Rust
- make the dashboard consume Rust state instead of coordinating the pipeline

## Residual Elixir Runtime Authority To Retire

The following Elixir modules still retain real ingestion or control-plane authority today.
They are no longer target ownership. They are transitional and must be retired in order.

### Tier 1. Canonical ingestion authority still active

- `Axon.Watcher.Server`
  - owns filesystem event batching
  - stages paths into Elixir-side backlog
  - can still trigger `SCAN_ALL`
  - still performs purge and retry actions
- `Axon.Watcher.Staging`
  - owns an ETS-backed discovery buffer
  - writes pending rows and enqueues jobs transactionally
- `Axon.Watcher.PathPolicy`
  - still decides eligibility, project mapping, and hot priority on the Elixir watcher path
- `Axon.Watcher.IndexingWorker`
  - owns Oban worker execution for parse batches
- `Axon.Watcher.BatchDispatch`
  - enqueues canonical indexing jobs into Oban
- `Axon.Watcher.PoolFacade`
  - forwards control commands to Rust
  - remains an ingestion bridge for `PARSE_BATCH`, `SCAN_ALL`, and `PULL_PENDING`
- `Axon.Watcher.PoolEventHandler`
  - reacts to Rust-side batch readiness and still re-enqueues work through Oban

These modules are marked `to retire` as ingestion authorities.

### Tier 2. Canonical pressure/control authority still active

- `Axon.BackpressureController`
  - pauses/resumes/scales Oban queues from Elixir
  - still acts as a circuit breaker over active ingestion queues
- `Axon.Watcher.TrafficGuardian`
  - no longer pulls from Rust actively
  - is now mostly telemetry-oriented
  - still remains transitional because it publishes pressure semantics around a path that is being de-authorized

These modules are marked `to retire` as canonical control authority.
Their telemetry can survive, but their decisions must not remain authoritative.

### Tier 3. Elixir modules that may remain after de-authoring

- `Axon.Watcher.CockpitLive`
- `Axon.Watcher.Progress`
- `Axon.Watcher.Telemetry`
- `Axon.Watcher.StatsCache`
- `Axon.Watcher.Auditor`
- `Axon.Watcher.SqlGateway`

These are expected to remain as visualization, projection, telemetry, or operator-facing read models.
They are not marked `to retire` unless they reintroduce ingestion ownership.

## Retirement Order

The safe removal order is:

1. freeze authority inventory and stop adding any new Elixir ingestion behavior
2. de-authorize `Axon.BackpressureController`, while keeping `Axon.Watcher.TrafficGuardian` as display-only telemetry if still useful
3. de-authorize `Axon.Watcher.Server` and `Axon.Watcher.PathPolicy` as staging/dispatch owners
4. retire `Axon.Watcher.BatchDispatch` and `Axon.Watcher.PoolEventHandler`
5. retire `Axon.Watcher.IndexingWorker` and `Axon.Watcher.Staging`
6. reduce `Axon.Watcher.PoolFacade` to read/telemetry-only bridge logic
7. keep Phoenix/LiveView consuming Rust-owned truth through SQL/MCP/API/SSE only

## Explicit Rule During Migration

Until the migration is complete:

- Rust remains the canonical runtime
- Elixir ingestion/control modules are `to retire`
- no new canonical backlog, scheduler, worker, or retry logic may be added to Elixir
- any temporary Elixir action must be treated as compatibility scaffolding, not target architecture

## Wave 1 Constraint Discovered

`Axon.BackpressureController` is now being reduced to telemetry/display semantics before the full Elixir ingestion chain is removed.
That is acceptable only if the next slice follows immediately:

- `Axon.Watcher.Server`
- `Axon.Watcher.Staging`
- `Axon.Watcher.BatchDispatch`
- `Axon.Watcher.IndexingWorker`
- `Axon.Watcher.PoolEventHandler`

must lose their remaining authority quickly, otherwise Elixir can still feed Oban while no longer applying queue-side braking.

The intended steady state remains:

- Rust throttles canonical ingestion
- Elixir displays pressure and operator state
- explicit operator intent may be relayed to Rust
- Elixir must not fabricate local `indexing` truth ahead of Rust/DB evidence

## First Delivered Slice

The first concrete migration slice now exists on the Rust side:

- Rust can stage a hot file delta directly into `IST`
- hierarchical `Axon Ignore` is enforced during that delta staging
- hot deltas are promoted with explicit priority instead of waiting for a full rescan
- a native Rust filesystem watcher is now wired to that hot-delta staging path
- duplicate bursts and missing ephemeral paths are absorbed without failing the runtime

This slice does not yet replace the OS watcher fully.
It does establish the new ownership boundary:

- file delta eligibility and staging belong to Rust
- Elixir no longer needs to remain the canonical authority for this path

The next safe step is to wire a Rust-native filesystem watcher to this staging path, then retire the equivalent Elixir control flow.

## Non-Goals

This decision does not require:

- removing Phoenix immediately
- rewriting the UI immediately
- deleting every Elixir module immediately

It only clarifies the canonical runtime ownership now, so the remaining migration can proceed without ambiguity.
