---
phase: 01-consolidation-and-scale
plan: 01
subsystem: storage, pipeline
tags: [kuzudb, batch-insert, csv-copy, async-embeddings, profiling, threadpool]

requires:
  - phase: v0.3 (all phases)
    provides: incremental indexing, parallel parsing, shell/CI integration
provides:
  - Batch CSV COPY FROM for incremental add_nodes/add_relationships
  - Async embedding generation via ThreadPoolExecutor
  - Performance profiling baseline for 3 largest repos
affects: [01-02 code quality, future perf optimization]

tech-stack:
  added: []
  patterns: [CSV COPY FROM for batch inserts, ThreadPoolExecutor for async embeddings]

key-files:
  created:
    - .paul/phases/01-consolidation-and-scale/profiling-baseline.md
  modified:
    - src/axon/core/storage/kuzu_backend.py
    - src/axon/core/ingestion/pipeline.py
    - src/axon/cli/main.py
    - tests/core/test_kuzu_backend.py
    - tests/core/test_pipeline.py

key-decisions:
  - "storage_load is 98%+ of indexing time — future perf work must target KuzuDB bulk_load"
  - "Async embeddings via ThreadPoolExecutor (1 thread, default non-blocking)"

patterns-established:
  - "Batch inserts: group by table, CSV COPY FROM, fallback to individual on failure"
  - "Async background work: ThreadPoolExecutor + Future on result object"

duration: ~1 session
started: 2026-02-27
completed: 2026-02-27
---

# Phase 1 Plan 01: Performance Optimization Summary

**Batch CSV COPY FROM for incremental inserts, async embedding generation, and profiling baseline for 3 largest repos — storage_load identified as 98%+ bottleneck.**

## Performance

| Metric | Value |
|--------|-------|
| Duration | 1 session |
| Started | 2026-02-27 |
| Completed | 2026-02-27 |
| Tasks | 3 completed |
| Files modified | 5 source + 1 doc |

## Acceptance Criteria Results

| Criterion | Status | Notes |
|-----------|--------|-------|
| AC-1: Batch node inserts use CSV COPY FROM | Pass | `add_nodes` groups by table, uses `_csv_copy`, fallback to individual inserts |
| AC-2: Batch relationship inserts use CSV COPY FROM | Pass | `add_relationships` groups by (src_table, dst_table), same pattern |
| AC-3: Embeddings can run asynchronously | Pass | `_run_embeddings` + `_EMBEDDING_POOL`, `wait_embeddings` param, `embedding_future` on result |
| AC-4: Profiling baseline established | Pass | 3 repos profiled, per-phase timings, baseline doc created |
| AC-5: All existing tests pass | Pass | 687 passed (678 original + 9 new), 0 failures |

## Accomplishments

- Batch inserts for incremental path: `add_nodes` and `add_relationships` now use CSV COPY FROM with automatic fallback to individual inserts on failure
- Async embedding pipeline: `run_pipeline` returns immediately by default, embeddings compute in background via `ThreadPoolExecutor`; CLI uses `wait_embeddings=True` for display counts
- Profiling baseline: storage_load is 98%+ of total indexing time across all 3 large repos (machineflow 477s, flow_analyzer 589s, BookingSystem 535s) — all other pipeline phases combined < 15 seconds

## Task Commits

| Task | Commit | Type | Description |
|------|--------|------|-------------|
| All 3 tasks | `b0096c5` | perf | batch inserts, async embeddings, profiling baseline |

## Files Created/Modified

| File | Change | Purpose |
|------|--------|---------|
| `src/axon/core/storage/kuzu_backend.py` | Modified | Batch `add_nodes`/`add_relationships` via CSV COPY FROM with fallback |
| `src/axon/core/ingestion/pipeline.py` | Modified | `_run_embeddings`, `_EMBEDDING_POOL`, `wait_embeddings` param, `PipelineResult.embedding_future` |
| `src/axon/cli/main.py` | Modified | `wait_embeddings=True` for CLI analyze command |
| `tests/core/test_kuzu_backend.py` | Modified | +7 tests (batch nodes, batch rels, empty, mixed labels, fallback) |
| `tests/core/test_pipeline.py` | Modified | +2 tests (async future exists, blocking mode) |
| `.paul/phases/01-consolidation-and-scale/profiling-baseline.md` | Created | Per-phase timing data for 3 largest repos |

## Decisions Made

| Decision | Rationale | Impact |
|----------|-----------|--------|
| storage_load is the sole perf target | 98%+ of indexing time across all repos; pipeline phases are negligible | Future perf work must target KuzuDB bulk_load, not pipeline phases |
| Async embeddings via ThreadPoolExecutor (1 thread) | Embeddings are I/O-bound (model inference), single thread sufficient | Pipeline returns immediately; callers optionally wait via future |
| CLI uses wait_embeddings=True | CLI needs final embedding counts for display output | No user-facing behavior change |

## Deviations from Plan

None — plan executed exactly as written.

## Issues Encountered

None.

## Next Phase Readiness

**Ready:**
- Batch inserts available for incremental path
- Async embeddings decouple index availability from vector computation
- Profiling baseline establishes measurement foundation

**Concerns:**
- storage_load bottleneck is inside KuzuDB COPY FROM — may require schema changes (REL TABLE GROUP cartesian product) rather than application-level fixes

**Blockers:**
None.

---
*Phase: 01-consolidation-and-scale, Plan: 01*
*Completed: 2026-02-27*
