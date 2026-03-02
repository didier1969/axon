# Project State

## Project Reference

See: .paul/PROJECT.md (updated 2026-02-28 after v0.5 complete)

**Core value:** Developers and AI agents can instantly understand any codebase — files auto-indexed, agents query the DB to reduce token usage and improve quality.
**Current focus:** v0.6 Phase 2 — Daemon central (Unix socket, LRU cache, shared embeddings model).

## Current Position

Milestone: v0.6 Daemon & Centralisation
Phase: 2 of 3 (Daemon central)
Plan: Not started
Status: Ready to plan Phase 2
Last activity: 2026-03-02 — Phase 1 complete (Centralisation du stockage), 782 tests pass

Progress:
- v0.6 Daemon & Centralisation: [███░░░░░░░] 33% (1/3 phases)

## Loop Position

Current loop state:
```
PLAN ──▶ APPLY ──▶ UNIFY
  ✓        ✓        ✓     [Phase 1 loop closed — ready for Phase 2 PLAN]
```

## Accumulated Context

### Decisions
| Decision | Phase | Impact |
|----------|-------|--------|
| storage_load is 98%+ of indexing time | v0.4 Plan 01-01 | Future perf work must target KuzuDB bulk_load |
| Async embeddings via ThreadPoolExecutor | v0.4 Plan 01-01 | Pipeline returns immediately, embeddings in background |
| KuzuDB has no specific exception types | v0.4 Plan 01-02 | All except blocks use RuntimeError |
| kuzu_backend split into submodules | v0.4 Plan 01-02 | Shared constants in kuzu_backend.py, imported by submodules |
| events.jsonl global at ~/.axon/events.jsonl | v0.4 Plan 01-04 | One log for all repos on the machine |
| log_event() never raises (BLE001 catch-all) | v0.4 Plan 01-04 | Analytics failure never blocks main flow |
| repo= opens/closes KuzuBackend per request | v0.4 Plan 01-04 | Safe for read-only, avoids connection leaks |
| KuzuDB creates a single FILE (not directory) | v0.5 Plan 01-02 | Template copies use shutil.copy2, not copytree |
| Watcher embeddings on EMBEDDING_INTERVAL (60s) | v0.5 Plan 01-02 | _run_global_phases(embeddings=False); last_embed timer added |
| test_watcher.py floor is ~28s | v0.5 Plan 01-02 | KuzuDB open ~1.3s/test; accepted, not worth mocking |
| USES is a distinct RelType (not USES_TYPE) | v0.5 Plan 02-01 | Elixir `use` is macro injection, different from type usage |
| Community detection via WCC + ThreadPoolExecutor | v0.5 Plan 02-02 | Per-component Leiden; small (<3 node) components skipped |
| Central KuzuDB at ~/.axon/repos/{slug}/kuzu | v0.6 Plan 01-01 | One storage location per repo, independent of project dir |
| Placeholder meta.json before KuzuDB init | v0.6 Plan 01-01 | Prevents _register_in_global_registry from deleting central slot |
| Auto-migration via shutil.move on analyze | v0.6 Plan 01-01 | Transparent migration for existing repos, no manual step |

### Deferred Issues
| Issue | Origin | Effort | Revisit |
|-------|--------|--------|---------|
| test_watcher.py at 28s (target was 15s) | v0.5 Plan 01-02 | S | accept as-is |
| cohesion: 0.0 placeholder in communities | v0.5 Plan 02-02 | S | revisit if per-component modularity needed |

### Blockers/Concerns
None.

### Git State
Last commit: 1abcb61 (chore(plan): create v0.6 Phase 1 plan — centralisation du stockage)
Tag: v0.5.0
Branch: main
Uncommitted: cli/main.py, mcp/server.py, mcp/tools.py, tests/cli/test_main.py, tests/mcp/test_tools.py, 01-01-SUMMARY.md

## Session Continuity

Last session: 2026-03-02
Stopped at: /paul:unify — Phase 1 loop closed, git commit pending
Next action: /paul:plan for Phase 2 (Daemon central)
Resume file: .paul/ROADMAP.md
Resume context:
- Phase 1 complete: KuzuDB centralised at ~/.axon/repos/{slug}/kuzu
- All phase 1 code committed as feat(01-centralisation-stockage)
- Phase 2 focus: axon daemon start/stop/status, Unix socket, LRU cache 5 DBs

---
*STATE.md — Updated after every significant action*
