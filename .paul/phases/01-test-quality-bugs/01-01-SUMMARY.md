---
phase: 01-test-quality-bugs
plan: 01
subsystem: testing
tags: [pytest, conftest, kuzu, embeddings, fixtures]

requires: []
provides:
  - autouse isolation fixture preventing events.jsonl pollution
  - embeddings=False applied to all non-embedding test calls
affects: 01-02 (if session-scoped KuzuDB fixture plan proceeds)

tech-stack:
  added: []
  patterns:
    - "autouse conftest.py fixture for test isolation (monkeypatching Path.home)"

key-files:
  created:
    - tests/core/conftest.py
  modified:
    - tests/core/test_pipeline.py
    - tests/core/test_watcher.py

key-decisions:
  - "AC-2/AC-3 MISSED: root cause was KuzuDB init overhead, not fastembed threads"
  - "embeddings=False applied but did not fix performance — wrong hypothesis"
  - "AC-1 fixed cleanly via autouse fixture"

patterns-established:
  - "tests/core/conftest.py: isolated_axon_home autouse fixture redirects Path.home() to tmp_path"

duration: ~1 session
started: 2026-02-28T00:00:00Z
completed: 2026-02-28T23:59:00Z
---

# Phase 1 Plan 01: Test Isolation & Performance — Summary

**AC-1 fixed (events.jsonl isolation works); AC-2/AC-3 MISSED — root cause was KuzuBackend.initialize() overhead, not fastembed threads.**

## Performance

| Metric | Value |
|--------|-------|
| Duration | ~1 session |
| Started | 2026-02-28 |
| Completed | 2026-02-28 |
| Tasks | 3 executed |
| Files modified | 3 |

## Acceptance Criteria Results

| Criterion | Status | Notes |
|-----------|--------|-------|
| AC-1: No events.jsonl pollution | **PASS** | Autouse fixture confirmed working — line count unchanged after full suite |
| AC-2: test_pipeline.py < 20s | **FAIL** | Still ~166s. embeddings=False did not help (wrong root cause) |
| AC-3: test_watcher.py < 15s | **FAIL** | Still ~102s. Same root cause miss |
| AC-4: test_analytics.py passes | **PASS** | 6 passed in 0.06s — autouse fixture doesn't conflict |
| AC-5: Full suite passes | **UNKNOWN** | Not verified before pause |

## Accomplishments

- `tests/core/conftest.py` created with autouse `isolated_axon_home` fixture — all tests now run in isolated temp axon home
- `embeddings=False` applied to 8 `run_pipeline()` calls in test_pipeline.py and 8 calls in test_watcher.py
- Root cause of test slowness correctly identified (during execution, not before)

## Task Commits

No per-task commits were made — changes staged but not committed before session pause.

| Task | Status | Notes |
|------|--------|-------|
| Task 1: Create conftest.py | Done | tests/core/conftest.py created |
| Task 2: embeddings=False in test_pipeline.py | Done | 8 calls patched |
| Task 3: embeddings=False in test_watcher.py | Done | 8 calls patched |

## Files Created/Modified

| File | Change | Purpose |
|------|--------|---------|
| `tests/core/conftest.py` | **Created** | autouse fixture: redirects Path.home() → tmp_path for all tests/core/ |
| `tests/core/test_pipeline.py` | Modified | 8 `run_pipeline()` calls + `embeddings=False` |
| `tests/core/test_watcher.py` | Modified | 8 `run_pipeline()` calls + `embeddings=False` |

## Critical Deviation: Wrong Root Cause Diagnosis

### Summary

| Type | Count | Impact |
|------|-------|--------|
| Misdiagnosis | 1 | AC-2 and AC-3 targets missed entirely |
| Deferred | 1 | test performance — logged for Plan 01-02 or Phase 2 |

**Total impact:** AC-1 wins cleanly; performance work wasted on wrong hypothesis.

### Root Cause Correction

**Plan hypothesis:** Test slowness caused by `fastembed` background threads loading the embedding model (~4-8s per `run_pipeline()` call without `embeddings=False`).

**Actual root cause (discovered by profiling during execution):**
- `KuzuBackend.initialize()` creates a fresh KuzuDB on disk — **4-5s per test in fixture setup**
- `run_pipeline()` + `bulk_load()` for even a tiny 2-3 file repo — **5-7s per test call**
- Embeddings are a no-op when `embeddings=False`, so disabling them saved <1s per test
- The background fastembed threads were not the bottleneck at all

Consequence: 18 tests × (4-5s init + 5-7s pipeline) ≈ 162-216s → matches observed 166s. Target of <20s requires a fundamentally different approach.

### Flaky Test Fix (Identified, Not Applied)

`test_async_embeddings_returns_future` race condition:
- **Problem:** `with patch()` context exits before background thread runs `embed_graph`, so real fastembed loads
- **Fix:** Move `result.embedding_future.result(timeout=10)` INSIDE the `with patch()` block
- **Status:** Not applied — deferred to Plan 01-02

## Issues Encountered

| Issue | Resolution |
|-------|------------|
| Wrong root cause diagnosis | Discovered via profiling during execution; AC-2/AC-3 remain unmet |
| Session paused before UNIFY | Captured in handoff HANDOFF-2026-02-28b.md |

## Decision Required: What to Do About Performance

Three options identified:

**Option A — Plan 01-02: Session-scoped KuzuDB fixture**
- Create a session-scoped pre-initialized KuzuDB template, copied (not re-created) per test
- Saves 4-5s × 30 tests = 2-2.5 minutes
- Keeps tests as true integration tests
- Requires careful fixture design for isolation

**Option B — Accept partial win, move to Phase 2**
- AC-1 (events.jsonl isolation) is fixed and committed value
- Test slowness is KuzuDB overhead — known, deferred
- Move on to Phase 2: Elixir `use` parser + community detection parallelization

**Option C — Mock KuzuDB in tests**
- Replace real KuzuBackend with a mock for pipeline tests
- Fastest tests but loses integration test value
- Significant effort to implement correctly

## Next Phase Readiness

**Ready:**
- events.jsonl isolation works — AC-1 is solid
- Root cause of test slowness is correctly understood (KuzuBackend.initialize overhead)
- A fix path exists (Option A: session-scoped fixture)

**Concerns:**
- embeddings=False changes are in place but effectively no-ops for performance
- Flaky async test fix still pending

**Blockers:**
- None — need decision on Option A/B/C before next plan

---
*Phase: 01-test-quality-bugs, Plan: 01*
*Completed: 2026-02-28*
