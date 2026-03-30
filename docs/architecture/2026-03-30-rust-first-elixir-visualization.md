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
