# Vector GPU Big-Bang Push/Backpressure Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace the current local refill architecture with a single bounded push/backpressure pipeline that continuously feeds prepare and GPU from claimable vector work.

**Architecture:** The migration is a big-bang replacement. The old GPU-worker-owned refill wave is removed. A single upstream producer feeds prepare continuously from claimable vector work, prepare pushes directly into the shared GPU-ready queue, and the GPU worker only consumes with VRAM admission guards. Control decisions stop using aggregated backlog as a proxy for claimable supply.

**Tech Stack:** Rust, Axon core runtime, crossbeam channels, shared queue state, GraphStore-backed file vectorization queues, existing VRAM guards and vector persist pipeline.

---

### Task 1: Freeze the target topology in code-facing documentation

**Files:**
- Reference: `docs/plans/2026-04-24-vector-gpu-big-bang-push-backpressure-design.md`
- Update: `docs/plans/2026-04-24-gpu-push-backpressure-plan.md`

**Step 1: Align the older push/backpressure note with the big-bang decision**

Write a short “superseded by” note at the top of `docs/plans/2026-04-24-gpu-push-backpressure-plan.md` pointing to the new big-bang design.

**Step 2: Verify the documents do not describe dual-mode compatibility**

Run: `rg -n "compat|fallback|dual|hybrid" docs/plans/2026-04-24-vector-gpu-big-bang-push-backpressure-design.md docs/plans/2026-04-24-gpu-push-backpressure-plan.md`

Expected: nothing that reintroduces a compatibility path for the migration itself.

**Step 3: Commit**

```bash
git add docs/plans/2026-04-24-vector-gpu-big-bang-push-backpressure-design.md docs/plans/2026-04-24-gpu-push-backpressure-plan.md
git commit -m "docs: freeze big-bang vector gpu migration target"
```

### Task 2: Separate claimable backlog from aggregate backlog in the vector lane

**Files:**
- Modify: `src/axon-core/src/graph_ingestion.rs`
- Modify: `src/axon-core/src/embedder.rs`
- Test: `src/axon-core/src/graph_ingestion.rs`

**Step 1: Write failing tests for queue-count semantics**

Add tests proving that:
- claimable vector backlog counts only `queued` and `paused_for_interactive_priority`
- inflight vector items are excluded from claimable backlog
- persist/outbox rows are excluded from claimable backlog

**Step 2: Run tests to verify failure**

Run: `cargo test --manifest-path src/axon-core/Cargo.toml claimable_vector_backlog -- --test-threads=1`

Expected: FAIL because no dedicated claimable backlog API exists yet.

**Step 3: Implement a dedicated GraphStore API**

Add a dedicated method such as:
- `fetch_claimable_file_vectorization_queue_count()`

Keep it physically distinct from:
- `fetch_file_vectorization_queue_counts()`
- `fetch_vector_persist_outbox_counts()`

**Step 4: Replace control inputs in the vector worker**

In `embedder.rs`, stop using the current aggregate `file_backlog_depth` as the primary refill signal for the vector feed path. Keep aggregate views only where they are genuinely needed for unrelated coordination.

**Step 5: Run tests to verify pass**

Run: `cargo test --manifest-path src/axon-core/Cargo.toml claimable_vector_backlog -- --test-threads=1`

Expected: PASS.

**Step 6: Commit**

```bash
git add src/axon-core/src/graph_ingestion.rs src/axon-core/src/embedder.rs
git commit -m "refactor: separate claimable vector backlog from aggregate backlog"
```

### Task 3: Remove the local `active_works` refill-wave architecture

**Files:**
- Modify: `src/axon-core/src/embedder.rs`
- Test: `src/axon-core/src/embedder.rs`

**Step 1: Write failing tests around refill ownership**

Add tests proving the GPU worker no longer needs to:
- maintain a growing local reservoir as the primary refill store,
- reconstruct the next wave from local `active_works`,
- treat refill as a one-shot burst.

Focus on control-flow tests, not performance tests.

**Step 2: Run tests to verify failure**

Run: `cargo test --manifest-path src/axon-core/Cargo.toml vector_refill_ownership -- --test-threads=1`

Expected: FAIL because the current loop still owns refill.

**Step 3: Extract a dedicated upstream producer responsibility**

Restructure the vector lane so one control path is responsible for:
- claiming vector work from the claimable queue,
- dispatching prepare requests,
- honoring ready-queue backpressure.

The GPU worker should stop being the main owner of refill reconstruction.

**Step 4: Remove obsolete local-wave logic**

Delete or collapse code paths whose only job was:
- partial top-up into `active_works`,
- wave rebuilding from locally buffered file work,
- refill decisions based on local reservoir depth.

**Step 5: Run tests to verify pass**

Run: `cargo test --manifest-path src/axon-core/Cargo.toml vector_refill_ownership -- --test-threads=1`

Expected: PASS.

**Step 6: Commit**

```bash
git add src/axon-core/src/embedder.rs
git commit -m "refactor: move vector refill ownership out of gpu worker loop"
```

### Task 4: Make prepare a continuously fed stage

**Files:**
- Modify: `src/axon-core/src/embedder.rs`
- Modify: `src/axon-core/src/vector_pipeline.rs`
- Test: `src/axon-core/src/embedder.rs`

**Step 1: Write failing tests for continuous prepare feeding**

Add tests proving:
- when the ready queue is below the low watermark and claimable work exists, prepare dispatch continues,
- prepare dispatch stops only on backpressure or lack of claimable work,
- prepare workers are not dependent on one local refill wave to stay busy.

**Step 2: Run tests to verify failure**

Run: `cargo test --manifest-path src/axon-core/Cargo.toml continuous_prepare_feed -- --test-threads=1`

Expected: FAIL.

**Step 3: Implement continuous dispatch semantics**

Keep the existing shared ready queue, but make the producer repeatedly feed prepare based on:
- claimable work availability,
- inflight prepare limit,
- ready queue watermarks,
- VRAM admission constraints where appropriate.

Do not reintroduce local refill waves.

**Step 4: Run tests to verify pass**

Run: `cargo test --manifest-path src/axon-core/Cargo.toml continuous_prepare_feed -- --test-threads=1`

Expected: PASS.

**Step 5: Commit**

```bash
git add src/axon-core/src/embedder.rs src/axon-core/src/vector_pipeline.rs
git commit -m "refactor: continuously feed prepare from claimable vector work"
```

### Task 5: Reduce the GPU worker to admission + consumption

**Files:**
- Modify: `src/axon-core/src/embedder.rs`
- Test: `src/axon-core/src/embedder.rs`

**Step 1: Write failing tests for GPU worker responsibilities**

Add tests proving the GPU worker:
- consumes from the shared ready queue,
- obeys VRAM admission,
- dispatches persist,
- does not own the main refill orchestration.

**Step 2: Run tests to verify failure**

Run: `cargo test --manifest-path src/axon-core/Cargo.toml gpu_worker_consumption_only -- --test-threads=1`

Expected: FAIL.

**Step 3: Simplify the worker loop**

Collapse the worker logic so the hot loop becomes:
- check admission,
- pop ready,
- embed,
- persist,
- repeat.

Preserve:
- claim lease correctness,
- error paths,
- restart handling,
- interactive guards if still required.

**Step 4: Run tests to verify pass**

Run: `cargo test --manifest-path src/axon-core/Cargo.toml gpu_worker_consumption_only -- --test-threads=1`

Expected: PASS.

**Step 5: Commit**

```bash
git add src/axon-core/src/embedder.rs
git commit -m "refactor: reduce gpu worker to admission and consumption"
```

### Task 6: Remove obsolete scheduler coupling from vector feed control

**Files:**
- Modify: `src/axon-core/src/vector_control.rs`
- Modify: `src/axon-core/src/embedder.rs`
- Test: `src/axon-core/src/embedder.rs`

**Step 1: Write failing tests for direct backpressure control**

Add tests proving:
- vector feed is controlled by claimable supply plus ready-queue watermarks,
- graph backlog does not directly suppress vector refill while ready supply is low,
- scheduler state is no longer the primary refill mechanism.

**Step 2: Run tests to verify failure**

Run: `cargo test --manifest-path src/axon-core/Cargo.toml vector_feed_backpressure_control -- --test-threads=1`

Expected: FAIL.

**Step 3: Remove or neutralize obsolete control paths**

Delete or downgrade logic whose main purpose was to emulate refill by:
- `graph_priority`,
- `semantic_underfed`,
- indirect scheduler holds,
- aggregate backlog heuristics used as refill authority.

Keep only what remains necessary for unrelated safety mechanisms.

**Step 4: Run tests to verify pass**

Run: `cargo test --manifest-path src/axon-core/Cargo.toml vector_feed_backpressure_control -- --test-threads=1`

Expected: PASS.

**Step 5: Commit**

```bash
git add src/axon-core/src/vector_control.rs src/axon-core/src/embedder.rs
git commit -m "refactor: drive vector feed from backpressure instead of scheduler priority"
```

### Task 7: Preserve lease and failure correctness through the big-bang rewrite

**Files:**
- Modify: `src/axon-core/src/embedder.rs`
- Modify: `src/axon-core/src/vector_pipeline.rs`
- Modify: `src/axon-core/src/graph_ingestion.rs`
- Test: `src/axon-core/src/embedder.rs`
- Test: `src/axon-core/src/graph_ingestion.rs`

**Step 1: Write failing correctness tests**

Cover:
- no work item is lost when prepare fails,
- no work item is duplicated across ready/persist/finalize,
- paused interactive work still round-trips safely,
- persist failure does not strand claims permanently.

**Step 2: Run tests to verify failure**

Run: `cargo test --manifest-path src/axon-core/Cargo.toml vector_claim_safety -- --test-threads=1`

Expected: FAIL.

**Step 3: Implement minimal correctness fixes**

Fix only claim/lease correctness gaps introduced by the new topology. Do not add new tuning logic here.

**Step 4: Run tests to verify pass**

Run: `cargo test --manifest-path src/axon-core/Cargo.toml vector_claim_safety -- --test-threads=1`

Expected: PASS.

**Step 5: Commit**

```bash
git add src/axon-core/src/embedder.rs src/axon-core/src/vector_pipeline.rs src/axon-core/src/graph_ingestion.rs
git commit -m "fix: preserve claim and lease correctness in big-bang vector flow"
```

### Task 8: Requalify the new topology against the current safe baseline

**Files:**
- Reference: `scripts/benchmark-vector-token-matrix.sh`
- Reference: `.axon/benchmarks/20260423T203226Z-oven-vram-cap-7000-admission-6300-safe-scan/results.tsv`

**Step 1: Run targeted tests first**

Run:
- `cargo test --manifest-path src/axon-core/Cargo.toml -- --test-threads=1`

Expected: PASS for the touched areas.

**Step 2: Run a VRAM-safe qualification benchmark**

Run the existing benchmark flow with:
- `tokens=16000`
- `ready-depth=96`
- `pipeline-depth=12`
- `prepare-workers=8`
- VRAM budget `7000`
- GPU admission `6300`

Expected:
- `vram_budget_exceeded = 0`
- no structural collapse to long starvation after the initial burst

**Step 3: Compare against safe baseline**

Baseline file:
- `/.axon/benchmarks/20260423T203226Z-oven-vram-cap-7000-admission-6300-safe-scan/results.tsv`

Acceptance:
- sustained throughput clearly exceeds the prior valid baseline,
- the run no longer shows the old burst-then-famine pattern.

**Step 4: Commit**

```bash
git add src/axon-core/src/embedder.rs src/axon-core/src/vector_control.rs src/axon-core/src/vector_pipeline.rs src/axon-core/src/graph_ingestion.rs docs/plans/2026-04-24-vector-gpu-big-bang-push-backpressure-design.md docs/plans/2026-04-24-vector-gpu-big-bang-push-backpressure-implementation-plan.md
git commit -m "feat: replace vector refill waves with big-bang push backpressure pipeline"
```

Plan complete and saved to `docs/plans/2026-04-24-vector-gpu-big-bang-push-backpressure-implementation-plan.md`. Two execution options:

1. Subagent-Driven (this session) - I dispatch fresh subagent per task, review between tasks, fast iteration
2. Parallel Session (separate) - Open new session with executing-plans, batch execution with checkpoints
