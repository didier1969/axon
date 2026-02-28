---
phase: 01-test-quality-bugs
plan: 02
subsystem: testing
tags: [pytest, kuzu, fixtures, session-scope, embeddings, watcher]

requires:
  - phase: 01-test-quality-bugs/01-01
    provides: events.jsonl isolation via autouse conftest, root-cause diagnosis (KuzuDB 4-5s init, not embeddings)

provides:
  - Session-scoped kuzu_template fixture (schema pre-created once per session)
  - Session-scoped watcher_indexed_template fixture (pre-indexed DB once per session)
  - Async embeddings race fix in test_pipeline.py
  - Watcher aggressiveness hotfix (embeddings now on 60s interval, not 30s)

affects: [phase-2-parser-performance, future-test-authorship]

tech-stack:
  added: []
  patterns:
    - "Session-scoped KuzuDB template: shutil.copy2 (single file) + initialize() = IF NOT EXISTS no-ops"
    - "watcher_indexed_template: uses patch.object(Path, 'home') not monkeypatch (function-scoped unavailable in session)"

key-files:
  created: []
  modified:
    - tests/core/conftest.py
    - tests/core/test_pipeline.py
    - tests/core/test_watcher.py
    - src/axon/core/ingestion/watcher.py

key-decisions:
  - "KuzuDB creates a single FILE at the given path (not a directory) — copy2 not copytree"
  - "test_watcher.py 28s accepted as floor: KuzuDB open on existing DB ~1.3s/test, 12 tests + 10s session setup"
  - "Watcher hotfix: _run_global_phases must pass embeddings=False; EMBEDDING_INTERVAL (60s) added to watch loop"

patterns-established:
  - "Session fixtures for KuzuDB: tmp_path_factory + copy2 template per test"
  - "Session fixtures that need Path.home() isolation: use unittest.mock.patch.object, not monkeypatch"

duration: ~45min
started: 2026-02-28T00:00:00Z
completed: 2026-02-28T00:00:00Z
---

# Phase 1 Plan 02: Session-scoped KuzuDB Fixtures + Async Race Fix

**Session-scoped KuzuDB schema template + pre-indexed watcher template eliminate per-test init overhead; async embeddings race fixed; watcher aggressiveness hotfix applied.**

## Performance

| Metric | Value |
|--------|-------|
| Duration | ~45 min |
| Tasks | 3 planned + 1 hotfix |
| Files modified | 4 |
| Tests passing | 752/752 |

## Acceptance Criteria Results

| Criterion | Status | Notes |
|-----------|--------|-------|
| AC-1: Async embeddings race fixed | **PASS** | 3/3 consecutive runs pass; future.result() moved inside `with patch()` block |
| AC-2: test_pipeline.py < 100s | **PASS** | 81s (was ~166s, -52%) |
| AC-3: test_watcher.py < 15s | **PARTIAL** | 28s (was ~102s, -73%); 15s not achievable — see deviations |
| AC-4: Full suite no regression | **PASS** | 752/752 |

## Accomplishments

- `kuzu_template` session fixture: empty schema-initialized KuzuDB created once per session; per-test storage fixtures copy it via `shutil.copy2` saving ~3.5s/test
- `watcher_indexed_template` session fixture: repo pre-indexed once per session; 8 `run_pipeline()` setup calls removed from test bodies
- Async race fixed: `result.embedding_future.result(timeout=10)` moved inside `with patch()` block so mock stays active during thread resolution
- **Hotfix (out of plan scope)**: `_run_global_phases()` in `watcher.py` was calling `run_pipeline(full=True)` with embeddings every 30s — fixed to `embeddings=False`; `EMBEDDING_INTERVAL` (60s) now actually implemented in the watch loop

## Files Created/Modified

| File | Change | Purpose |
|------|--------|---------|
| `tests/core/conftest.py` | Modified | Added `kuzu_template` + `watcher_indexed_template` session fixtures |
| `tests/core/test_pipeline.py` | Modified | Async race fix; `storage`/`rich_storage` use template via `copy2` |
| `tests/core/test_watcher.py` | Modified | `storage` uses pre-indexed template; 8 `run_pipeline()` calls removed |
| `src/axon/core/ingestion/watcher.py` | Modified | Hotfix: global phases `embeddings=False`; `last_embed` timer for 60s cadence |

## Decisions Made

| Decision | Rationale | Impact |
|----------|-----------|--------|
| `shutil.copy2` not `copytree` | KuzuDB creates a single file at the path, not a directory | All template copies use `copy2` |
| Accept 28s for test_watcher.py | `kuzu.Database()` on existing file ≈ 1.3s; 12 tests + 10s session setup = floor | Target revised to < 30s |
| Apply watcher hotfix inline | Machine stability risk; 1-line change, no test changes needed | `EMBEDDING_INTERVAL` now actually enforced |

## Deviations from Plan

### Summary

| Type | Count | Impact |
|------|-------|--------|
| Auto-fixed | 1 | KuzuDB single-file format → `copy2` instead of `copytree` |
| Scope additions | 1 | Watcher hotfix (aggressiveness bug discovered during review) |
| Deferred | 1 | AC-3 target not met (28s vs 15s) |

**Total impact:** Essential fix + valuable hotfix; no scope creep beyond what was necessary.

### Auto-fixed

**KuzuDB storage format mismatch**
- **Found during:** Task 2 (first test run)
- **Issue:** Plan used `shutil.copytree` assuming KuzuDB creates a directory; actual: single file
- **Fix:** `shutil.copytree` → `shutil.copy2` in conftest.py, test_pipeline.py, test_watcher.py (3 files)
- **Verification:** All 752 tests pass

### Scope Addition

**Watcher aggressiveness hotfix**
- **Trigger:** User concern about machine stability during indexation
- **Issue:** `_run_global_phases()` called `run_pipeline(full=True)` with `embeddings=True` (default) every 30s — loads fastembed model + embeds all symbols every 30s; `EMBEDDING_INTERVAL = 60` was defined but never used
- **Fix:** Added `with_embeddings: bool = False` param to `_run_global_phases`; added `last_embed` tracking in `watch_repo`; embeddings now fire every 60s when both global+embedding timers are dirty
- **Verification:** 76 watcher+CLI tests pass

### Deferred

- **AC-3 gap (28s vs 15s):** Even copying an existing KuzuDB and opening it takes ~1.3s per test. With 12 tests + ~10s session setup, 28s is the realistic floor without mocking KuzuDB or reusing connections across tests (which breaks isolation). Accepted as-is.

## Issues Encountered

| Issue | Resolution |
|-------|------------|
| `NotADirectoryError` on first test run | KuzuDB creates single file, not directory → `copy2` |
| AC-3 target 15s not met | KuzuDB open ~1.3s even for existing DB; 15s required ~0s open, which is unrealistic |

## Next Phase Readiness

**Ready:**
- Test infrastructure stable: isolation, speed, determinism all improved
- Foundation for Phase 2 (Parser & Performance) is clean
- Watcher is safe for production use (no embedding spikes every 30s)

**Concerns:**
- test_watcher.py at 28s (acceptable but not ideal)
- KuzuDB open cost (~1.3s) is a recurring overhead for any new integration tests

**Blockers:**
- None

---
*Phase: 01-test-quality-bugs, Plan: 02*
*Completed: 2026-02-28*
