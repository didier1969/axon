---
phase: 02-large-project-performance
plan: 02
subsystem: core/ingestion
tags: [incremental-indexing, performance, pipeline, content-hash, delta]

requires:
  - phase: 02-01-benchmark-baseline
    provides: PhaseTimings on PipelineResult; baseline showing walk(36%) + parse(35%) as dominant phases

provides:
  - Incremental fast-path in run_pipeline(): skips re-parsing unchanged files on warm starts
  - result.incremental flag and result.changed_files counter on PipelineResult
  - TestIncrementalPipeline test class (5 tests)

affects: [02-03-parallel-parsing, any caller of run_pipeline()]

tech-stack:
  added: [hashlib (stdlib, no new deps)]
  patterns:
    - "Content-hash manifest via storage.get_indexed_files() — sha256(content) diff drives incremental decision"
    - "Early return from run_pipeline() on incremental path — returns (partial_graph, result)"

key-files:
  created: []
  modified:
    - src/axon/core/ingestion/pipeline.py
    - tests/core/test_pipeline.py
    - tests/e2e/test_full_pipeline.py

key-decisions:
  - "Use content hash (sha256) not mtime — more reliable; walk_repo() already reads content anyway"
  - "Skip global phases (community detection etc.) on incremental path — consistent with watcher behaviour"
  - "result.symbols/relationships left at 0 on incremental path — counts are meaningless for a partial run"
  - "Inline logic in run_pipeline(), no helper function — 37 lines, no abstraction needed"

patterns-established:
  - "Incremental activation: storage is not None AND not full AND manifest non-empty"
  - "Deletions: remove_nodes_by_file() for paths in manifest but not on disk"
  - "Changes/additions: reindex_files() for files whose hash differs from manifest"

duration: ~12min
started: 2026-02-26T00:00:00Z
completed: 2026-02-26T00:00:00Z
---

# Phase 2 Plan 02: Incremental Indexing Summary

**Content-hash delta in `run_pipeline()` skips walk+parse for unchanged files on warm starts, turning O(all files) re-indexes into O(changed files); 650 tests passing.**

## Performance

| Metric | Value |
|--------|-------|
| Duration | ~12 min |
| Tasks | 2 completed |
| Files modified | 3 |
| New lines (pipeline.py) | +37 |
| New lines (test_pipeline.py) | +78 |

## Acceptance Criteria Results

| Criterion | Status | Notes |
|-----------|--------|-------|
| AC-1: Unchanged files skipped | Pass | `result.incremental=True`, `changed_files=0` on no-change run |
| AC-2: Changed file re-indexed | Pass | `changed_files=1`; updated symbol queryable from storage |
| AC-3: New file indexed | Pass | `changed_files=1`; new symbols in storage |
| AC-4: Deleted file removed | Pass | `changed_files=1`; `get_indexed_files()` no longer contains deleted path |
| AC-5: full=True forces full re-index | Pass | `result.incremental=False` |
| AC-6: storage=None skips incremental | Pass | Branch guarded by `storage is not None` |
| AC-7: No regression | Pass | 650/650 tests passing |

## Accomplishments

- Added incremental fast-path to `run_pipeline()`: after `walk_repo()`, computes sha256 of each file's content, diffs against `storage.get_indexed_files()` manifest, calls `reindex_files()` for changed/new files, removes deleted files, then early-returns
- `result.incremental` and `result.changed_files` now populated on warm-start calls (previously always False/0)
- 5 new `TestIncrementalPipeline` tests cover no-change, change, add, delete, and full-bypass scenarios

## Files Created/Modified

| File | Change | Purpose |
|------|--------|---------|
| `src/axon/core/ingestion/pipeline.py` | Modified | Added `import hashlib` + 37-line incremental branch |
| `tests/core/test_pipeline.py` | Modified | Added `TestIncrementalPipeline` (5 tests) |
| `tests/e2e/test_full_pipeline.py` | Modified | Updated `test_idempotent` for incremental semantics |

## Decisions Made

| Decision | Rationale | Impact |
|----------|-----------|--------|
| Content hash over mtime | Reliable across copies/moves; content already in memory from walk | No mtime storage needed |
| Skip global phases on incremental | Consistent with watcher; community/dead-code requires full graph anyway | 19% community phase not re-run on warm starts |
| `result.symbols=0` on incremental | Symbols not re-counted (partial run); avoids misleading stats | Callers must check `result.incremental` before using counts |
| Inline, no helper function | 37 lines with clear structure; abstraction not justified | Zero indirection |

## Deviations from Plan

| Type | Count | Impact |
|------|-------|--------|
| Auto-fixed | 1 | Pre-existing e2e test updated |
| Scope additions | 0 | — |
| Deferred | 0 | — |

**Total impact:** One essential test fix, no scope creep.

### Auto-fixed: `test_idempotent` semantics

- **Found during:** Task 2 (full suite run)
- **Issue:** `TestIdempotency::test_idempotent` asserted `result1.symbols == result2.symbols` across two runs. The incremental path returns `symbols=0` by design (no files parsed), so this assertion fails.
- **Fix:** Updated test to assert `result2.incremental is True` and `result2.changed_files == 0` — which is the correct idempotency contract now.
- **Files:** `tests/e2e/test_full_pipeline.py`
- **Verification:** Test passes post-fix

## Issues Encountered

None beyond the auto-fixed test deviation above.

## Next Phase Readiness

**Ready:**
- Incremental path live; any re-index call on an existing DB now skips unchanged files automatically
- Benchmark CLI can re-run to measure warm-start speedup: `uv run python benchmarks/run_benchmark.py --repo-path . --no-embeddings` (run twice, second run measures incremental overhead only)
- Foundation for Plan 02-03 (parallel parsing) is in place: `walk_repo()` already uses `ThreadPoolExecutor`; parse phase is the remaining bottleneck for cold-start indexes

**Concerns:**
- `result.symbols=0` on incremental runs may surprise callers who always expect a populated count; callers should gate on `result.incremental`
- Community detection (19%) still runs on full re-index (cold start) — worth revisiting if cold-start time matters at 100k+ LOC

**Blockers:** None

---
*Phase: 02-large-project-performance, Plan: 02*
*Completed: 2026-02-26*
