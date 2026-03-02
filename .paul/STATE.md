# Project State

## Project Reference

See: .paul/PROJECT.md (updated 2026-02-28 after v0.5 complete)

**Core value:** Developers and AI agents can instantly understand any codebase — files auto-indexed, agents query the DB to reduce token usage and improve quality.
**Current focus:** Awaiting next milestone — v0.5 Hardening complete.

## Current Position

Milestone: v0.6 Daemon & Centralisation
Phase: 1 of 3 (Centralisation du stockage)
Plan: Not started
Status: Ready to plan
Last activity: 2026-03-02 — Milestone v0.6 created

Progress:
- v0.6 Daemon & Centralisation: [░░░░░░░░░░] 0%

## Loop Position

Current loop state:
```
PLAN ──▶ APPLY ──▶ UNIFY
  ○        ○        ○     [Ready for first PLAN]
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

### Deferred Issues
| Issue | Origin | Effort | Revisit |
|-------|--------|--------|---------|
| test_watcher.py at 28s (target was 15s) | v0.5 Plan 01-02 | S | accept as-is |
| cohesion: 0.0 placeholder in communities | v0.5 Plan 02-02 | S | revisit if per-component modularity needed |

### Blockers/Concerns
None.

### Git State
Last commit: e5495f4 (feat(02-parser-and-performance): Elixir USES + community parallelization)
Tag: v0.5.0
Branch: main
Uncommitted: .paul docs only (milestone completion docs)

## Session Continuity

Last session: 2026-03-02
Stopped at: /paul:pause — v0.6 créé, prêt pour Phase 1
Next action: /paul:plan → Phase 1 Centralisation du stockage
Resume file: .paul/HANDOFF-2026-03-02.md
Resume context:
- KuzuDB à migrer de {project}/.axon/kuzu vers ~/.axon/repos/{name}/kuzu
- axon-mcp wrapper actif dans ~/.local/bin/axon-mcp (MCP only, pas de watchers)
- nexus à réindexer après migration (DB supprimée car corrompue)
- Engram/Mem0/code-search MCPs supprimés, 3 restants : playwright, chrome-devtools, axon

---
*STATE.md — Updated after every significant action*
