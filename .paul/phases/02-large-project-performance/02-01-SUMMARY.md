---
phase: 02-large-project-performance
plan: 01
subsystem: core/ingestion
tags: [benchmarking, performance, instrumentation, pipeline, timing]

requires: []
provides:
  - PhaseTimings dataclass with per-phase float fields on PipelineResult
  - benchmarks/run_benchmark.py — standalone benchmark CLI
  - Baseline performance data identifying bottlenecks on the axon repo

affects: [02-02-incremental-indexing, 02-03-parallel-parsing]

tech-stack:
  added: []
  patterns:
    - "_t = time.monotonic() / result.phase_timings.X = time.monotonic() - _t wraps each phase"

key-files:
  created: [benchmarks/run_benchmark.py]
  modified: [src/axon/core/ingestion/pipeline.py]

key-decisions:
  - "Per-phase timing uses monotonic clock, _t pattern (no helper function — keeps it obvious)"
  - "Benchmark skips embeddings by default (--no-embeddings) to isolate indexing perf from fastembed model load"
  - "Benchmark in benchmarks/ as standalone script, not integrated into CLI — simpler for one-off runs"

patterns-established:
  - "PhaseTimings is a flat dataclass — dataclasses.asdict() gives the benchmark its row data"

duration: ~8min
started: 2026-02-26T00:00:00Z
completed: 2026-02-26T00:00:00Z
---

# Phase 2 Plan 01: Benchmark Baseline Summary

**Per-phase timing instrumented in `run_pipeline()` and benchmark CLI created; baseline shows File walking (36%) and Parsing (35%) as the two dominant phases on the axon repo.**

## Performance

| Metric | Value |
|--------|-------|
| Duration | ~8 min |
| Tasks | 2 completed |
| Files modified | 1 |
| Files created | 1 |

## Acceptance Criteria Results

| Criterion | Status | Notes |
|-----------|--------|-------|
| AC-1: PhaseTimings populated | Pass | All 13 fields populated; sum ≈ total duration (1.080s vs 1.081s on axon repo) |
| AC-2: Benchmark CLI produces report | Pass | Table and `--json` both verified; exit code 0 |
| AC-3: Existing tests unaffected | Pass | 645 tests pass (no regressions) |

## Accomplishments

- Added `PhaseTimings` dataclass (13 fields) to `pipeline.py` and attached to `PipelineResult` via `field(default_factory=PhaseTimings)`
- Instrumented all 13 phases in `run_pipeline()` with `time.monotonic()` before/after; storage and embeddings phases included
- Created `benchmarks/run_benchmark.py` with table output, descending sort, bottleneck call-out, and `--json` flag for programmatic use

## Files Created/Modified

| File | Change | Purpose |
|------|--------|---------|
| `src/axon/core/ingestion/pipeline.py` | Modified | Added `PhaseTimings` dataclass, `field` import, `phase_timings` on `PipelineResult`, instrumented 13 phases |
| `benchmarks/run_benchmark.py` | Created | Standalone CLI: runs pipeline, reports per-phase table or JSON, identifies bottleneck |

## Baseline Benchmark Data (axon repo, 85 files, 1415 symbols)

```
Phase                          Duration       %
───────────────────────────────────────────────────
File walking                      0.32s   35.9%
Parsing code                      0.31s   35.0%
Detecting communities             0.17s   19.0%
Tracing calls                     0.03s    3.6%
Detecting execution flows         0.03s    3.5%
Analyzing git history             0.01s    1.2%
Analyzing types                   0.01s    0.7%
...
Total: 0.89s
```

**Insight for Plan 02-02:** Community detection (19%) is surprisingly heavy relative to its role. On a large repo (100k+ LOC), all three — walking, parsing, and community detection — will compound. Incremental indexing (02-02) eliminates walk/parse cost for unchanged files. Parallel parsing (02-03) attacks the parse phase directly.

## Decisions Made

| Decision | Rationale | Impact |
|----------|-----------|--------|
| `_t` pattern (not helper) | Avoids abstraction for 13 identical timing lines; easy to read | Minor verbosity, zero indirection |
| `--no-embeddings` default | Fastembed model load (~2-3s) would dominate all runs and mask indexing perf | Benchmark measures indexing only |
| Standalone script in `benchmarks/` | Not a user-facing feature; developer tool for perf work | Keeps CLI surface clean |

## Deviations from Plan

None — executed exactly as written.

## Issues Encountered

None.

## Next Phase Readiness

**Ready:**
- PhaseTimings data available for Plans 02-02 and 02-03 to reference in their own benchmarks
- Benchmark CLI can be re-run against any repo to measure improvement after each optimisation
- Community detection identified as a third significant phase (not in original scope notes — worth tracking)

**Concerns:**
- Community detection at 19% on a small repo (85 files) may scale poorly — worth profiling separately before Plan 02-03
- Benchmark run against axon itself (85 files) is too small for target metric (<60s for 100k LOC); test against a large OSS repo before claiming success

**Blockers:** None

---
*Phase: 02-large-project-performance, Plan: 01*
*Completed: 2026-02-26*
