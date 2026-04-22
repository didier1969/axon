# Axon Brain / Axon Indexer Split Concept

> **For Claude:** REQUIRED SUB-SKILL: Use `idea-to-delivery` / `consensus-driven-delivery` for independent review before planning implementation. Do not jump directly into code split work from this concept without reviewer convergence.

## Goal

Split the current Axon runtime into two applications with cleaner ownership:

- `axon-brain`
  - MCP server
  - SQL read path
  - operator/dashboard surfaces
  - `SOLL` read/write ownership
  - `IST` read-only snapshot access
- `axon-indexer`
  - watcher
  - scan/ingress
  - graph production
  - chunk/vector production
  - finalize/persist
  - exclusive `IST` writer ownership

The purpose is not aesthetic modularity. The purpose is to remove control-plane instability caused by sharing one process lifecycle with ingestion, graphing, and vectorization.

## Primary success criteria

This split is justified first as a control-plane isolation move, not as a throughput fix.

Success means:

- MCP latency remains stable under heavy indexing load
- operator/dashboard truth remains available while indexing is busy
- writer authority is unambiguous by database
- public status can still explain live runtime state, not just persisted table state

This split is **not** considered successful merely because code ownership looks cleaner.

## Why this split is justified by the real system

Runtime evidence and code inspection show the current model is still too coupled:

- `scripts/start.sh` treats runtime modes as one executable with feature flags:
  - `full`
  - `graph_only`
  - `read_only`
  - `mcp_only`
- `src/axon-core/src/main_services.rs` starts:
  - indexing workers
  - semantic workers
  - MCP/SQL HTTP server
  from one runtime boot path
- startup and qualification behavior has repeatedly shown that ingestion/bootstrap turbulence can pollute MCP/control-plane readiness and benchmarking windows
- `IST` is repopulated aggressively and the current runtime reuses the same process authority for:
  - ingestion and writer pressure
  - MCP serving
  - SQL read path
  - status truth

This means the current architecture still mixes:

1. control-plane responsibility
2. data-plane write-heavy ingestion
3. query-plane/operator-facing read serving

That coupling makes the system harder to benchmark, harder to reason about, and more fragile under heavy indexing load.

It does **not** prove that process splitting alone will improve upstream admission throughput. Current throughput evidence still points primarily at upstream supply/admission behavior.

## Current architecture reality

Today the runtime is effectively a monolith with mode-based suppression:

- one core binary
- one runtime mode enum deciding which workers are spawned
- one MCP/SQL server embedded in the same service boot path
- one dashboard process adjacent to that runtime

This is a useful transitional architecture, but not a clean ownership model.

## Proposed architecture

### Application 1: `axon-brain`

Responsibilities:

- own the MCP server and public query contract
- own SQL read/query exposure for operators and tooling
- own dashboard/operator-facing status surfaces
- own `SOLL` mutations and `SOLL` consistency
- read from `IST` in snapshot/read-only mode
- expose freshness/lag metadata when `IST` trails `SOLL` or indexing activity

Database ownership:

- `SOLL` writer: exclusive to `axon-brain`
- `SOLL` readers: local to `axon-brain`
- `IST` readers: local read-only snapshot access only

Operational goal:

- stable latency and truth reporting even while indexing is busy
- public operator/MCP truth that remains explicit about freshness and indexer lag

### Application 2: `axon-indexer`

Responsibilities:

- watcher and scan discovery
- ingress buffering and admission
- file persistence into canonical `IST`
- graph production
- chunk extraction and semantic preparation
- vectorization and downstream finalize

Database ownership:

- `IST` writer: exclusive to `axon-indexer`
- `IST` execution-local reads: allowed for indexing control loops
- no `SOLL` write authority

Operational goal:

- maximize throughput from discovery to `IST` without perturbing operator-serving paths

## Cross-plane runtime truth contract

`IST` snapshot access alone is not enough for `axon-brain`.

Current `status(full)` truth also depends on live runtime surfaces such as:

- ingress buffer state
- admission controller state
- graph/vector control state
- service-pressure state
- finalize/runtime telemetry

So the split requires an explicit control-plane feed from `axon-indexer` to `axon-brain`.

Minimum contract:

- `axon-indexer` exports authoritative runtime telemetry/status
- `axon-brain` consumes that feed and republishes it as the public operator/MCP truth surface
- `axon-brain` combines:
  - `SOLL` truth it owns locally
  - `IST` read-only snapshot truth
  - live runtime-control truth sourced from `axon-indexer`

This contract must be first-class. `axon-brain` cannot infer full runtime authority from `IST` alone.

### Runtime-truth transport requirements

Phase-1 transport requirements:

- heartbeat or snapshot cadence must be explicit
- stale threshold must be explicit
- required fields for public status must be explicit
- last-good snapshot behavior must be explicit
- operator-visible degraded state must be explicit when the feed is stale or absent

Whether the transport is implemented as push, pull, or hybrid is an implementation decision. The concept only requires that freshness semantics are first-class and testable.

## Canonical authority model

### `SOLL`

- source of operator intent, contract, and MCP mutation truth
- writer authority belongs to `axon-brain`

### `IST`

- source of indexed/runtime knowledge truth
- writer authority belongs to `axon-indexer`
- reader authority belongs to both applications, but only `axon-indexer` writes it

This is a split of process ownership, not a split into duplicate truths.

## Reader/writer contract

The split is only valid if writer ownership is strict and freshness is explicit.

Rules:

- one writer for `SOLL`
- one writer for `IST`
- `axon-brain` must treat `IST` as a snapshot-backed read surface, not as a writable coordination store
- `axon-indexer` must not serve the public MCP contract directly
- cross-plane status must expose staleness:
  - last successful `IST` refresh observed by `axon-brain`
  - lag between `SOLL` mutation truth and visible `IST` index truth when relevant
  - last successful runtime-truth heartbeat received from `axon-indexer`
  - explicit degraded state when runtime truth is stale even if `IST` is readable

## `IST` read model

The split must choose an explicit read topology for `axon-brain`.

Preferred initial model:

- one writer-active `IST` owned by `axon-indexer`
- `axon-brain` opens read-only/snapshot-safe connections against the same DuckDB surface
- `axon-brain` treats freshness as bounded and reports lag explicitly

This concept does **not** require a second `IST` copy or a new replica system in phase 1.

If the shared-file read model proves unstable under WAL or snapshot refresh pressure, a later read-replica/export path can be considered. That is not a phase-1 assumption.

Fallback rule:

- if `axon-brain` cannot maintain trustworthy read-only `IST` visibility, it must degrade explicitly to control-plane status plus stale-index truth
- it must not present full runtime/index truth as healthy while the read model is unstable

## Command routing contract

Giving `axon-brain` the public MCP contract requires explicit routing for runtime-affecting operations.

Rules:

- `axon-brain` serves the public MCP API
- MCP tools that operate on `SOLL` stay local to `axon-brain`
- runtime-affecting tools that need live indexing authority are proxied from `axon-brain` to `axon-indexer`
- `axon-indexer` does not expose the public MCP surface directly to operators
- tools with ambiguous ownership must be made explicit before migration

The split must therefore define:

- which tools are brain-local
- which tools are proxied to indexer
- which tools are disallowed during rollout or degraded mode

Initial ownership classes:

- brain-local read
  - public read/query/status work that depends on `SOLL`, stable `IST` snapshots, and last-good runtime truth
- brain-local write
  - `SOLL` mutations and control-plane-local operator actions
- proxied runtime mutation
  - actions that need live indexing authority or `IST` writer authority
- degraded/disallowed
  - actions that would be misleading or unsafe while runtime feed freshness is outside threshold

## Control-loop consequences

This split aligns with the two-loop runtime model already established:

1. upstream push loop
   - watcher
   - buffered discovery
   - persisted file pending
   - graph ready
2. downstream GPU-paced pull loop
   - graph ready
   - prepare
   - ready batches
   - GPU
   - finalize

`axon-indexer` owns both loops.

`axon-brain` does not participate in those control loops. It observes their truth and exposes it.

## Why this is better than the current runtime-mode split

The current modes (`full`, `graph_only`, `read_only`, `mcp_only`) suppress behavior inside one process family.

That is weaker than:

- distinct process lifecycles
- distinct resource budgets
- explicit writer ownership
- clean failure boundaries

The new split improves:

- operator-serving stability
- MCP latency isolation
- benchmark clarity
- fault containment
- architectural readability

It should not be marketed as a direct throughput win until measured separately.

## Non-goals

- do not create two copies of `IST`
- do not create two copies of `SOLL`
- do not move public MCP serving into the indexing application
- do not let `axon-brain` become a hidden writer to `IST`
- do not attempt a full codebase rewrite before contracts and boot topology are explicit

## Reuse vs change

### Reuse

- keep one repository
- keep shared core libraries where they remain valid
- keep current DuckDB-based truth surfaces
- keep existing dashboard attached to the control-plane application
- keep existing MCP tools and public contracts under `axon-brain`

### Change

- replace one runtime binary with two explicit applications or entrypoints
- move MCP/SQL serving out of the indexing boot path
- move indexing workers out of the control-plane boot path
- replace runtime mode suppression with explicit process composition
- introduce cross-plane freshness and lag reporting as first-class status

## Migration shape

Recommended migration sequence:

1. split boot topology first
   - two entrypoints
   - same shared code where possible
2. preserve current DB schema and truth surfaces
3. move MCP/SQL/dashboard into `axon-brain`
4. move watcher/ingestion/graph/vector into `axon-indexer`
5. add explicit status/freshness contracts between the two
6. only then simplify or remove old runtime modes

## Readiness and qualification topology

The split needs separate readiness surfaces:

- `brain_ready`
  - MCP/query/control-plane ready
- `indexer_ready`
  - watcher/ingestion/indexing plane ready
- `system_converged`
  - both planes healthy and cross-plane truth fresh enough for full operator confidence

Operator rule:

- `brain_ready` alone is not enough to claim full runtime truth
- only `system_converged` can be shown as fully green for end-to-end operator confidence

Qualification must test at least:

- control-plane availability without indexer
- indexer availability without brain
- MCP latency under indexing pressure
- stale-runtime detection when indexer lags or disappears
- stale-`IST` detection when snapshot truth trails writer activity
- no dual-write authority for `SOLL` or `IST`

## Main risks

1. stale-read ambiguity
   - `axon-brain` may show valid but lagging `IST` truth unless freshness is explicit
2. rollout complexity
  - two processes, two lifecycles, more deployment choreography
3. hidden coupling in current code
  - shared modules may still assume one-process boot order
4. accidental dual-write authority
  - must be prevented at design level, not only by convention
5. false confidence from stale control-plane truth
   - `axon-brain` can look healthy while its runtime feed is stale unless degradation is explicit

## Decision

Proceed with a split into:

- `axon-brain` as control-plane/query-plane application
- `axon-indexer` as data-plane indexing application

under these constraints:

- strict writer ownership by database
- shared canonical truths, not duplicated truths
- explicit lag/freshness surfaces
- explicit runtime-truth feed from `axon-indexer` to `axon-brain`
- explicit proxy model for runtime-affecting MCP actions
- migration by topology split first, schema churn later only if proven necessary
- no claim that this split alone fixes the primary upstream throughput bottleneck
