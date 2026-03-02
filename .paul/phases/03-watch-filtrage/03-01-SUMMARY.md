---
phase: 03-watch-filtrage
plan: 01
subsystem: watcher
tags: [watchfiles, ignore-patterns, debounce, cli]

requires:
  - phase: 02-daemon-central
    provides: Daemon + MCP proxy infrastructure; watcher unchanged in v0.6

provides:
  - .paul/ added to DEFAULT_IGNORE_PATTERNS (watcher no longer re-indexes PAUL files)
  - watchfiles pre-filter via _make_watch_filter() — ignored paths filtered before entering loop
  - debounce_ms parameter on watch_repo() — configurable at call site
  - --debounce CLI flag on `axon watch` and `axon serve`

affects: 03-02-queue, 03-03-byte-offset

tech-stack:
  added: []
  patterns:
    - "watch_filter callable pattern for watchfiles.awatch() — pre-filter before Python loop"

key-files:
  created: []
  modified:
    - src/axon/config/ignore.py
    - src/axon/core/ingestion/watcher.py
    - src/axon/cli/main.py
    - tests/config/test_ignore.py
    - tests/core/test_watcher.py

key-decisions:
  - "watch_filter at watchfiles level (not in-Python dedup) — prevents path from ever entering awatch loop"
  - "_reindex_files ignore check retained as safety net for direct callers"

patterns-established:
  - "_make_watch_filter(repo_path, gitignore) factory pattern for watch_filter callables"

duration: ~45min
started: 2026-03-02T00:00:00Z
completed: 2026-03-02T00:00:00Z
---

# Phase 3 Plan 01: Watch Filters + Configurable Debounce Summary

**Added `.paul/` to ignore patterns, pre-filter at watchfiles level via `_make_watch_filter()`, and `debounce_ms` param + `--debounce` CLI flag — watcher no longer reacts to PAUL file edits.**

## Performance

| Metric | Value |
|--------|-------|
| Duration | ~45 min |
| Started | 2026-03-02 |
| Completed | 2026-03-02 |
| Tasks | 3 completed |
| Files modified | 5 |

## Acceptance Criteria Results

| Criterion | Status | Notes |
|-----------|--------|-------|
| AC-1: .paul/ files are ignored | Pass | `should_ignore(".paul/STATE.md")` → True; `_reindex_files` returns count==0 |
| AC-2: watchfiles pre-filter skips ignored paths | Pass | `_make_watch_filter()` returns False for .paul/ paths; never enters loop |
| AC-3: Debounce is configurable | Pass | `debounce_ms=1000` passes `rust_timeout=1000` to `watchfiles.awatch()` |
| AC-4: CLI flags accepted | Pass | `axon watch --debounce 200` and `axon serve --debounce 200` both accepted |

## Accomplishments

- `.paul/` added to `DEFAULT_IGNORE_PATTERNS` alongside `.git` and `.axon`
- `_make_watch_filter()` helper returns a watchfiles-compatible `watch_filter` callable that pre-filters ignored paths at the watchfiles level (before Python loop)
- `watch_repo()` gains `debounce_ms: int = 500` param; `rust_timeout` no longer hardcoded
- `axon watch` and `axon serve` both gain `--debounce INTEGER` option
- 3 new tests added; 821 total tests, 0 failures; 0 ruff lint errors in modified files

## Task Commits

| Task | Commit | Type | Description |
|------|--------|------|-------------|
| Task 1+2+3 | `3aaadc7` | feat | filters + configurable debounce (all tasks in single commit) |

## Files Created/Modified

| File | Change | Purpose |
|------|--------|---------|
| `src/axon/config/ignore.py` | Modified | Added `".paul"` to `DEFAULT_IGNORE_PATTERNS` |
| `src/axon/core/ingestion/watcher.py` | Modified | `_make_watch_filter()` helper + `debounce_ms` param + `rust_timeout` wired |
| `src/axon/cli/main.py` | Modified | `--debounce` option on `watch` and `serve` commands |
| `tests/config/test_ignore.py` | Modified | `test_ignores_paul_directory` (2 assertions) |
| `tests/core/test_watcher.py` | Modified | `test_paul_files_skipped_by_reindex_files` + `test_debounce_ms_accepted` |

## Decisions Made

| Decision | Rationale | Impact |
|----------|-----------|--------|
| watchfiles `watch_filter` callable (not in-Python dedup) | Prevents path from ever entering awatch loop — more efficient | Pattern for 03-02 queue integration |
| `_reindex_files` ignore check retained | Safety net for direct callers (tests, non-watchfiles paths) | Belt-and-suspenders; no wasted reindexes from PAUL |

## Deviations from Plan

None — plan executed exactly as specified. Pre-existing ruff lint errors (E501, E402, I001) in modified files were fixed as part of clean-up (within APPLY scope).

## Issues Encountered

None.

## Next Phase Readiness

**Ready:**
- Plan 03-02 can proceed: asyncio.Queue sequential consumer
- `watch_repo()` signature stable; 03-02 will add `queue` logic inside the existing async loop
- All 821 tests passing, clean lint baseline

**Concerns:**
- None

**Blockers:**
- None

---
*Phase: 03-watch-filtrage, Plan: 01*
*Completed: 2026-03-02*
