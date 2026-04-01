# Rust Memory-Budget Ingestion Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Move remaining ingestion authority out of Elixir and implement a Rust-owned memory-budget admission scheduler that adapts file concurrency to observed memory cost instead of relying on a coarse `titan` path.

**Architecture:** Rust becomes the only canonical ingestion scheduler. Elixir is reduced to visualization and operator-facing telemetry. The existing queue and pressure logic are migrated toward a dynamic memory-budget model that estimates per-file cost, reserves in-flight budget, and admits work only when safe headroom exists.

**Tech Stack:** Rust, crossbeam channels, DuckDB/Canard DB runtime, Elixir/Phoenix tests, ExUnit, cargo test

---

## Status Update 2026-04-01

Already delivered in code and verified:

- Rust memory-budget admission
- confidence-aware cold start by `parser class + size bucket`
- explicit `oversized_for_current_budget` refusal
- bounded candidate packing under budget
- removal of the canonical Rust `Titan` path
- dynamic claim throttling based on combined runtime pressure

Still remaining for this plan:

- degradation-before-refusal where feasible
- fairness / anti-starvation for delayed large files
- operator-surface exposure of the new Rust admission metrics

## Dependency Decision

After reviewing existing crates and official docs:

- no new scheduling dependency is adopted for this slice
- FIFO semaphores are a poor fit because they block the desired packing behavior
- Axon keeps a dedicated scheduler in local Rust code for budget, confidence, packing, and fairness

## Additional Constraints Added After Initial Design

The scheduler must now satisfy two extra behaviors explicitly:

1. `cold start by confidence`
   - a parser class must start conservatively while Axon has little or no history for it
   - concurrency may increase only after enough successful observations exist for that parser/size bucket

2. `budget-first packing and explicit oversize refusal`
   - a file must not be skipped only because it crosses a fixed byte threshold
   - if a file cannot fit even alone inside the effective budget, Axon must mark it explicitly as oversized for the current runtime envelope
   - when the next candidate does not fit, the scheduler should still admit a better-fitting combination of medium/small files instead of stalling behind FIFO order

These constraints refine the original plan; they do not replace it.

### Task 1: Freeze the de-authoring boundary in tests and docs

**Files:**
- Modify: `src/dashboard/test/axon_nexus/axon/backpressure_controller_test.exs`
- Modify: `src/dashboard/test/axon_nexus/axon/watcher/application_visualization_test.exs`
- Modify: `docs/architecture/2026-03-30-rust-first-elixir-visualization.md`
- Modify: `docs/working-notes/2026-04-01-reprise-handoff.md`

**Step 1: Write the failing dashboard expectation**

Add or refine tests so they express:

- Elixir remains display-only for pressure semantics
- visualization still boots correctly
- no Elixir test asserts canonical ingestion scaling authority

**Step 2: Run the targeted tests**

Run:

```bash
devenv shell -- bash -lc 'cd src/dashboard && mix test test/axon_nexus/axon/backpressure_controller_test.exs test/axon_nexus/axon/watcher/application_visualization_test.exs'
```

Expected: green if current display-only behavior is already true, otherwise failing assertions reveal remaining control semantics.

**Step 3: Update docs to match the migration boundary**

Record that:

- routing and admission must converge into Rust
- Elixir may render telemetry but must not own canonical scheduling

### Task 2: Introduce failing Rust tests for memory-budget admission

**Files:**
- Modify: `src/axon-core/src/queue.rs`
- Modify: `src/axon-core/src/main_background.rs`
- Modify: `src/axon-core/src/tests/maillon_tests.rs`

**Step 1: Add focused tests**

Add failing tests that cover:

- many small files fit concurrently within budget
- a large file reduces effective concurrency
- a second large file is not admitted if it would exceed budget
- admissions resume after budget is released

**Step 2: Run focused tests to verify failure**

Run:

```bash
devenv shell -- bash -lc 'cd src/axon-core && cargo test --manifest-path Cargo.toml memory_budget'
```

Expected: failures because the budget scheduler does not yet exist.

### Task 3: Add the Rust memory-budget model

**Files:**
- Create or Modify: `src/axon-core/src/runtime_profile.rs`
- Modify: `src/axon-core/src/queue.rs`
- Modify: `src/axon-core/src/main_background.rs`
- Modify: `src/axon-core/src/worker.rs`

**Step 1: Add budget primitives**

Introduce a Rust-owned budget model with:

- `memory_budget_bytes`
- `memory_reserved_bytes`
- file cost estimation from size
- safety multiplier

**Step 2: Track reservations**

When a file is admitted:

- reserve estimated bytes

When a file completes or is skipped:

- release reserved bytes

**Step 3: Make admission budget-aware**

Do not enqueue or claim work that would exceed the effective budget.

**Step 4: Keep the hot path ordered but safe**

Priority can change order, but not bypass memory admission.

### Task 4: Add observed-cost feedback

**Files:**
- Modify: `src/axon-core/src/worker.rs`
- Modify: `src/axon-core/src/main_background.rs`
- Modify: `src/axon-core/src/queue.rs`
- Modify: `src/axon-core/src/graph.rs` or the appropriate persistence layer if needed

**Step 1: Measure runtime cost**

Record a measured processing cost per completed file, at minimum via:

- file size
- parser/language class
- observed memory proxy or measured peak delta where feasible

**Step 2: Feed estimates back**

Use recent observed cost to adjust future estimates for similar files.

**Step 3: Add confidence-aware cold start**

Track at least:

- sample count per parser class
- size bucket per parser class
- confidence level for each class

Then:

- keep a stronger safety margin while confidence is low
- relax the margin only after enough stable observations exist
- fall back to conservative estimates immediately after outliers

**Step 4: Preserve safe fallback**

If no history exists, keep the conservative base estimate.

### Task 5: Replace fixed-size skip with explicit oversize classification

**Files:**
- Modify: `src/axon-core/src/worker.rs`
- Modify: `src/axon-core/src/queue.rs`
- Modify: `src/axon-core/src/graph.rs` or the appropriate persistence/status layer
- Modify: `docs/architecture/2026-03-30-adaptive-ingestion-concept.md`

**Step 1: Remove the fixed byte-threshold skip**

Do not reject a file only because it is larger than a hardcoded limit such as `1MB`.

**Step 2: Introduce explicit oversize refusal**

If a file's estimated cost exceeds the effective budget even when admitted alone:

- refuse it explicitly
- record a precise reason such as `oversized_for_current_budget`
- keep the refusal explainable to operators and to the LLM

**Step 3: Try degradation before final refusal when possible**

Attempt safer modes first where feasible:

- structure only
- semantics delayed
- lower scheduling priority

Only after those paths still do not fit should the runtime refuse the file.

### Task 6: Add budget-aware candidate packing instead of naive FIFO

**Files:**
- Modify: `src/axon-core/src/queue.rs`
- Modify: `src/axon-core/src/main_background.rs`
- Modify: `src/axon-core/src/graph.rs` if claim selection must see more candidate metadata

**Step 1: Inspect a bounded candidate window**

When claiming/admitting work:

- inspect a bounded number of eligible candidates
- do not commit to the first file blindly if it does not fit

**Step 2: Admit the best-fitting batch under budget**

Select a combination that:

- respects hot-path ordering first
- maximizes budget use without exceeding it
- allows multiple medium/small files to run together when a single large file would stall progress

**Step 3: Preserve fairness**

Prevent one large file from starving forever by adding:

- retry windows
- aging
- or another bounded fairness rule

### Task 7: Remove remaining Elixir routing authority for large-file handling

**Files:**
- Modify: `src/dashboard/lib/axon_nexus/axon/watcher/server.ex`
- Modify: `src/dashboard/lib/axon_nexus/axon/watcher/indexing_worker.ex`
- Modify: `src/dashboard/lib/axon_nexus/axon/backpressure_controller.ex`
- Modify: `src/dashboard/lib/axon_nexus/axon/watcher/application.ex`

**Step 1: Stop Elixir from classifying files for canonical scheduling**

Remove or neutralize logic where Elixir decides that a file is `titan` for the canonical path.

**Step 2: Keep only visualization-safe telemetry**

Preserve only:

- display of pressure
- display of runtime status
- operator-triggered actions relayed to Rust

**Step 3: Re-run targeted dashboard tests**

Run:

```bash
devenv shell -- bash -lc 'cd src/dashboard && mix test test/axon_nexus/axon/backpressure_controller_test.exs test/axon_nexus/axon/watcher/application_visualization_test.exs'
```

Expected: green.

### Task 8: Remove remaining `titan` semantics

**Files:**
- Modify: `src/axon-core/src/queue.rs`
- Modify: `src/axon-core/src/worker.rs`
- Modify: `docs/architecture/2026-03-30-adaptive-ingestion-concept.md`

**Step 1: Remove semantic dependence on `titan`**

`Titan` is no longer canonical in Rust runtime scheduling.
Any remaining mention should now be removed or archived instead of preserved as a live compatibility path.

**Step 2: Make budget the primary rule**

Document and implement that memory budget, not `titan`, is the canonical admission mechanism.

### Task 9: Verify the full ingestion safety envelope

**Files:**
- Verify only

**Step 1: Run Rust test suite**

```bash
devenv shell -- bash -lc 'cd src/axon-core && cargo test --manifest-path Cargo.toml'
```

Expected: green.

**Step 2: Run dashboard test suite**

```bash
devenv shell -- bash -lc 'cd src/dashboard && mix local.hex --force >/dev/null && mix local.rebar --force >/dev/null && mix test'
```

Expected: green.

**Step 3: Optional runtime validation**

```bash
bash scripts/start-v2.sh
```

Verify:

- MCP and SQL remain responsive
- large-file waves do not cause uncontrolled queue admission
- dashboard still renders pressure and status truthfully

**Step 4: Stop runtime**

```bash
bash scripts/stop-v2.sh
```
