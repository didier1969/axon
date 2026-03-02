---
phase: 01-centralisation-stockage
plan: 01
subsystem: storage
tags: [kuzudb, storage, migration, central-db, mcp, cli]

requires: []
provides:
  - Central KuzuDB storage at ~/.axon/repos/{slug}/kuzu
  - Auto-migration of legacy local KuzuDB on axon analyze
  - Slug-based repo identity in local meta.json
  - Backward-compat fallback for repos without slug field
affects: [02-daemon-central, 03-watch-filtrage]

tech-stack:
  added: []
  patterns:
    - central-storage: all KuzuDB at ~/.axon/repos/{slug}/kuzu, meta.json stays local
    - placeholder-before-init: write placeholder meta.json to slot before KuzuDB.initialize()
    - legacy-fallback: if meta.json lacks slug → open {project}/.axon/kuzu

key-files:
  created: []
  modified:
    - src/axon/cli/main.py
    - src/axon/mcp/server.py
    - src/axon/mcp/tools.py
    - tests/cli/test_main.py
    - tests/mcp/test_tools.py

key-decisions:
  - "Central KuzuDB at ~/.axon/repos/{slug}/kuzu — one location per repo regardless of project dir"
  - "Placeholder meta.json before KuzuDB init prevents _register_in_global_registry from deleting the slot"
  - "Slug computation inlined (not extracted) to keep blast radius minimal"
  - "Auto-migration via shutil.move on analyze — no manual migration command"

patterns-established:
  - "All storage paths use _central_db_path(slug) helper"
  - "All read commands: read slug from meta.json → central path → legacy fallback"
  - "analyze/watch/serve: compute slug early, write placeholder, then init KuzuDB"

duration: ~2h
started: 2026-03-02T00:00:00Z
completed: 2026-03-02T00:00:00Z
---

# Phase 1 Plan 01: Centralisation du stockage — Summary

**KuzuDB migrated from `{project}/.axon/kuzu` to `~/.axon/repos/{slug}/kuzu` with auto-migration, backward compat, and 782 tests passing.**

## Performance

| Metric | Value |
|--------|-------|
| Duration | ~2h |
| Started | 2026-03-02 |
| Completed | 2026-03-02 |
| Tasks | 2 completed |
| Files modified | 5 |
| Tests | 782 passed, 0 failed (+6 new tests) |

## Acceptance Criteria Results

| Criterion | Status | Notes |
|-----------|--------|-------|
| AC-1: Central DB path for new indexes | Pass | kuzu at `~/.axon/repos/{slug}/kuzu`, verified manually |
| AC-2: Auto-migration of local DB | Pass | `shutil.move` on analyze; log confirms migration |
| AC-3: CLI read commands use central DB | Pass | `_load_storage()` reads slug from meta.json |
| AC-4: MCP repo= queries use central DB | Pass | `_load_repo_storage()` opens `~/.axon/repos/{repo}/kuzu` |
| AC-5: MCP server lazy-init uses central DB | Pass | `_get_storage()` reads slug from CWD meta.json |
| AC-6: clean removes central DB + local .axon/ | Pass | Reads slug, rmtree central then local |
| AC-7: Backward compat for un-migrated repos | Pass | Missing slug → fallback to `{project}/.axon/kuzu` |
| AC-8: Tests pass | Pass | 782 passed, 0 failures |

## Accomplishments

- All KuzuDB storage centralised at `~/.axon/repos/{slug}/kuzu` — prerequisites for Phase 2 daemon LRU cache
- Auto-migration transparent: existing repos migrate silently on next `axon analyze`
- Backward compat ensures zero breakage for repos indexed pre-v0.6
- Discovered and fixed critical bug: `_register_in_global_registry` deleted central slot without placeholder meta.json

## Files Created/Modified

| File | Change | Purpose |
|------|--------|---------|
| `src/axon/cli/main.py` | Modified | `_central_db_path()`, `_auto_migrate_local_kuzu()`, updated `analyze`/`watch`/`serve`/`clean`/`_load_storage` |
| `src/axon/mcp/server.py` | Modified | `_get_storage()` reads slug from local meta.json, opens central DB |
| `src/axon/mcp/tools.py` | Modified | `_load_repo_storage()` opens `~/.axon/repos/{repo}/kuzu` first |
| `tests/cli/test_main.py` | Modified | Added `TestLoadStorage` (3 tests) + `test_clean_deletes_central_db_when_slug_present` |
| `tests/mcp/test_tools.py` | Modified | Added 2 tests: central path + legacy fallback for `_load_repo_storage` |

## Decisions Made

| Decision | Rationale | Impact |
|----------|-----------|--------|
| Central path: `~/.axon/repos/{slug}/kuzu` | All DBs in one place for daemon LRU cache (Phase 2) | All read/write paths updated |
| Slug inlined, not extracted into helper | Minimal blast radius; no shared helper needed for 3 call sites | `analyze`/`watch`/`serve` each have 6-line inline block |
| Placeholder meta.json before KuzuDB init | Prevents `_register_in_global_registry` rmtree on fresh central slot | Written only if placeholder doesn't exist; overwritten by real meta afterward |
| Legacy fallback: no slug → `{project}/.axon/kuzu` | Zero breakage for repos indexed before v0.6 | `_load_storage` and `_load_repo_storage` both implement fallback |

## Deviations from Plan

### Summary

| Type | Count | Impact |
|------|-------|--------|
| Auto-fixed | 2 | Essential corrections, no scope creep |
| Scope additions | 0 | - |
| Deferred | 0 | - |

**Total impact:** Two essential fixes, plan executed exactly as specified otherwise.

### Auto-fixed Issues

**1. Critical: `_register_in_global_registry` deleted central slot**
- **Found during:** Task 1 manual spot-check (after Task 2 tests passed)
- **Issue:** `_register_in_global_registry` treats a slot without `meta.json` as corrupt and calls `shutil.rmtree(candidate)`. New central slot had kuzu DB but no meta.json yet.
- **Fix:** Write placeholder `meta.json` with `{"path": str(repo_path), "name": repo_path.name}` to central slot before `KuzuBackend.initialize()`. When `_register_in_global_registry` runs, path matches, slot preserved; real meta.json overwrites placeholder.
- **Files:** `src/axon/cli/main.py` (`analyze` command)
- **Verification:** `~/.axon/repos/test_axon_central/kuzu` survived after analyze ✓

**2. Minor: test_no_index_exits_with_error caught wrong exception**
- **Found during:** Task 2 test writing
- **Issue:** Test used `pytest.raises(SystemExit)` but `typer.Exit` raises `click.exceptions.Exit`
- **Fix:** Changed to `pytest.raises(click.exceptions.Exit)` with `import click`
- **Files:** `tests/cli/test_main.py`

## Issues Encountered

| Issue | Resolution |
|-------|------------|
| git stash pop conflict on uv.lock during ruff pre-check | `git checkout -- uv.lock && git stash pop` |
| Ruff E501/I001 in unchanged lines | Pre-existing errors; no new errors introduced ✓ |

## Next Phase Readiness

**Ready:**
- All KuzuDB at `~/.axon/repos/{slug}/kuzu` — daemon Phase 2 can implement LRU cache over this directory
- `meta.json` registry at `~/.axon/repos/{slug}/meta.json` unchanged — daemon can list all DBs by scanning this dir
- `_central_db_path(slug)` helper reusable in Phase 2 daemon code

**Concerns:**
- None

**Blockers:**
- None

---
*Phase: 01-centralisation-stockage, Plan: 01*
*Completed: 2026-03-02*
