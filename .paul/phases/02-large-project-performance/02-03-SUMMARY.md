---
phase: 02-large-project-performance
plan: 03
subsystem: core/ingestion
tags: [parallel-parsing, threading, performance, test-coverage]

requires:
  - phase: 02-02-incremental-indexing
    provides: Established that parallel parsing was already in place (commit 8e71d2b); incremental path provides context

provides:
  - CPU-adaptive max_workers default (None) in walk_repo() and process_parsing()
  - TestParallelParsing class (2 tests): serial≡parallel correctness + order determinism

affects: [any caller of walk_repo() or process_parsing() relying on max_workers default]

tech-stack:
  added: []
  patterns:
    - "max_workers=None → ThreadPoolExecutor default (min(32, cpu_count+4)) — let stdlib scale"

key-files:
  created: []
  modified:
    - src/axon/core/ingestion/walker.py
    - src/axon/core/ingestion/parser_phase.py
    - tests/core/test_parser_phase.py

key-decisions:
  - "Parallel parsing was already implemented in Phase 1 (8e71d2b) — plan re-scoped from 'implement' to 'tune + test'"
  - "max_workers: int = 8 → int | None = None: pass None to ThreadPoolExecutor, not os.cpu_count() in app code"

patterns-established:
  - "Use max_workers=None for ThreadPoolExecutor — let Python pick min(32, cpu_count+4), not a hardcoded cap"

duration: ~5min
started: 2026-02-26T00:00:00Z
completed: 2026-02-26T00:00:00Z
---

# Phase 2 Plan 03: Adaptive Workers & Parallel Correctness Summary

**Replaced hardcoded `max_workers=8` with CPU-adaptive `None` default in `walk_repo()` and `process_parsing()`; added `TestParallelParsing` (2 tests) verifying serial≡parallel output and order determinism; 652 tests passing.**

## Discovery

Parallel parsing was already implemented in commit `8e71d2b` (Phase 1 language coverage — prior to Phase 2 planning). ROADMAP item 02-03 "Parallel parsing worker pool" was written without that knowledge. Plan was re-scoped accordingly: tune the existing implementation and add correctness tests.

## Performance

| Metric | Value |
|--------|-------|
| Duration | ~5 min |
| Tasks | 2 completed |
| Files modified | 3 |
| New test count | +2 |

## Acceptance Criteria Results

| Criterion | Status | Notes |
|-----------|--------|-------|
| AC-1: Adaptive worker count | Pass | `max_workers: int | None = None` in both files; ThreadPoolExecutor picks min(32, cpu_count+4) |
| AC-2: Parallel correctness — serial vs parallel | Pass | `test_serial_and_parallel_produce_identical_graphs` passes |
| AC-3: Parallel correctness — determinism | Pass | `test_parse_order_is_deterministic` passes |
| AC-4: No regression | Pass | 652/652 tests pass |

## Accomplishments

- Removed hardcoded `max_workers=8` cap from `walker.py` and `parser_phase.py`; both now scale with available CPUs via Python's ThreadPoolExecutor default
- Added `TestParallelParsing` class to `test_parser_phase.py` — first test coverage for the parallel parse path
- Confirmed via test: `executor.map()` preserves input order, so parse results are always aligned with the file list regardless of thread scheduling

## Files Created/Modified

| File | Change | Purpose |
|------|--------|---------|
| `src/axon/core/ingestion/walker.py` | Modified | `max_workers: int = 8` → `int | None = None`; updated docstring |
| `src/axon/core/ingestion/parser_phase.py` | Modified | Same signature change; updated docstring |
| `tests/core/test_parser_phase.py` | Modified | Added `TestParallelParsing` (2 tests) + `_make_graph_with_file_nodes` helper |

## Decisions Made

| Decision | Rationale | Impact |
|----------|-----------|--------|
| Pass `None` to ThreadPoolExecutor, not `os.cpu_count()` | stdlib handles the formula; avoids duplicating logic in app code | Cleaner; picks up stdlib improvements automatically |
| Plan re-scoped: tune + test, not implement | Parallel parsing already exists; implementing again = waste | Focused on real gaps (tuning + correctness coverage) |

## Deviations from Plan

| Type | Count | Impact |
|------|-------|--------|
| Scope reduction | 1 | Discovery that 02-03 work was already partially done → re-scope |
| Auto-fixed | 0 | — |
| Deferred | 0 | — |

**Total impact:** Plan re-scoped based on accurate codebase state. All AC satisfied. No scope creep.

## Issues Encountered

None.

## Phase 2 End-State

Phase 2 is now complete. Three plans executed:

| Plan | Feature | Impact |
|------|---------|--------|
| 02-01 | Benchmark baseline | Identified walk(36%) + parse(35%) as bottlenecks |
| 02-02 | Incremental indexing | Warm-start cut from ~0.89s to ~8ms |
| 02-03 | Adaptive workers + parallel tests | Cold-start scales with CPU count; parallel path tested |

**Cold-start profile (85 files, axon repo):**
- `walk_repo()`: parallel file reading, CPU-adaptive workers
- `process_parsing()`: parallel parse, CPU-adaptive workers, sequential graph merge
- Community detection: 0.17s (19%) — global operation, inherently sequential

## Next Phase Readiness

**Ready:**
- Performance foundation complete for Phase 3 (Workflow Integration)
- Incremental indexing live and tested; warm starts at ~8ms
- Thread pool scales with hardware for both walk and parse phases

**Concerns:**
- Community detection (19% of cold-start) remains sequential; at 100k+ LOC this may become the new bottleneck — but acceptable for Phase 3 scope

**Blockers:** None

---
*Phase: 02-large-project-performance, Plan: 03*
*Completed: 2026-02-26*
