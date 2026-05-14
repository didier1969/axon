# GPU-First Pull Refill Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Make the vector lane perform real pull-based refill so the GPU can rebuild ready stock when underfed, instead of staying throttled by graph priority while the ready queue collapses.

**Architecture:** Introduce an explicit underfed/refill override in the utility-first scheduler so vector refill can temporarily outrank graph priority when GPU supply is low. Then align the vector worker loop so refill urgency can actually materialize into more prepare dispatches, bounded by VRAM safety and explicit refill caps rather than passive graph-priority sleeps.

**Tech Stack:** Rust, crossbeam channels, runtime controller logic in `axon-core`, existing qualification scripts and SQLite benchmark mirror.

---

### Task 1: Freeze the current behavior with failing scheduler tests

**Files:**
- Modify: `src/axon-core/src/embedder.rs`
- Test: `src/axon-core/src/embedder.rs`

**Step 1: Write the failing tests**

Add tests near the existing scheduler tests covering:
- underfed vector lane with graph backlog present should prefer refill over graph priority
- refill override should produce a faster semantic policy than `graph_priority`

Test shape:

```rust
#[test]
fn test_utility_first_scheduler_prefers_refill_override_when_underfed_even_with_graph_backlog() {
    let _guard = ENV_TEST_GUARD.lock().unwrap();
    crate::service_guard::reset_for_tests();
    reset_utility_first_scheduler_for_tests();
    crate::service_guard::record_vector_ready_queue_depth(0);
    crate::service_guard::record_vector_prepare_inflight_depth(0);

    let diagnostics =
        current_utility_first_scheduler_diagnostics(256, 128, ServicePressure::Healthy);

    assert!(diagnostics.semantic_underfeed);
    assert_eq!(diagnostics.state, UtilityFirstSchedulerState::BalancedDrain);
    assert_eq!(diagnostics.reason, "semantic_underfed");
}
```

```rust
#[test]
fn test_semantic_policy_with_graph_uses_refill_profile_when_underfed() {
    let _guard = ENV_TEST_GUARD.lock().unwrap();
    crate::service_guard::reset_for_tests();
    reset_utility_first_scheduler_for_tests();
    crate::service_guard::record_vector_ready_queue_depth(0);
    crate::service_guard::record_vector_prepare_inflight_depth(0);

    let policy = semantic_policy_with_graph(128, 256, ServicePressure::Healthy);

    assert_eq!(policy.profile, "semantic_refill");
    assert_eq!(policy.sleep, Duration::from_millis(100));
    assert_eq!(policy.idle_sleep, Duration::from_millis(250));
}
```

**Step 2: Run test to verify it fails**

Run:

```bash
cargo test --manifest-path src/axon-core/Cargo.toml test_utility_first_scheduler_prefers_refill_override_when_underfed_even_with_graph_backlog test_semantic_policy_with_graph_uses_refill_profile_when_underfed -- --test-threads=1
```

Expected: FAIL because the current code returns `GraphPriority` / `graph_priority`.

**Step 3: Commit**

```bash
git add src/axon-core/src/embedder.rs
git commit -m "test: capture missing pull refill override"
```

### Task 2: Add an explicit refill-override scheduler state

**Files:**
- Modify: `src/axon-core/src/vector_control.rs`
- Test: `src/axon-core/src/embedder.rs`

**Step 1: Implement the new scheduler state**

Add a new `UtilityFirstSchedulerState` variant:

```rust
RefillOverride,
```

Update:
- `UtilityFirstSchedulerState::as_str`
- `current_utility_first_scheduler_diagnostics`
- `target_semantic_policy_with_graph`

Rules:
- if `semantic_underfeed` is true and service pressure is healthy and interactive is inactive, choose `RefillOverride` even when `graph_queue_depth > 0`
- keep `RecoveryOverride` and interactive guards above refill override
- explicitly update both reason ordering and hold-window behavior so `RefillOverride` is not trapped behind an existing `GraphPriority` hold
- return a dedicated profile:

```rust
semantic_policy_profile("semantic_refill", false, 100, 250)
```

Add tests for:
- `graph_queue_depth > 0` + underfeed should yield `RefillOverride`, not `GraphPriority`
- `GraphPriority -> RefillOverride` transition should bypass or intentionally override the hold window
- interactive and recovery states must still outrank refill override

**Step 2: Run tests**

Run:

```bash
cargo test --manifest-path src/axon-core/Cargo.toml test_utility_first_scheduler_ test_semantic_policy_with_graph_ -- --test-threads=1
```

Expected: PASS for old and new scheduler tests.

**Step 3: Commit**

```bash
git add src/axon-core/src/vector_control.rs src/axon-core/src/embedder.rs
git commit -m "feat: let vector refill override graph priority when underfed"
```

### Task 3: Remove the real refill bottlenecks in prepare dispatch

**Files:**
- Modify: `src/axon-core/src/embedder.rs`
- Test: `src/axon-core/src/embedder.rs`

**Step 1: Write the failing tests**

Add tests for both:
- `single_worker_gpu_prepare_prefetch_limits(...)`
- the bounded prepare queue / dispatch path around `configured_vector_prepare_queue_bound()` and `prepare_tx.send(...)`

The failing tests should prove that refill urgency can still be blocked by runtime caps even if helper math says “dispatch more”.

```rust
#[test]
fn test_single_worker_gpu_prepare_prefetch_limits_expand_more_under_refill_urgency() {
    let controller = VectorBatchControllerDiagnostics {
        state: VectorBatchControllerState::IdleDrain,
        reason: "ready_queue_starved".to_string(),
        adjustments_total: 1,
        last_adjustment_ms: 10_000,
        target_embed_batch_chunks: 192,
        target_files_per_cycle: 64,
        window_embed_calls: 2,
        window_chunks: 32,
        window_files_touched: 8,
        avg_chunks_per_embed_call: 16.0,
        avg_files_per_embed_call: 4.0,
        embed_ms_per_chunk: 15.0,
    };

    let (max_inflight, request_cap) =
        single_worker_gpu_prepare_prefetch_limits(true, 12, &controller, 32, 6_000);

    assert!(max_inflight >= 24);
    assert!(request_cap >= 16);
}
```

Add a second test that validates the queue-bound path explicitly, by forcing a tiny prepare queue and showing that refill stalls until the cap is raised or made refill-aware.

**Step 2: Implement refill-aware real caps**

Change `single_worker_gpu_prepare_prefetch_limits(...)` so that:
- when aggressive prefetch is enabled and `controller.reason` indicates starvation, the returned `max_inflight_prepares` can scale toward the refill target
- `request_ready_depth_ceiling` can exceed the current `8/12` style mini-cap
- keep VRAM-safe behavior untouched; this is dispatch-side, not batch-size-side

Also make the actual runtime caps consistent with that intent:
- handle `configured_vector_prepare_queue_bound()` explicitly
- ensure refill urgency is not still throttled by a tiny bounded prepare queue
- keep the change limited to refill capacity, not GPU batch size

**Step 3: Run tests**

Run:

```bash
cargo test --manifest-path src/axon-core/Cargo.toml test_single_worker_gpu_prepare_prefetch_limits_ test_request_prepared_vector_embed_sequence_ -- --test-threads=1
```

Expected: PASS for old and new prefetch-limit tests.

**Step 4: Commit**

```bash
git add src/axon-core/src/embedder.rs
git commit -m "feat: remove refill bottlenecks from prepare dispatch path"
```

### Task 4: Verify the worker loop only where the scheduler change actually lands

**Files:**
- Modify: `src/axon-core/src/embedder.rs:1600-1865`
- Test: `src/axon-core/src/embedder.rs`

**Step 1: Keep loop changes minimal and targeted**

The main choke currently happens before refill work starts, via:
- `semantic_policy_with_graph(...)`
- `target_semantic_policy_with_graph(...)`

So Task 4 must not introduce a second overlapping refill policy inside the active loop unless implementation proves it is still needed after Task 2.

Allowed scope:
- wire through any new scheduler diagnostics the loop genuinely needs
- remove redundant branches if the scheduler change already fixes the choke
- do not invent a second refill mechanism inside the loop

Do not:
- change batch token sizing
- change VRAM guard behavior
- change graph workers

**Step 2: Add a narrow unit test**

Add a helper-oriented test only if the loop receives a new branch or new signal from the scheduler. If no loop change is needed after Task 2, delete this task during implementation rather than adding redundant logic.

**Step 3: Run tests**

Run:

```bash
cargo test --manifest-path src/axon-core/Cargo.toml test_utility_first_scheduler_ test_semantic_policy_with_graph_ test_single_worker_gpu_prepare_prefetch_limits_ -- --test-threads=1
```

Expected: PASS.

**Step 4: Commit**

```bash
git add src/axon-core/src/embedder.rs src/axon-core/src/vector_control.rs
git commit -m "refactor: keep vector loop aligned with pull refill scheduler"
```

### Task 5: Restore reserve baseline to the last known sane value before benchmarking

**Files:**
- Modify: `src/axon-core/src/vector_control.rs`
- Test: `src/axon-core/src/embedder.rs`

**Step 1: Revert the experiment-only reserve floor**

Set:

```rust
const READY_RESERVE_FLOOR: usize = 16;
const READY_RESERVE_HEAVY_BACKLOG: usize = 24;
const READY_RESERVE_EXTREME_BACKLOG: usize = 32;
```

Keep the new refill override behavior from Tasks 2-4.

**Step 2: Update tests**

Restore reserve expectation tests so they match the `16/24/32` baseline again.

**Step 3: Run tests**

Run:

```bash
cargo test --manifest-path src/axon-core/Cargo.toml test_vector_ready_reserve_target_ -- --test-threads=1
```

Expected: PASS.

**Step 4: Commit**

```bash
git add src/axon-core/src/vector_control.rs src/axon-core/src/embedder.rs
git commit -m "refactor: keep refill override while restoring sane reserve baseline"
```

### Task 6: Run a VRAM-safe qualification benchmark and compare against baseline

**Files:**
- Use: `scripts/benchmark-vector-token-matrix.sh`
- Read: `.axon/benchmarks/.../results.tsv`
- Read: `.axon-dev/run/benchmark.sqlite3`

**Step 1: Run the benchmark**

Run:

```bash
env AXON_INSTANCE_KIND=dev bash scripts/benchmark-vector-token-matrix.sh \
  --mode warm \
  --duration 120 \
  --interval 1 \
  --tokens 16000 \
  --ready-depth 96 \
  --pipeline-depth 12 \
  --prepare-workers 8 \
  --max-items 128 \
  --max-batch-bytes 8388608 \
  --graph-workers 2 \
  --max-vram-used-mb 7000 \
  --gpu-admission-vram-used-mb 6300 \
  --label-prefix oven-pull-refill-v1
```

Expected:
- `vram_budget_exceeded = 0`
- new `results.tsv` line written

**Step 2: Inspect batch-level metrics**

Query the mirror scoped to the run window from the benchmark summary or qualification timestamps:

```bash
python3 - <<'PY'
import sqlite3
run_started_ms = ...  # read from benchmark/qualification artifacts
run_finished_ms = ... # read from benchmark/qualification artifacts
con = sqlite3.connect('.axon-dev/run/benchmark.sqlite3')
for row in con.execute(\"\"\"
select chunk_count, total_tokens, gpu_used_mb,
       ready_queue_depth_at_gpu_start,
       prepare_inflight_at_gpu_start, batch_wait_for_ready_ms,
       (gpu_finished_at_ms - gpu_started_at_ms) as gpu_window_ms,
       vector_worker_admission_reason, allowed_gpu_workers
from vector_batch_run
where started_at_ms >= ? and started_at_ms <= ?
order by started_at_ms asc
\"\"\"):
    print(row)
PY
```

Expected comparison target versus old baseline:
- `window_chunks_per_second` should beat or at least approach the previous `6.98 chunks/s`
- `ready_queue_depth_at_gpu_start` should no longer sit at `2..5` for the entire useful window
- `batch_wait_for_ready_ms` should materially drop
- `vector_worker_admission_reason` should not reveal refill starvation hidden behind a guard state

**Step 3: Validate VRAM-safe acceptance criteria**

Read `results.tsv` and reject the run if any of the following are true:

```text
vram_budget_exceeded != 0
max_gpu_used_mb >= 7000
avg_gpu_used_mb is persistently near the admission wall without throughput gain
batch_wait_for_ready_ms does not materially improve versus the previous baseline
```

Also record:
- `max_gpu_used_mb`
- `avg_gpu_used_mb`
- `gpu_admission_vram_used_mb`
- `window_chunks_per_second`
- whether the run still collapses after the startup burst

**Step 4: Commit benchmark evidence**

```bash
git add docs/plans/2026-04-23-gpu-first-pull-refill-plan.md
git commit -m "docs: record gpu-first pull refill implementation plan"
```

### Task 7: Update the master refactor plan with the pull-refill tranche

**Files:**
- Modify: `docs/plans/2026-04-22-ist-indexer-reset-and-refactor-implementation-plan.md`

**Step 1: Add a short section**

Document:
- diagnosis: push works, pull refill breaks under graph priority
- new tranche: GPU-first pull refill override
- benchmark success criteria

**Step 2: Run diff hygiene**

Run:

```bash
git diff --check -- docs/plans/2026-04-22-ist-indexer-reset-and-refactor-implementation-plan.md docs/plans/2026-04-23-gpu-first-pull-refill-plan.md
```

Expected: no diff-check errors.

**Step 3: Commit**

```bash
git add docs/plans/2026-04-22-ist-indexer-reset-and-refactor-implementation-plan.md docs/plans/2026-04-23-gpu-first-pull-refill-plan.md
git commit -m "docs: add gpu-first pull refill tranche to master plan"
```
