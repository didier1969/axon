---
phase: 02-daemon-central
plan: 01
subsystem: daemon, infra
tags: [asyncio, unix-socket, lru-cache, kuzu, paths, cli]

requires:
  - phase: 01-centralisation-stockage
    provides: central KuzuDB path structure (~/.axon/repos/{slug}/kuzu)

provides:
  - axon.core.paths module (shared path constants)
  - axon.daemon package (LRU cache, protocol, asyncio Unix socket server)
  - axon daemon start|stop|status CLI commands

affects: 02-02-mcp-proxy (daemon is the server 02-02 will route to)

tech-stack:
  added: [asyncio.start_unix_server, subprocess.Popen start_new_session, signal.SIGTERM]
  patterns:
    - Double-checked locking for LRU cache load (I/O outside lock, insertion inside)
    - JSON-line protocol for Unix socket IPC
    - Daemon subprocess via Popen(start_new_session=True) — no os.fork()

key-files:
  created:
    - src/axon/core/paths.py
    - src/axon/daemon/__init__.py
    - src/axon/daemon/__main__.py
    - src/axon/daemon/lru_cache.py
    - src/axon/daemon/protocol.py
    - src/axon/daemon/server.py
    - tests/daemon/test_lru_cache.py
    - tests/daemon/test_server.py
  modified:
    - src/axon/cli/main.py
    - src/axon/mcp/server.py
    - src/axon/mcp/tools.py

key-decisions:
  - "Double-checked locking: KuzuBackend.initialize() I/O runs outside the lock, insertion with eviction runs inside"
  - "Popen(start_new_session=True) for daemon spawning — portable, no os.fork() complexity"
  - "MCP still uses direct KuzuBackend (unchanged) — proxy routing deferred to Plan 02-02"

patterns-established:
  - "All ~/.axon path computation goes through axon.core.paths — no inline path duplication"
  - "Daemon dispatch: resolve slug → get_or_load(slug) → call handle_* function directly"

duration: ~45min
started: 2026-03-02T16:00:00Z
completed: 2026-03-02T17:48:20Z
---

# Phase 2 Plan 01: Daemon Package + Path Refactor Summary

**Asyncio Unix socket daemon with thread-safe LRU KuzuBackend cache (maxsize=5), shared path constants module, and `axon daemon start|stop|status` CLI; 16 new tests, 798 total passing.**

## Performance

| Metric | Value |
|--------|-------|
| Duration | ~45 min |
| Completed | 2026-03-02T17:48:20Z |
| Tasks | 3 completed |
| Files created | 8 |
| Files modified | 3 |

## Acceptance Criteria Results

| Criterion | Status | Notes |
|-----------|--------|-------|
| AC-1: `axon daemon start` launches background daemon | Pass | Popen(start_new_session=True), waits for socket |
| AC-2: Daemon serves tool requests via Unix socket | Pass | JSON-line protocol, asyncio server |
| AC-3: LRU cache evicts LRU at capacity=5 | Pass | 7 LRU tests covering all eviction scenarios |
| AC-4: `axon daemon stop` sends SIGTERM | Pass | CLI reads PID file, os.kill(pid, SIGTERM) |
| AC-5: `axon daemon status` shows accurate state | Pass | Queries daemon via socket, prints cache stats |
| AC-6: `axon daemon start` idempotent when running | Pass | os.kill(pid, 0) check before spawning |
| AC-7: Path constants shared via axon.core.paths | Pass | All 3 callers updated, no inline path duplication |
| AC-8: All existing tests pass | Pass | 798 passing, 0 failures |

## Accomplishments

- `axon.core.paths` module centralises all `~/.axon/` path computation — no more inline `Path.home() / ".axon" / ...` across callers
- `axon.daemon` package: thread-safe LRU cache (OrderedDict + Lock + double-checked locking), JSON-line IPC protocol, asyncio Unix socket server dispatching all MCP tools
- `axon daemon start|stop|status` CLI subcommands via Typer sub-application

## Task Commits

| Task | Commit | Type | Description |
|------|--------|------|-------------|
| All 3 tasks | `be2bad5` | feat | implement axon daemon package and CLI commands |

## Files Created/Modified

| File | Change | Purpose |
|------|--------|---------|
| `src/axon/core/paths.py` | Created | Shared path constants: `central_db_path`, `daemon_sock_path`, `daemon_pid_path` |
| `src/axon/daemon/__init__.py` | Created | Package marker |
| `src/axon/daemon/__main__.py` | Created | Entry point: `python -m axon.daemon --max-dbs N` |
| `src/axon/daemon/lru_cache.py` | Created | Thread-safe LRU cache, double-checked locking, `close_all()` on shutdown |
| `src/axon/daemon/protocol.py` | Created | `encode_request`, `decode_request`, `encode_response` for JSON-line IPC |
| `src/axon/daemon/server.py` | Created | asyncio server: `run_daemon()`, `_dispatch_tool()`, `_handle_connection()` |
| `tests/daemon/test_lru_cache.py` | Created | 7 tests for LRU eviction, caching, status, close_all |
| `tests/daemon/test_server.py` | Created | 9 tests for protocol encoding and dispatch routing |
| `src/axon/cli/main.py` | Modified | Replaced inline `_central_db_path` with import alias; added `daemon_app` Typer sub-app |
| `src/axon/mcp/server.py` | Modified | Import `central_db_path` from `axon.core.paths` |
| `src/axon/mcp/tools.py` | Modified | Import `central_db_path` from `axon.core.paths` |

## Decisions Made

| Decision | Rationale | Impact |
|----------|-----------|--------|
| Double-checked locking in LRU cache | KuzuBackend.initialize() is slow I/O; blocking under lock would serialize all requests | Load outside lock, insert+evict inside lock — avoids race + maximises concurrency |
| Popen(start_new_session=True) for daemon spawn | Portable across Linux/macOS; avoids os.fork() complexity and double-fork pattern | Daemon is a true orphan process, socket appears within 5s |
| MCP proxy deferred to Plan 02-02 | Keeping daemon introduction isolated from MCP routing change reduces risk | MCP still uses direct KuzuBackend; daemon is ready but not yet wired |

## Deviations from Plan

None — plan executed exactly as specified. All three tasks completed in single session, committed atomically.

## Issues Encountered

| Issue | Resolution |
|-------|------------|
| Handoff stated "uncommitted" but commit `be2bad5` exists | Commit was created same session, after handoff was written. No impact. |

## Next Phase Readiness

**Ready:**
- Daemon package is importable and all AC verified
- `axon daemon start` spawns a live Unix socket server
- `_dispatch_tool` routes all 8 MCP tools through LRU cache
- Foundation for Plan 02-02: MCP proxy only needs to connect to socket and forward calls

**Concerns:**
- `axon daemon status` requires socket query; if daemon starts but socket isn't ready yet, status shows partial info (by design, best-effort)
- LRU cache opens backends in `read_only=True` mode — watcher writes to same DB; concurrency is safe for KuzuDB but worth verifying under Plan 02-03 (watcher integration)

**Blockers:**
- None

---
*Phase: 02-daemon-central, Plan: 01*
*Completed: 2026-03-02*
