---
phase: 02-qualite-parsers-features
plan: 03
subsystem: core
tags: [walker, paths, cli, file-size, slug, refactor]

requires:
  - phase: 02-01
    provides: byte-offset infrastructure
  - phase: 02-02
    provides: parser quality + test coverage

provides:
  - 512KB file size guard in walker (OOM prevention)
  - compute_repo_slug() shared helper in core/paths.py
  - Deduplicated slug logic across 3 CLI commands

affects: [02-04, future walker changes, cli slug computation]

tech-stack:
  added: []
  patterns:
    - "File size guard before read_text() via stat().st_size"
    - "Pure helper in core/paths.py for shared CLI logic"

key-files:
  created: []
  modified:
    - src/axon/core/ingestion/walker.py
    - src/axon/core/paths.py
    - src/axon/cli/main.py
    - tests/core/test_walker.py

key-decisions:
  - "stat() check inside existing OSError try block (avoids separate handler)"
  - "compute_repo_slug() is pure (no side effects) — _register_in_global_registry kept as-is"
  - "hashlib kept in cli/main.py (still used by _register_in_global_registry)"

patterns-established:
  - "Walker file size limit: _MAX_FILE_BYTES = 512 * 1024 at module level"
  - "Slug helper: compute_repo_slug(repo_path) in core/paths.py"

duration: ~15min
started: 2026-03-03T00:00:00Z
completed: 2026-03-03T00:15:00Z
---

# Phase 02 Plan 03: Walker 512KB Limit + compute_repo_slug() Summary

**Walker now skips files >512KB with a warning; slug computation extracted from 3 duplicated CLI blocks into a shared helper in core/paths.py.**

## Performance

| Metric | Value |
|--------|-------|
| Duration | ~15 min |
| Tasks | 5/5 completed |
| Files modified | 4 |
| Tests | 875 passing (+4 new) |

## Acceptance Criteria Results

| Criterion | Status | Notes |
|-----------|--------|-------|
| AC-1: Large Files Skipped | Pass | Files >512KB excluded from walk_repo(); logger.warning emitted with path |
| AC-2: compute_repo_slug Helper | Pass | Importable from axon.core.paths; collision logic matches inline original |
| AC-3: CLI Uses Helper | Pass | 4 occurrences in cli/main.py (1 import + 3 usages); 0 inline blocks remain |
| AC-4: Tests Pass | Pass | 875 tests passing (871 → 875, +4 walker tests) |

## Accomplishments

- `walker.read_file()` now guards against OOM on large files: stat check before `read_text()`, returns `None` with `logger.warning` for files >512KB
- `compute_repo_slug(repo_path)` added to `core/paths.py` — pure function, no side effects, exact same collision logic as the 3 inline blocks it replaced
- `cli/main.py` analyze, serve, and watch commands all use the helper; removed the "Compute slug (inline, mirrors...)" comment blocks
- 4 new tests in `TestWalkRepoSkipsLargeFiles`: skip behavior, warning emission, boundary (512KB+1), under-limit included

## Files Created/Modified

| File | Change | Purpose |
|------|--------|---------|
| `src/axon/core/ingestion/walker.py` | Modified | Added `import logging`, `logger`, `_MAX_FILE_BYTES`, stat check in `read_file()` |
| `src/axon/core/paths.py` | Modified | Added `import hashlib`, `import json`, `compute_repo_slug()` function |
| `src/axon/cli/main.py` | Modified | Added `compute_repo_slug` to import; replaced 3 inline slug blocks |
| `tests/core/test_walker.py` | Modified | Added `import logging`, `TestWalkRepoSkipsLargeFiles` (4 tests) |

## Decisions Made

| Decision | Rationale | Impact |
|----------|-----------|--------|
| stat() inside existing OSError try block | `stat()` can raise OSError; no separate handler needed | Clean, minimal change |
| `_register_in_global_registry` unchanged | Has side effects (shutil.rmtree) beyond slug computation — different concern | Avoids unintended behavior change |
| hashlib kept in cli/main.py | Still referenced by `_register_in_global_registry` | No import removal needed |

## Deviations from Plan

None — plan executed exactly as written.

## Issues Encountered

| Issue | Resolution |
|-------|------------|
| System `python3` lacks axon module | Used `.venv/bin/python3` (standard for this project) |

## Next Phase Readiness

**Ready:**
- walker.py now safe for repos with large generated/binary files mistakenly tracked
- compute_repo_slug() available to any future code needing slug computation

**Concerns:**
- None

**Blockers:**
- None — plan 02-04 (socket buffer readline(), axon_batch partial failure, AXON_LRU_SIZE) ready to plan

---
*Phase: 02-qualite-parsers-features, Plan: 03*
*Completed: 2026-03-03*
