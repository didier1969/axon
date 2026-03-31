# Adaptive Ingestion Concept

## Purpose

This document defines the target ingestion architecture for Axon.

It is written under the architecture rule that:

- Rust is the canonical runtime plane
- Elixir/Phoenix is the visualization and operator plane

The objective is simple:

- ingest as fast as possible
- slow down automatically when the machine, DuckDB, or the live query surfaces require it
- preserve `SOLL`
- keep `IST` truthful and reconstructible
- keep MCP and SQL responsive during heavy indexing

This is not a failure report. It is the design target for the next iterations.

## Core Principles

### 1. `IST` is a managed staging and knowledge graph

`IST` is not just a storage sink.
It is the durable operational buffer for discovery, scheduling, parsing, indexing, and derived enrichment.

It must support:

- discovery staging
- eligibility decisions
- prioritization
- claiming
- indexing state transitions
- resumability after restart
- derived layers such as chunks and embeddings

### 2. `SOLL` remains physically and logically protected

`SOLL` is a protected conceptual asset.

Any reset, purge, rebuild, or compatibility repair applies to `IST`, never to `SOLL` unless explicitly requested.

`IST` must also support additive boot-time repair for legacy schemas.

If a new column is required by the runtime, Axon should first attempt a safe additive migration before falling back to a broader `IST` reset.
This preserves delta continuity across restarts and avoids replaying the whole universe when a narrow schema drift can be repaired in place.

### 3. DuckDB is a single-write authority

Axon must embrace DuckDB's write model instead of fighting it.

The target model is:

- one authoritative write path
- live query surfaces protected from write starvation
- adaptive throttling when write pressure degrades query service

Axon must not assume that full-speed ingestion and always-responsive live queries happen automatically under heavy load.
That balance must be orchestrated.

### 4. Structure first, semantics second

The ingestion order is:

1. discovery
2. structural indexing
3. chunk derivation
4. chunk embeddings
5. graph projections
6. graph embeddings

If the system is under pressure, derived semantic work is reduced before structural truth is compromised.

## Target Chain

## Stage 1. Discovery

The scanner traverses the project universe and performs only cheap decisions:

- path filtering through `Axon Ignore`
- capability filtering
- basic metadata extraction
- lightweight fingerprinting

The scanner does not parse ASTs and does not perform semantic work.

Its role is to feed `IST`.

## Stage 2. Eligibility and Staging

Each discovered file is written into `IST` with enough metadata to support scheduling:

- `path`
- `project_slug`
- `size`
- `mtime`
- `fingerprint`
- `status`
- `priority`
- `parser_kind`
- `eligibility_reason`

This stage creates a durable backlog.

The database becomes the canonical source of pending work.
RAM queues remain acceleration layers only.

The first Rust-native delta path is now aligned with this model:

- changed files can be re-staged directly into `IST`
- `Axon Ignore` is checked again on the delta path
- hot deltas receive elevated priority so they do not wait behind bulk universe work
- a native debounced filesystem watcher now feeds this path directly
- duplicate bursts and short-lived missing paths are tolerated without poisoning the backlog
- the active project root can be armed before the cold universe, so the hot set becomes reactive without waiting for every recursive watch target to finish registering
- the active project can also be pre-indexed before the full universe scan, so `IST` becomes useful for the current repo before cold-universe discovery completes
- unreadable project roots are skipped instead of poisoning the whole watch setup

## Stage 3. Adaptive Claiming

A scheduler claims pending work from `IST` according to system pressure and service health.

It must adapt to:

- DB backlog depth
- RAM queue depth
- worker occupancy
- parse latency
- commit latency
- embedding backlog
- memory pressure
- MCP and SQL latency

Claim size and claim frequency must be dynamic, not fixed.

The canonical throttling state now belongs to Rust.
Elixir may still publish observational system pressure for the UI, but it must not be treated as the source of truth for ingestion control.

The runtime claim policy should therefore remain explicit and observable in Rust with stable modes such as:

- `fast`
- `slow`
- `guarded`
- `paused`

These modes are not cosmetic.
They are the runtime truth the operator plane should eventually consume instead of inventing a second control policy.

## Hot Project First

The current repo must become queryable early.

The target boot order is:

1. arm the universe root watcher
2. arm the active project hot set
3. pre-index the active project subtree into `IST`
4. continue the cold-universe scan

This does not change the global universe objective.
It changes time-to-usefulness for the active project.

The watcher must also tolerate two startup storms:

1. the early event burst while the hot set is still arming
2. the delayed event burst right after the cold universe finishes arming

Both are expected side effects of large watch registration on a busy workspace.
They must be suppressed instead of triggering a destructive safety rescan during startup.

The suppression policy must therefore cover both windows explicitly:

- the early hot-set registration window
- the short post-cold-arm stabilization window

Outside these windows, genuine watcher `need_rescan()` signals must still be honored.

Bootstrap suppression must not discard the active project's real hot deltas.
During a storm, Axon now salvages only file paths that belong to the active project, instead of recursively restaging whole directories from that batch.
This keeps the watcher callback responsive and preserves the operator-visible delta path.

## Stage 4. Worker Lanes

Work is dispatched into explicit lanes:

- `hot`: recent changes and user-driven work
- `bulk`: background universe scan
- `titan`: unusually large or expensive files

The system always protects a minimum service capacity for `hot` work.

## Stage 5. Single Structural Commit Path

Structural writes remain serialized through one authoritative commit path.

This is a deliberate design choice for truth and consistency.

Instead of parallelizing writes blindly, Axon must optimize:

- batching
- scheduling
- priority
- commit cadence

Two invariants now apply to this commit path:

- a file already in `indexing` must not be reopened immediately by a hot delta
- a real metadata change observed during `indexing` must be replayed as a second pass after the current commit finishes

This avoids duplicate work and duplicate symbol insertion while still preserving delta truth.

The same path must remain compatible with previously created `IST` files.
When a newly introduced field such as `needs_reindex` is absent from a legacy `File` table, Axon repairs that gap additively at boot before any claim or reopen logic runs.
The same restart path must also salvage interrupted claims:

- files left in `status='indexing'` after a crash are moved back to `pending` at boot
- `worker_id` is cleared
- `needs_reindex` is preserved, so a true mid-index change still forces the second pass after replay

Delete and rename events must also preserve `IST` truth without a full rescan:

- a missing watcher path now tombstones the matching `File` row, or the matching subtree if the missing path is a directory prefix
- derived truth attached to that path (`CONTAINS`, `Symbol`, `Chunk`, `CALLS`, `CALLS_NIF`, `ChunkEmbedding`) is purged immediately
- a late worker commit must not resurrect a tombstoned path; tombstone state wins over stale extraction results
- a rename is therefore represented as `old path -> tombstoned`, `new path -> staged hot delta`

The watcher path now also carries explicit primary checkpoints:

- `watcher.storm_suppressed`
- `watcher.storm_salvaged`
- `watcher.rescan_requested`
- `watcher.rescan_started`
- `watcher.rescan_completed`
- `watcher.rescan_skipped`
- `watcher.received`
- `watcher.filtered`
- `watcher.db_upsert`
- `watcher.staged`
- `watcher.tombstoned`
- `watcher.staged_none`
- `watcher.staging_failed`
- `watcher.error`

Additional lower-level probes such as `watcher.staged_batch`, `watcher.storm_salvaged_none`, and `watcher.missing` also exist in the runtime, but the list above is the primary operator-facing checkpoint chain used to prove where a hot delta stops instead of inferring it indirectly from missing rows.

## Stage 6. Derived Semantic Work

Derived semantic work is executed only when structural ingestion is healthy.

Derived layers include:

- `Chunk`
- `ChunkEmbedding`
- future `GraphProjection`
- future `GraphEmbedding`

These layers are disposable and versioned.
They must never be treated as primary truth.

## Symbol Identity

`Symbol.id` must stay stable enough for graph work, but unique enough to survive a large universe scan.

The current rule is:

- globally qualified names keep a project-qualified ID
- non-qualified top-level names become path-aware inside their project

This prevents collisions such as repeated helper names across multiple scripts in the same project without changing the human-facing `name` field.

## Axon Ignore

## Intent

Axon needs a sovereign ignore system.

It must not depend strictly on `.gitignore`, because some files excluded from Git can still be valuable for immediate indexing and project operations.

Examples include:

- progress files
- planning files
- generated but operationally relevant documents

## Target Behaviour

Axon will support a hierarchical ignore model similar to `gitignore`, but under Axon's own policy.

The recommended precedence is:

1. runtime overrides
2. `.axonignore.local`
3. `.axonignore`
4. optional `.ignore`
5. optional `.gitignore`
6. hard technical Axon protections

This model must support:

- hierarchical inheritance
- local overrides in subdirectories
- explicit re-inclusion with `!pattern`

## Implementation Direction

Axon should reuse the Rust `ignore` crate for matching semantics and hierarchy.

Axon should not reinvent pattern matching.

What Axon owns is the policy layer:

- which ignore files are active
- which sources are optional
- what precedence is enforced
- which technical exclusions remain unconditional

## DuckDB Compatibility Requirement

## Problem to solve

When Axon performs heavy ingestion, live SQL and MCP queries can become slow or unresponsive.

This is expected if the system behaves like an unbounded writer.

The design must therefore preserve query service while scanning large universes.

## Required mechanism

Axon must implement a write-pressure controller that can temporarily slow, pause, or shrink structural ingestion when live query responsiveness degrades.

The controller must react before the system becomes unusable.

## Service policy

When pressure rises:

1. slow or stop semantic enrichment first
2. reduce bulk claiming
3. preserve hot-lane indexing
4. keep SQL and MCP alive

If pressure becomes critical:

- pause bulk ingestion
- preserve service
- resume progressively after recovery

## Compatibility with the current design

This concept is fully compatible with:

- one authoritative write path
- DB-backed pending work
- RAM queues as accelerators
- `SOLL` protection
- derived semantic layers

It explicitly rejects:

- uncontrolled write floods
- fixed ingestion speeds
- treating live query service as a best-effort side effect

## Control Metrics

The controller should observe at least:

- `pending` file count
- `indexing` file count
- RAM queue depth
- parse latency p50/p95
- writer commit latency p50/p95
- MCP latency p50/p95
- SQL latency p50/p95
- embedding backlog
- RSS memory

These metrics govern:

- claim size
- claim interval
- active worker count
- writer batch size
- semantic worker activity

## Hardware-Aware Runtime Profile

Axon must detect its host profile very early during startup.

This profile must provide a single source of truth for adaptive defaults such as:

- CPU core count
- total RAM
- reserved RAM headroom
- queue capacity
- worker count
- blocking thread budget
- semantic default posture
- future GPU presence and accelerator budget

This avoids hardcoded assumptions such as a fixed number of workers or fixed concurrency on every machine.

The profile is not the same as live backpressure.

- the runtime profile defines startup defaults
- backpressure adjusts behavior continuously during execution

## Operating Rule

Axon must follow one operating law:

> ingest as fast as possible, and as slowly as necessary

That means:

- accelerate when the system is healthy
- slow down when truth, service quality, or machine health are at risk

## Implementation Phases

### Phase 1. Unify `Axon Ignore`

- create one Rust ignore policy engine
- reuse it in the core scanner
- align the dashboard scanner with it

### Phase 2. Instrument the pipeline

- add backlog, latency, and pressure metrics
- expose service quality for MCP and SQL

### Phase 3. Introduce adaptive claiming

- replace fixed polling and fixed fetch sizes
- drive claiming from pressure and service health

### Phase 4. Separate work lanes

- `hot`
- `bulk`
- `titan`

The separation is now explicit in Rust memory queues:

- `hot` keeps reserved capacity and drains first
- `bulk` absorbs ordinary background work and hits backpressure before `hot`
- `titan` isolates oversized files so they do not poison the common lane

The structural commit path does not split:

- workers still converge into the same writer actor
- `IST` truth remains single-path
- lanes change scheduling and pressure isolation, not truth ownership

Canonical claim pressure now follows the common lane:

- `hot + bulk` drive claim slowdown
- `titan` backlog is isolated from that signal
- oversized files therefore stop degrading ordinary throughput prematurely
- if a RAM lane is saturated, a claimed file must be requeued back to `pending` in `IST` rather than being silently lost

### Phase 5. Make semantic work slack-driven

- suspend embeddings first under pressure
- resume only when structural service is healthy

### Phase 6. Add graph projections

- only after chunk-based retrieval is stable and useful

## Invariants

The following invariants must hold:

- `SOLL` is never purged by automatic ingestion recovery
- `IST` remains reconstructible
- structural truth is committed before semantic enrichment
- live SQL and MCP service remain protected under load
- a file accepted by `Axon Ignore` and by parser capabilities eventually reaches `IST`
- no adaptive mechanism may silently drop accepted work
