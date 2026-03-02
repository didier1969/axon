---
phase: 03-watch-filtrage
plan: 02
subsystem: watcher
tags: [asyncio, queue, producer-consumer, watchfiles, ingestion]

# Dependency graph
requires:
  - phase: 03-01
    provides: watch_filter, debounce_ms param, .paul/.git/.axon exclusions
provides:
  - asyncio.Queue producer/consumer pattern inside watch_repo()
  - Sequential batch consumption (no producer stall under lock)
  - Sentinel-based clean consumer exit
affects: [03-03 byte-offset caching, serve --watch mode, MCP lock handling]

# Tech tracking
tech-stack:
  added: []
  patterns: [asyncio.Queue producer/consumer with None sentinel, inner coroutines closing over shared state]

key-files:
  created: []
  modified:
    - src/axon/core/ingestion/watcher.py
    - tests/core/test_watcher.py

key-decisions:
  - "asyncio.Queue[list[Path] | None] — None sentinel stops consumer; no queue.join() needed"
  - "Inner _producer/_consumer close over queue/gitignore/storage — no arg threading"
  - "Timer logic (dirty, last_global, last_embed) moved entirely into _consumer"

patterns-established:
  - "Producer puts batches + sentinel; consumer breaks on None + calls task_done()"
  - "asyncio.gather(create_task(_producer()), create_task(_consumer())) as main body"

# Metrics
duration: ~30min
started: 2026-03-02T00:00:00Z
completed: 2026-03-02T00:00:00Z
---

# Phase 3 Plan 02: asyncio.Queue Producer/Consumer Watcher Summary

**`watch_repo()` refactored with asyncio.Queue: producer feeds path batches independently while consumer drains sequentially, eliminating producer stall when MCP holds the lock.**

## Performance

| Metric | Value |
|--------|-------|
| Duration | ~30min |
| Started | 2026-03-02 |
| Completed | 2026-03-02 |
| Tasks | 2 completed |
| Files modified | 2 |

## Acceptance Criteria Results

| Criterion | Status | Notes |
|-----------|--------|-------|
| AC-1: Producer feeds queue independently | Pass | test_processes_multiple_batches: 2 batches → 2 _reindex_files calls |
| AC-2: Consumer processes sequentially | Pass | Queue ensures FIFO; consumer awaits each batch before next |
| AC-3: Sentinel terminates consumer cleanly | Pass | None sentinel → break + task_done(); gather() returns cleanly |
| AC-4: Existing watch_repo callers unaffected | Pass | Public signature unchanged; all existing tests pass |

## Accomplishments

- `watch_repo()` split into `_producer()` (awatch loop → queue) and `_consumer()` (drain + timers)
- Timer ownership moved to consumer: `dirty`, `last_global`, `last_embed` now local to `_consumer()`
- 2 new tests added: `test_processes_multiple_batches` and `test_empty_changes_not_queued`
- Full suite: 823 tests, 0 failures, 0 ruff errors

## Task Commits

| Task | Commit | Type | Description |
|------|--------|------|-------------|
| Task 1: Refactor watch_repo() with Queue | `4bf2657` | feat | asyncio.Queue producer/consumer watcher |
| Task 2: Add queue tests | `4bf2657` | feat | TestWatchRepoQueue — 2 new tests |

Plan metadata: `4bf2657` (feat(03-watch-filtrage): Plan 03-02 — asyncio.Queue producer/consumer watcher)

## Files Created/Modified

| File | Change | Purpose |
|------|--------|---------|
| `src/axon/core/ingestion/watcher.py` | Modified | watch_repo() refactored with _producer/_consumer inner coroutines |
| `tests/core/test_watcher.py` | Modified | TestWatchRepoQueue class with 2 new queue-behaviour tests |

## Decisions Made

| Decision | Rationale | Impact |
|----------|-----------|--------|
| None sentinel instead of queue.join() | Simpler; producer signals done by value, consumer exits on it | Consumer loop is a clean while True + break |
| Inner coroutines close over queue/gitignore | No need to pass args through Queue items | Keeps batch items as plain list[Path] |
| Timer logic in consumer only | Only consumer processes batches; producer is fire-and-forget | Single source of truth for timing state |

## Deviations from Plan

### Summary

| Type | Count | Impact |
|------|-------|--------|
| Auto-fixed | 0 | — |
| Scope additions | 0 | — |
| Deferred | 0 | — |

**Total impact:** None — plan executed exactly as written.

## Issues Encountered

None.

## Next Phase Readiness

**Ready:**
- watch_repo() is fully queue-based; producer never stalls under MCP lock
- Test fixtures and patterns stable for 03-03

**Concerns:**
- None

**Blockers:**
- None — ready for Plan 03-03 (byte-offset caching: start_byte/end_byte in schema)

---
*Phase: 03-watch-filtrage, Plan: 02*
*Completed: 2026-03-02*
