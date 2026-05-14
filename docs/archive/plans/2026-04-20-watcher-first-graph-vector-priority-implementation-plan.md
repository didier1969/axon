# Watcher-First Graph-Then-Vector Priority Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Make Axon rebuild its workbase aggressively by prioritizing `watcher -> queue -> graph -> vector`, so every newly identified file becomes durably actionable without waiting for later repair passes, graphing remains dominant with fairness, and CPU-side preparation feeds the GPU instead of starving it.

**Architecture:** Introduce an explicit watcher-first scheduling contract instead of relying on implicit queue interactions. The runtime should treat file identification as the dominant admission signal, promote graph work before semantic work with hysteresis and a semantic floor, and only let vectorization consume files that have already become graph-ready, while CPU-side preparation maintains a healthy pre-GPU reserve and respects downstream persist pressure.

**Tech Stack:** Rust (`axon-core`), Cozo-backed ingestion state, runtime tuning / service guard metrics, MCP `status`, Python qualification scripts.

---

### Task 0: Define the canonical stage and ownership model first

**Files:**
- Modify: `src/axon-core/src/mcp/tools_framework.rs`
- Modify: `src/axon-core/src/graph_ingestion.rs`
- Modify: `docs/operations/2026-04-18-live-dev-runtime-operations.md`
- Test: `src/axon-core/src/mcp/tests.rs`

**Step 1: Write the failing test**

Add a status-level test asserting runtime truth distinguishes these stages and ownership surfaces:
- watcher buffered / staged
- persisted `File`
- graph work actionable
- `graph_ready`
- `FileVectorizationQueue` owned
- `vector_ready` or explicitly excluded

**Step 2: Run test to verify it fails**

Run: `cargo test --manifest-path src/axon-core/Cargo.toml test_status_reports_canonical_ingestion_stage_model -- --test-threads=1`

Expected: FAIL because the current truth is still too compressed.

**Step 3: Implement the minimal stage model truth**

Expose the canonical state/ownership model in status and operational notes so later tasks refine real surfaces rather than inventing a new abstract pipeline.

**Step 4: Run test to verify it passes**

Run: `cargo test --manifest-path src/axon-core/Cargo.toml test_status_reports_canonical_ingestion_stage_model -- --test-threads=1`

Expected: PASS.

**Step 5: Commit**

```bash
git add src/axon-core/src/mcp/tools_framework.rs src/axon-core/src/graph_ingestion.rs docs/operations/2026-04-18-live-dev-runtime-operations.md src/axon-core/src/mcp/tests.rs
git commit -m "feat: expose canonical ingestion stage model"
```

### Task 1: Define the priority contract in runtime truth

**Files:**
- Modify: `src/axon-core/src/mcp/tools_framework.rs`
- Modify: `src/axon-core/src/main_background.rs`
- Modify: `src/axon-core/src/runtime_profile.rs`
- Test: `src/axon-core/src/mcp/tests.rs`

**Step 1: Write the failing test**

Add a status assertion proving runtime truth exposes a priority contract like:
- `watcher_identification = highest`
- `graphing_after_enqueue = second`
- `vectorization_after_graph_ready = third`

**Step 2: Run test to verify it fails**

Run: `cargo test --manifest-path src/axon-core/Cargo.toml test_status_reports_priority_contract_for_watcher_first_pipeline -- --test-threads=1`

Expected: FAIL because the contract is not yet exposed.

**Step 3: Implement the minimal runtime truth**

Add a structured section under `runtime_authority` or adjacent status truth that exposes:
- the canonical pipeline order
- whether each lane is currently backlog-gated
- whether vectorization is allowed to advance ahead of graph backlog

**Step 4: Run test to verify it passes**

Run: `cargo test --manifest-path src/axon-core/Cargo.toml test_status_reports_priority_contract_for_watcher_first_pipeline -- --test-threads=1`

Expected: PASS.

**Step 5: Commit**

```bash
git add src/axon-core/src/mcp/tools_framework.rs src/axon-core/src/main_background.rs src/axon-core/src/runtime_profile.rs src/axon-core/src/mcp/tests.rs
git commit -m "feat: expose watcher-first priority contract"
```

### Task 2: Make newly identified files durably actionable without waiting for repair passes

**Files:**
- Modify: `src/axon-core/src/graph_ingestion.rs`
- Modify: `src/axon-core/src/scanner.rs`
- Modify: `src/axon-core/src/ingress_buffer.rs`
- Test: `src/axon-core/src/mcp/tests.rs`

**Step 1: Write the failing test**

Add a test that upserts a newly identified file and asserts:
- it lands in `File`
- it becomes durably actionable for graph work on the hot path
- it is not dependent on a later backfill or reconciliation pass as its primary path to actionability

**Step 2: Run test to verify it fails**

Run: `cargo test --manifest-path src/axon-core/Cargo.toml test_newly_identified_file_is_enqueued_immediately_for_graph_pipeline -- --test-threads=1`

Expected: FAIL because current behavior still depends too much on later reconciliation or indirect queue repair.

**Step 3: Implement the minimal enqueue-first behavior**

Change ingress promotion / hot upsert behavior so that a newly identified eligible file:
- gets persisted immediately
- is admitted to graph work durably on the hot path
- records an explicit reason/source for this admission (`watcher_hot_identified`, `scan_identified`, or equivalent)

Preserve buffering/coalescing and subtree hint behavior; do not turn watcher churn into synchronous graph thrash.

**Step 4: Run test to verify it passes**

Run: `cargo test --manifest-path src/axon-core/Cargo.toml test_newly_identified_file_is_enqueued_immediately_for_graph_pipeline -- --test-threads=1`

Expected: PASS.

**Step 5: Commit**

```bash
git add src/axon-core/src/graph_ingestion.rs src/axon-core/src/scanner.rs src/axon-core/src/ingress_buffer.rs src/axon-core/src/mcp/tests.rs
git commit -m "feat: enqueue identified files immediately"
```

### Task 3: Make graph work dominate semantic admission with fairness and hysteresis

**Files:**
- Modify: `src/axon-core/src/graph_ingestion.rs`
- Modify: `src/axon-core/src/main_background.rs`
- Modify: `src/axon-core/src/service_guard.rs`
- Modify: `src/axon-core/src/optimizer.rs`
- Test: `src/axon-core/src/mcp/tests.rs`

**Step 1: Write the failing test**

Add a test showing that when both graph backlog and semantic backlog are present:
- graph admission remains dominant
- semantic/vector lanes retain a minimum reserve floor instead of being starved completely
- state transitions between graph-first and semantic-refill are bounded by hysteresis

**Step 2: Run test to verify it fails**

Run: `cargo test --manifest-path src/axon-core/Cargo.toml test_graph_backlog_blocks_vector_priority_until_graph_ready_advances -- --test-threads=1`

Expected: FAIL because current scheduling still allows semantic pressure to obscure graph-first intent.

**Step 3: Implement the minimal dominance rule**

Adjust the scheduler / service guard / optimizer interaction so that:
- graph backlog age/debt is treated as the dominant signal
- graph workers and graph queue promotion are not throttled behind semantic drain
- semantic/vector lanes retain a minimum reserve floor while backlog exists
- entry/exit from graph-priority mode uses explicit thresholds and hold windows instead of sample-to-sample flips

**Step 4: Run test to verify it passes**

Run: `cargo test --manifest-path src/axon-core/Cargo.toml test_graph_backlog_blocks_vector_priority_until_graph_ready_advances -- --test-threads=1`

Expected: PASS.

**Step 5: Commit**

```bash
git add src/axon-core/src/graph_ingestion.rs src/axon-core/src/main_background.rs src/axon-core/src/service_guard.rs src/axon-core/src/optimizer.rs src/axon-core/src/mcp/tests.rs
git commit -m "feat: prioritize graph backlog ahead of vector drain"
```

### Task 4: Gate vectorization strictly behind graph-ready truth

**Files:**
- Modify: `src/axon-core/src/graph_ingestion.rs`
- Modify: `src/axon-core/src/vector_control.rs`
- Modify: `src/axon-core/src/embedder.rs`
- Test: `src/axon-core/src/mcp/tests.rs`

**Step 1: Write the failing test**

Add a test asserting that a file may only enter semantic/vector work when:
- it is graph-ready
- it is not deleted/skipped/oversized
- there is actual embedding work left for its chunks
- repair paths like `resume_vectorization` still work

**Step 2: Run test to verify it fails**

Run: `cargo test --manifest-path src/axon-core/Cargo.toml test_vectorization_admits_only_graph_ready_files -- --test-threads=1`

Expected: FAIL.

**Step 3: Implement the minimal graph-ready gate**

Tighten queue admission and claims so that vectorization becomes purely downstream:
- no semantic work before graph-ready
- no hidden backdoor through orphan recovery as primary flow
- queue repair remains fallback only
- excluded/non-vectorizable states stay explicit and non-duplicative

**Step 4: Run test to verify it passes**

Run: `cargo test --manifest-path src/axon-core/Cargo.toml test_vectorization_admits_only_graph_ready_files -- --test-threads=1`

Expected: PASS.

**Step 5: Commit**

```bash
git add src/axon-core/src/graph_ingestion.rs src/axon-core/src/vector_control.rs src/axon-core/src/embedder.rs src/axon-core/src/mcp/tests.rs
git commit -m "feat: gate vectorization behind graph-ready truth"
```

### Task 5: Increase CPU-side preparation to feed the GPU without shifting congestion downstream

**Files:**
- Modify: `src/axon-core/src/vector_control.rs`
- Modify: `src/axon-core/src/optimizer.rs`
- Modify: `src/axon-core/src/service_guard.rs`
- Modify: `scripts/qualify_runtime.py`
- Test: `src/axon-core/src/mcp/tests.rs`

**Step 1: Write the failing test**

Add a test that simulates:
- GPU headroom available
- RAM headroom available
- ready queue too thin
- graph backlog already within target
- persist queue not congested
- graph backlog already under control

and asserts the controller prefers:
- deeper `prepare_inflight`
- higher `ready reserve`
- better micro-batch composition / file grouping
- not more GPU sessions

**Step 2: Run test to verify it fails**

Run: `cargo test --manifest-path src/axon-core/Cargo.toml test_underfed_gpu_prefers_cpu_prepare_buffer_growth_after_graph_backlog_clears -- --test-threads=1`

Expected: FAIL.

**Step 3: Implement the minimal underfeed response**

Adjust controller heuristics so that when:
- graph backlog is not the blocker
- vector lane is starved
- CPU and RAM have room
- persist pressure is not already elevated

the runtime grows CPU-side buffer preparation first:
- `prepare_inflight`
- `ready reserve target`
- micro-batch sizing
- file grouping

without reintroducing VRAM pressure.

**Step 4: Run test to verify it passes**

Run: `cargo test --manifest-path src/axon-core/Cargo.toml test_underfed_gpu_prefers_cpu_prepare_buffer_growth_after_graph_backlog_clears -- --test-threads=1`

Expected: PASS.

**Step 5: Commit**

```bash
git add src/axon-core/src/vector_control.rs src/axon-core/src/optimizer.rs src/axon-core/src/service_guard.rs scripts/qualify_runtime.py src/axon-core/src/mcp/tests.rs
git commit -m "feat: grow cpu-side buffer before gpu scaling"
```

### Task 6: Qualify the pipeline end to end on dev first

**Files:**
- Modify: `scripts/qualify_runtime.py`
- Modify: `scripts/mcp_validate.py`
- Modify: `docs/operations/2026-04-18-live-dev-runtime-operations.md`
- Test: runtime qualification artifacts in `.axon/qualification-suite-runs/`

**Step 1: Add the qualification checkpoints**

Ensure qualification captures:
- watcher identification rate
- graph queue growth / drain
- semantic queue growth / drain
- whether files are making the transition `identified -> graph_ready -> vector_ready`
- `graph_ready` p50/p95 after identification
- `ready_queue_depth_current == 0` dwell time under backlog
- `gpu_idle_wait_ms_total` delta
- `prepare_queue_wait_ms_total`
- `persist_queue_wait_ms_total`
- semantic completion rate while graph ingress remains active

**Step 2: Run qualification on dev**

Run: `python3 scripts/qualify_runtime.py --instance dev --label watcher-first-graph-vector`

Expected:
- watcher/backlog truth visible
- graph backlog appears before vector backlog
- vector backlog is downstream, not the first admission surface
- graph-first does not collapse semantic throughput into starvation

**Step 3: Fix any failing invariant**

If qualification shows:
- watcher not filling the base fast enough
- graph not taking priority
- vectorization bypassing graph dominance

return to the corresponding task and fix the invariant.

**Step 4: Re-run qualification**

Run the same command until the pipeline order is visible and defensible.

**Step 5: Commit**

```bash
git add scripts/qualify_runtime.py scripts/mcp_validate.py docs/operations/2026-04-18-live-dev-runtime-operations.md
git commit -m "test: qualify watcher-first graph-vector pipeline on dev"
```

### Task 7: Validate cold-start and recovery before live

**Files:**
- Modify: `scripts/qualify_runtime.py`
- Modify: `docs/operations/2026-04-18-live-dev-runtime-operations.md`
- Test: runtime qualification artifacts in `.axon/qualification-suite-runs/`

**Step 1: Add cold/recovery checks**

Ensure qualification explicitly covers:
- cold rebuild after empty or purged IST
- missed watcher event recovery
- restart recovery where scanner/reconciliation must still preserve correctness

**Step 2: Run recovery qualification on dev**

Run: `python3 scripts/qualify_runtime.py --instance dev --label watcher-first-recovery`

Expected:
- watcher-first hot path is visible
- scanner/reconciliation still repair correctness after cold-start or restart

**Step 3: Fix any failing invariant**

If qualification shows recovery regressions, fix them before live promotion.

**Step 4: Re-run qualification**

Repeat until both hot-path priority and cold/recovery correctness are defensible.

**Step 5: Commit**

```bash
git add scripts/qualify_runtime.py docs/operations/2026-04-18-live-dev-runtime-operations.md
git commit -m "test: qualify watcher-first cold-start and recovery"
```

### Task 8: Promote to live only after dev proof

**Files:**
- No code change required unless qualification exposes a final issue
- Operational references: `scripts/axon`, `scripts/start-live.sh`, `scripts/status-live.sh`

**Step 1: Re-run short validation on main/dev**

Run:
- `cargo test --manifest-path src/axon-core/Cargo.toml test_status_reports_public_surface_and_runtime_truth -- --test-threads=1`
- `python3 scripts/mcp_validate.py --url http://127.0.0.1:44139/mcp --surface core --project AXO --timeout 30`

Expected: PASS.

**Step 2: Promote to live**

Run: `./scripts/axon promote-live-safe --project AXO`

Expected: promotion completes without manual recovery.

**Step 3: Validate live**

Run:
- `bash scripts/status-live.sh`
- `python3 scripts/mcp_validate.py --url http://127.0.0.1:44129/mcp --surface core --project AXO --timeout 30`

Expected:
- `HEALTHY`
- MCP quality gate passes
- watcher/graph/vector ordering remains visible in `status(mode="full")`

**Step 4: Record residual risk**

Document any remaining caveat such as:
- watcher cold-start still slower than desired
- graph queue can still be obscured under rare interactive storms
- GPU remains underfed in specific workloads

**Step 5: Commit**

```bash
git add relevant-doc-updates
git commit -m "docs: record watcher-first live qualification"
```

### Expert Review Gate

Before execution starts, require two independent plan reviews with explicit verdicts:
- Runtime scheduling / ingestion reviewer
- GPU / vector pipeline reviewer

Allowed verdicts only:
- `approved`
- `approved_with_reservations`
- `needs_reframe`
- `blocked`

If either reviewer returns `needs_reframe` or `blocked`, revise the plan before implementation.
