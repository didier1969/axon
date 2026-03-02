---
phase: 01-securite-robustesse
plan: 01
subsystem: security
tags: [security, path-traversal, cypher-injection, thread-safety, socket-permissions, snippets, queue, atomicity]

requires:
  - phase: v0.6-daemon-centralisation
    provides: KuzuDB backend, MCP proxy daemon, watcher pipeline

provides:
  - Path traversal prevention in _load_repo_storage()
  - Cypher injection prevention + N+1 elimination in handle_detect_changes()
  - Complete _WRITE_KEYWORDS (DDL coverage)
  - Thread-safe _get_storage() with double-checked locking
  - Unix socket 0o600 permissions
  - Semantic snippets via _make_snippet() (400 chars, newline-aware)
  - Callers/callees capped at 20 with overflow message
  - Bounded asyncio.Queue(maxsize=100) + drop-oldest strategy
  - remove_nodes_by_file() returns actual deletion count
  - Atomic meta.json write via tempfile + os.replace()

affects: [phase-02-qualite-parsers-features]

tech-stack:
  added: []
  patterns: [double-checked-locking, parameterized-queries, atomic-file-write, bounded-queue-drop-oldest, semantic-truncation]

key-files:
  modified:
    - src/axon/mcp/tools.py
    - src/axon/mcp/server.py
    - src/axon/daemon/server.py
    - src/axon/core/storage/kuzu_search.py
    - src/axon/core/storage/kuzu_backend.py
    - src/axon/core/ingestion/watcher.py
    - src/axon/cli/main.py
    - tests/mcp/test_tools.py
    - tests/mcp/test_server.py
    - tests/core/test_kuzu_search.py
    - tests/core/test_kuzu_backend.py
    - tests/core/test_watcher.py

key-decisions:
  - "Drop-oldest strategy on full queue: most recent events > stale batches"
  - "_make_snippet max 400 chars: signatures can be long; 200 was too narrow for LLMs"
  - "Count-before-delete in remove_nodes_by_file: KuzuDB lacks DETACH DELETE … RETURNING count(*)"
  - "All 11 fixes in 1 plan: fixes are independent, no inter-task dependencies"

patterns-established:
  - "Parameterized KuzuDB queries: execute_raw(query, parameters={'key': value})"
  - "_sanitize_repo_slug() as security gate before any filesystem path construction"
  - "Atomic writes: NamedTemporaryFile + os.replace() for all persistent metadata"
  - "Bounded queues with drop-oldest: preserve newest events, warn on overflow"

duration: ~45min
started: 2026-03-02T00:00:00Z
completed: 2026-03-02T00:00:00Z
---

# Phase 1 Plan 01: Sécurité & Robustesse Summary

**11 security vulnerabilities and bugs eliminated — audit score raised from 61→~75/100, 28 new tests added (824→852 total).**

## Performance

| Metric | Value |
|--------|-------|
| Duration | ~45 min |
| Date | 2026-03-02 |
| Tasks | 3 completed |
| Files modified | 12 (7 source + 5 test) |
| Tests added | 28 |

## Acceptance Criteria Results

| Criterion | Status | Notes |
|-----------|--------|-------|
| AC-1: Path traversal blocked | Pass | `_sanitize_repo_slug()` rejects `..`, `/`, spaces, null bytes, >200 chars |
| AC-2: Cypher injection + N+1 eliminated | Pass | Single `IN $fps` parameterized query; `execute_raw()` accepts `parameters` |
| AC-3: `_WRITE_KEYWORDS` complete | Pass | RENAME, ALTER, IMPORT, TRUNCATE added |
| AC-4: `_get_storage()` thread-safe | Pass | Double-checked locking with `threading.Lock()`; 2 threads → 1 init |
| AC-5: Unix socket owner-only | Pass | `os.chmod(sock_path, stat.S_IRUSR \| stat.S_IWUSR)` (0o600) |
| AC-6: Semantic snippets | Pass | `_make_snippet()`: prefers signature, 400-char limit, newline boundary |
| AC-7: Callers/callees capped | Pass | `_MAX_RELATIONS_DISPLAYED = 20`; "... and N more" overflow line |
| AC-8: Queue bounded | Pass | `asyncio.Queue(maxsize=100)` + drop-oldest on `QueueFull` |
| AC-9: `remove_nodes_by_file` count | Pass | COUNT before DELETE per table; returns actual total |
| AC-10: Atomic meta.json write | Pass | `NamedTemporaryFile` + `os.replace()` — no partial writes |

## Accomplishments

- **3 CRITIQUE security fixes**: path traversal, Cypher injection, DDL write-guard bypass — zero known critical attack vectors remaining
- **2 MAJEUR security fixes**: race condition in lazy-init, world-readable socket — hardened against concurrent access and local privilege issues
- **6 MAJEUR bug fixes**: better LLM output (snippets, callers cap), stable pipeline under load (bounded queue, atomic writes), correct API return values

## Files Created/Modified

| File | Change | Purpose |
|------|--------|---------|
| `src/axon/mcp/tools.py` | Modified | `_sanitize_repo_slug()`, `_MAX_RELATIONS_DISPLAYED`, `_WRITE_KEYWORDS`, batched detect-changes query, callers/callees cap |
| `src/axon/mcp/server.py` | Modified | `threading.Lock()` + double-checked locking in `_get_storage()` |
| `src/axon/daemon/server.py` | Modified | `os.chmod(sock_path, 0o600)` after socket bind |
| `src/axon/core/storage/kuzu_search.py` | Modified | `_make_snippet()` function; replaced all `content[:200]` occurrences |
| `src/axon/core/storage/kuzu_backend.py` | Modified | `execute_raw(parameters=)` support; `remove_nodes_by_file()` returns actual count |
| `src/axon/core/ingestion/watcher.py` | Modified | `_WATCH_QUEUE_MAXSIZE = 100`; `asyncio.Queue(maxsize=100)`; drop-oldest logic |
| `src/axon/cli/main.py` | Modified | `import tempfile`; atomic meta.json write via `NamedTemporaryFile + os.replace()` |
| `tests/mcp/test_tools.py` | Modified | `TestSanitizeRepoSlug` (6), `TestDetectChangesSecurity` (2), `TestWriteKeywords` (4), `TestCallersCap` (3) |
| `tests/mcp/test_server.py` | Modified | `TestGetStorageThreadSafety` (1), `TestDaemonSocketPermissions` (2) |
| `tests/core/test_kuzu_search.py` | Modified | `TestMakeSnippet` (5) |
| `tests/core/test_kuzu_backend.py` | Modified | `TestRemoveNodesByFileCount` (3) |
| `tests/core/test_watcher.py` | Modified | `TestWatchQueueBounded` (2) |

## Decisions Made

| Decision | Rationale | Impact |
|----------|-----------|--------|
| Drop-oldest strategy on full queue | Most recent events are most relevant; stale batches discarded gracefully | Queue never blocks producer; newest events always processed |
| `_make_snippet` max 400 chars (vs 200) | Function signatures easily exceed 200 chars; LLMs benefit from full context | Richer snippets in MCP query/context results |
| Count-before-delete in `remove_nodes_by_file` | KuzuDB lacks `DETACH DELETE … RETURNING count(*)` | Adds one COUNT query per table (cheap, O(file)) |
| All 11 fixes in 1 plan | No inter-dependencies between fixes; bulk closure faster | Single commit, minimal branch risk |

## Deviations from Plan

### Summary

| Type | Count | Impact |
|------|-------|--------|
| Auto-fixed | 1 | `_WATCH_QUEUE_MAXSIZE` initially placed inside function; moved to module level for test imports |
| Scope additions | 0 | — |
| Deferred | 0 | — |

**Total impact:** One minor placement correction, no scope creep.

### Auto-fixed Issues

**1. `_WATCH_QUEUE_MAXSIZE` placement**
- **Found during:** Task 3 (core bugs)
- **Issue:** Constant placed inside `watch_repo()` function body; test `from axon.core.ingestion.watcher import _WATCH_QUEUE_MAXSIZE` would fail
- **Fix:** Moved to module level after `EMBEDDING_INTERVAL = 60`
- **Files:** `src/axon/core/ingestion/watcher.py`
- **Verification:** `python -c "from axon.core.ingestion.watcher import _WATCH_QUEUE_MAXSIZE; assert _WATCH_QUEUE_MAXSIZE == 100"` ✓

## Issues Encountered

| Issue | Resolution |
|-------|------------|
| `uv run pytest` fails with `Invalid project metadata: Proprietary SPDX expression` | Used `python3 -m pytest` via `.venv/bin/activate` instead |
| Ruff E501 (5 pre-existing lines in tools.py) | Confirmed baseline: same 5 errors exist before changes; not introduced |

## Next Phase Readiness

**Ready:**
- All CRITIQUE/MAJEUR security issues closed — clean foundation for Phase 2 quality work
- `execute_raw(parameters=)` available — future Cypher work can use parameterized queries
- Semantic snippets in place — quality improvements accumulate organically

**Concerns:**
- `traverse_with_depth` still has N+1 BFS queries (deferred to Phase 2 scope limits)
- sql/yaml parsers still lack byte offsets (Phase 2 target)

**Blockers:** None

---
*Phase: 01-securite-robustesse, Plan: 01*
*Completed: 2026-03-02*
