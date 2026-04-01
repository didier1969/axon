# Axon Delivery Plan

Date: 2026-04-01
Status: active master plan
Scope: single delivery plan from current verified state to a deliverable Axon runtime

## Purpose

This is the single canonical delivery plan for Axon.

It consolidates:

- verified current state
- remaining migration work
- hardening work
- developer-facing capability work
- delivery gates

It replaces fragmented planning as the main execution map.

## Current Verified Baseline

Already true as of 2026-04-01:

- official environment is `devenv shell`
- Rust core test suite is green
- dashboard test suite is green
- canonical runtime starts through `scripts/start-v2.sh`
- SQL and MCP surfaces respond correctly
- nominal backend is **Canard DB** (`DuckDB`)
- Rust already owns the main runtime plane
- Elixir is already partially reduced to visualization and telemetry
- memory-budget admission now exists in Rust
- confidence-aware estimation by parser class and size bucket now exists in Rust
- bounded candidate packing under budget now exists in Rust
- explicit `oversized_for_current_budget` handling now exists in Rust
- `Titan` has been removed from the canonical Rust runtime path
- fixed `>1MB => skip` behavior has been removed
- the legacy Elixir control chain `Server/Staging/PathPolicy/Oban/IndexingWorker/BatchDispatch` has been removed from the dashboard code path
- `PoolFacade` no longer exposes batch admission or pull APIs on the Elixir side
- the Rust bridge now emits runtime telemetry consumed by the Phoenix cockpit (`budget`, `reserved`, `exhaustion`, `queue_depth`, `claim_mode`, `service_pressure`, `oversized_refusals_total`, `degraded_mode_entries_total`)
- fairness debt now persists in `IST` (`defer_count`, `last_deferred_at_ms`) so repeatedly deferred large files can eventually outrank newer packable work
- cold oversized candidates now receive a bounded probation before definitive `oversized_for_current_budget` refusal
- no external scheduling crate has been adopted; current direction is a Rust-native Axon scheduler because existing FIFO semaphore/queue crates are a poor fit for budget packing

## Delivery Definition

Axon is considered delivered when all of the following are true:

1. Rust is the only canonical ingestion and scheduling authority.
2. Elixir/Phoenix is limited to visualization, operator telemetry, and read-side projections.
3. Ingestion remains stable under mixed waves of small, medium, and large files without host collapse.
4. Oversized files are handled explicitly and explainably.
5. Retrieval and operator-facing analysis are reliable enough for daily development use.
6. Documentation and runtime truth are aligned and restart-safe.
7. Delivery gates pass in the official environment.

## Phase 0: Keep Truth Stable

Goal:
- prevent the repo from drifting back into ambiguous or misleading state

Tasks:
- keep `README.md`, `STATE.md`, `ROADMAP.md`, and handoff docs aligned with executable truth
- keep `docs/archive/` historical only
- keep runtime validation centered on `devenv shell` and `start-v2.sh`
- ensure future exports and generated artifacts land in canonical locations only

Exit criteria:
- no canonical doc contradicts the executable runtime

## Phase 1: Finish Rust-Owned Ingestion Control

Goal:
- complete the migration from coarse queueing to Rust-owned adaptive admission

Tasks:
- keep the scheduler dependency-free for now; reuse existing runtime primitives unless a later repriorization need justifies a small queue crate
- keep memory-budget admission as the primary scheduling rule
- add degradation-before-refusal where feasible:
  - structure only
  - delayed semantics
  - lower priority retry
- expose the new Rust admission metrics cleanly to the operator plane

Exit criteria:
- no canonical ingest decision depends on fixed-size thresholds or Elixir routing
- admission decisions are explainable by budget, confidence, and priority

Already completed in this phase:

- fairness / anti-starvation now exists through persistent defer debt recorded in `File.defer_count`
- Phoenix cockpit shows `oversized_refusals_total` and `degraded_mode_entries_total`
- cold oversized candidates are first deferred during a bounded probation window instead of being refused on first sight

## Phase 2: Retire Residual Elixir Ingestion Authority

Goal:
- eliminate remaining control-plane ambiguity

Primary modules to retire or reduce:

- `Axon.Watcher.PoolFacade`
- `Axon.Watcher.PoolEventHandler`
- `Axon.Watcher.TrafficGuardian`
- `Axon.Watcher.StatsCache`
- residual control semantics around `Axon.BackpressureController`

Already completed in this phase:

- retired `Axon.Watcher.Server`
- retired `Axon.Watcher.Staging`
- retired `Axon.Watcher.PathPolicy`
- retired `Axon.Watcher.IndexingWorker`
- retired `Axon.Watcher.BatchDispatch`
- retired `Axon.Watcher.TrafficGuardian`
- removed legacy dashboard `Oban` ingestion configuration
- removed Elixir batch APIs `PoolFacade.parse_batch/1` and `PoolFacade.pull_pending/1`
- removed legacy batch telemetry from the dashboard bus
- removed dead compiled dashboard modules `AxonDashboardWeb.StatusLive`, `Axon.Watcher.StatsCache`, `Axon.Watcher.PoolEventHandler`, `Axon.Watcher.Auditor`, and `Axon.Watcher.Tracking`
- removed dead Ecto read-side modules `Axon.Watcher.IndexedProject` and `Axon.Watcher.IndexedFile`

Target state:

- Elixir may show pressure
- Elixir may relay operator intent
- Elixir must not own backlog, scheduling, retries, or canonical throttling

Exit criteria:
- Rust is the only runtime truth for discovery, staging, claims, admission, parsing, and backpressure
- dashboard startup path contains no hidden canonical ingestion path

## Phase 3: Host-Safety and Runtime Hardening

Goal:
- make Axon safe to run on constrained WSL and real developer machines

Tasks:
- add confidence-weighted memory estimation by parser class and size bucket
- expose runtime metrics for:
  - memory budget
  - reserved in-flight memory
  - exhaustion ratio
  - oversize refusals
  - degraded-mode counts
- add stronger host-pressure awareness where feasible:
  - Axon RSS
  - service latency
  - queue pressure
  - optional future host-wide memory / disk pressure signals
- verify no swap-thrash-inducing behavior under mixed workloads
- keep embeddings as the first work class to pause under pressure

Exit criteria:
- Axon degrades in a controlled way instead of driving the host into pathological thrash

Already completed in this phase:

- runtime telemetry bridge exports `budget`, `reserved`, `exhaustion`, `queue_depth`, `claim_mode`, `service_pressure`, `oversized_refusals_total`, and `degraded_mode_entries_total`
- Phoenix cockpit displays these Rust-origin signals without regaining scheduling authority
- Phoenix cockpit also reflects host-pressure telemetry (`cpu`, `ram`, `io_wait`, queue constrained/resumed state, indexing guidance) as read-side operator signals

## Phase 4: Retrieval and Developer Utility

Goal:
- make Axon genuinely useful as a development intelligence system, not just a stable ingestor

Tasks:
- strengthen developer-facing retrieval quality
- strengthen impact analysis before change
- strengthen technical debt and quality surfacing
- improve continuity between `IST`, `GraphProjection`, derived chunks, and MCP answers
- preserve `SOLL` continuity and high-signal project memory

Exit criteria:
- Axon can answer development queries reliably enough to support everyday work

## Phase 5: Derived Layers and Semantic Consolidation

Goal:
- make the higher-level graph and semantic layers robust, not decorative

Tasks:
- continue hardening `GraphProjection`
- continue hardening chunk derivation and invalidation
- continue hardening embedding freshness and drift handling
- improve semantic clone and neighborhood logic only after structural truth is stable
- add cross-project reasoning only after single-project stability is strong

Exit criteria:
- derived layers remain reconstructible, coherent, and subordinate to structural truth

## Phase 6: Operator Surface and Observability

Goal:
- make the dashboard useful because it reflects Rust truth, not because it invents control

Tasks:
- show canonical Rust pressure, mode, and budget state in the dashboard
- show host-pressure signals useful to the operator when queues are constrained
- expose oversize/degraded/paused reasons clearly
- keep operator actions explicit and auditable
- preserve telemetry without reintroducing Elixir scheduling authority

Exit criteria:
- the dashboard is a truthful cockpit, not a parallel control plane

## Phase 7: Delivery Cleanup

Goal:
- make the repo and runtime shippable for continued development and takeover

Tasks:
- remove or archive superseded plans after they are absorbed
- keep one canonical handoff and one canonical delivery plan
- remove leftover compatibility code that no longer serves runtime behavior
- align status docs, architecture docs, and startup docs

Exit criteria:
- a new human or LLM can resume the project without doubting what is current, historical, or canonical

## Execution Order

The rational execution order is:

1. finish Rust-owned adaptive admission
2. retire residual Elixir ingestion authority
3. harden host safety and runtime metrics
4. strengthen retrieval and developer utility
5. harden derived semantic layers
6. finish operator surface around Rust truth
7. cleanup and freeze delivery documentation

## Delivery Gates

Axon is not declared delivered until all gates below are green.

### Gate A: Build and Test

- `devenv shell -- bash -lc 'cd src/axon-core && cargo test --manifest-path Cargo.toml'`
- `devenv shell -- bash -lc 'cd src/dashboard && mix local.hex --force >/dev/null && mix local.rebar --force >/dev/null && mix test'`

### Gate B: Runtime Boot

- `bash scripts/start-v2.sh`
- SQL responds
- MCP responds
- dashboard responds
- `bash scripts/stop-v2.sh`

### Gate C: Ingestion Safety

Required scenarios:

- many small files
- mixed medium files
- one large file
- large-file wave
- parser class with no prior confidence
- oversized file that does not fit even alone

Success condition:

- no host collapse
- no uncontrolled memory growth
- explicit degraded or refused state when necessary

### Gate D: Architecture Boundary

- no canonical ingestion decision lives in Elixir
- dashboard remains display/operator plane only

### Gate E: Retrieval Usefulness

- representative developer workflows produce trustworthy enough results
- impact / quality / debt answers remain grounded in current `IST`

## What Is Already Done vs Remaining

Already done:

- repo truth cleanup and archive boundary
- canonical export path correction for `SOLL`
- verification of official environment and runtime
- first Rust memory-budget scheduler slice
- confidence-aware dynamic admission slice
- budget-aware candidate packing slice
- removal of the Elixir legacy dispatch chain from the dashboard tree
- removal of fixed-size skip
- removal of canonical `Titan` behavior in Rust
- initial Elixir de-authoring slice

Still remaining:

- confidence-aware cold start
- explicit oversize status path
- budget-aware candidate packing
- fairness for delayed large files
- removal of residual Elixir ingestion modules
- richer host-safety observability
- delivery-grade retrieval hardening
- final cleanup and freeze of canonical docs

## Immediate Next Slice

The next highest-value slice is:

1. implement confidence-aware cold start
2. implement explicit oversize refusal
3. implement budget-aware candidate packing

This is the shortest path to making the new scheduler both safer and more efficient.
