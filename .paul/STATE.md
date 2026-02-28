# Project State

## Project Reference

See: .paul/PROJECT.md (updated 2026-02-27 after v0.4 complete)

**Core value:** Developers and AI agents can instantly understand any codebase — files auto-indexed, agents query the DB to reduce token usage and improve quality.
**Current focus:** v0.5 Hardening — Phase 2: Elixir `use` parser + community detection parallelization

## Current Position

Milestone: v0.5 Hardening
Phase: 2 of 2 — Parser & Performance
Plan: none yet — ready for /paul:plan
Status: Phase 1 complete, Phase 2 not started
Last activity: 2026-02-28 — Phase 1 complete (2 plans), watcher hotfix applied

Progress:
- v0.5 Hardening: [█████░░░░░] 50%
- Phase 2: [░░░░░░░░░░] 0%

## Loop Position

Current loop state:
```
PLAN ──▶ APPLY ──▶ UNIFY
  ✓        ✓        ✓     [Phase 1 complete — ready for Phase 2 PLAN]
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

### Deferred Issues
| Issue | Origin | Effort | Revisit |
|-------|--------|--------|---------|
| Community detection (19% cold-start) still sequential | v0.3 Phase 2 | M | v0.5 Phase 2 ← active |
| test_watcher.py at 28s (target was 15s) | v0.5 Plan 01-02 | S | accept as-is |

### Blockers/Concerns
None.

### Git State
Last commit: 3f731e2 (docs: pause — handoff after v0.4 milestone complete)
Tag: v0.4.0
Branch: main
Uncommitted: Phase 1 changes (test fixtures, watcher hotfix) — commit pending

## Session Continuity

Last session: 2026-02-28
Stopped at: Phase 1 UNIFY complete, ready to start Phase 2
Next action: /paul:plan (Phase 2 — Parser & Performance)
Resume file: .paul/phases/01-test-quality-bugs/01-02-SUMMARY.md
Resume context:
- Phase 1 done: test isolation ✓, pipeline 81s ✓, watcher 28s ✓, watcher hotfix ✓
- Uncommitted changes from Phase 1 need git commit before Phase 2
- Phase 2 targets: Elixir `use` parser + community detection parallelization

---
*STATE.md — Updated after every significant action*
