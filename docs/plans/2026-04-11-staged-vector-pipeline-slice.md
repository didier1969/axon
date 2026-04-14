# Staged Vector Pipeline Slice Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Extract explicit `prepare -> embed -> persist` boundaries from the current vector worker loop without changing runtime concurrency semantics.

**Architecture:** Keep the current vector worker and controller intact, but refactor the monolithic loop into staged data structures and helper functions. This creates a safe seam for the later decoupled pipeline while preserving MCP protection, queue semantics, and current batching behavior.

**Tech Stack:** Rust, Axon core runtime, DuckDB-backed `GraphStore`, existing `service_guard` telemetry, existing vector batch controller.

---

### Task 1: Freeze the current vector-cycle contract in tests

**Files:**
- Modify: `src/axon-core/src/embedder.rs`

**Step 1: Write failing tests**

Add tests for:
- converting a `VectorBatchPlan` into a prepared embed batch with preserved work lists
- converting a prepared batch plus embeddings into a persist plan with zipped updates
- rejecting mismatched embedding counts

**Step 2: Run tests to verify they fail**

Run:

```bash
cargo test test_prepared_vector_embed_batch_ -- --nocapture
```

Expected:
- missing types/functions or assertion failures

**Step 3: Write minimal implementation**

Implement:
- `PreparedVectorEmbedBatch`
- `VectorPersistPlan`
- helpers to build texts, zip embeddings, and preserve work transitions

**Step 4: Run tests to verify they pass**

Run:

```bash
cargo test test_prepared_vector_embed_batch_ -- --nocapture
```

Expected:
- PASS

### Task 2: Refactor the vector worker loop to use explicit stage helpers

**Files:**
- Modify: `src/axon-core/src/embedder.rs`

**Step 1: Write failing test or strengthen assertions**

If needed, extend tests to assert:
- success path preserves `finalize_after_success`
- failure path preserves `next_active_after_failure`

**Step 2: Implement minimal refactor**

Replace inline loop logic with:
- prepare step: build `PreparedVectorEmbedBatch`
- embed step: `model.embed(prepared.texts, None)`
- persist step: convert to `VectorPersistPlan`, then write embeddings

Do not:
- change queue admission
- change worker counts
- introduce new threads or channels

**Step 3: Run targeted tests**

Run:

```bash
cargo test test_vector_batch_plan_advances_multiple_files_and_tracks_partial_cycles -- --nocapture
cargo test test_prepared_vector_embed_batch_ -- --nocapture
```

Expected:
- PASS

### Task 3: Keep observability and MCP behavior stable

**Files:**
- Modify: `src/axon-core/src/embedder.rs`
- Test: `src/axon-core/src/mcp/tests.rs`

**Step 1: Verify telemetry still increments in the same semantic places**

Ensure:
- `Fetch` timing still belongs to batch-plan preparation
- `Embed` timing still wraps only `model.embed()`
- `DB write` still wraps `update_chunk_embeddings`
- `Mark done` still wraps `mark_file_vectorization_done`

**Step 2: Re-run non-regression tests**

Run:

```bash
cargo test test_axon_debug_reports_backlog_memory_and_storage_views -- --nocapture
cargo test test_retrieve_context_ -- --nocapture
```

Expected:
- PASS

### Task 4: Close the slice with verified outcomes

**Files:**
- No new files required beyond the plan

**Step 1: Summarize what is now explicit**

Capture:
- what was extracted
- what remains monolithic
- what becomes possible next

**Step 2: Stop before concurrency redesign**

Do not yet:
- add internal channels
- spawn separate prepare/persist workers
- make worker counts adaptive

This slice is complete only when:
- staged boundaries are explicit in code
- tests prove no semantic regression
- observability remains intact
- the code is easier to evolve into a bounded staged pipeline next
