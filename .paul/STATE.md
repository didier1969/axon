# Project State

## Project Reference

See: .paul/PROJECT.md (updated 2026-02-28 after v0.5 complete)

**Core value:** Developers and AI agents can instantly understand any codebase — files auto-indexed, agents query the DB to reduce token usage and improve quality.
**Current focus:** v0.6 Phase 2 — Daemon central (Unix socket, LRU cache, shared embeddings model).

## Current Position

Milestone: v0.6 Daemon & Centralisation
Phase: 2 of 3 (Daemon central) — In Progress
Plan: 02-02 created, awaiting APPLY
Status: PLAN created, ready for APPLY
Last activity: 2026-03-02 — Created 02-02-PLAN.md (MCP proxy + max_tokens)

Progress:
- v0.6 Daemon & Centralisation: [███░░░░░░░] 33% (1/3 phases)
- Phase 2: [███░░░░░░░] 33% (1/3 plans complete)

## Loop Position

Current loop state:
```
PLAN ──▶ APPLY ──▶ UNIFY
  ✓        ○        ○     [Plan 02-02 created, awaiting approval]
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
| Double-checked locking in LRU cache | v0.6 Plan 02-01 | KuzuBackend.initialize() I/O outside lock; insert+evict inside lock |
| Popen(start_new_session=True) for daemon spawn | v0.6 Plan 02-01 | Portable orphan process; no os.fork() complexity |
| MCP proxy deferred to Plan 02-02 | v0.6 Plan 02-01 | Daemon exists but MCP still uses direct KuzuBackend |

### Deferred Issues
| Issue | Origin | Effort | Revisit |
|-------|--------|--------|---------|
| test_watcher.py at 28s (target was 15s) | v0.5 Plan 01-02 | S | accept as-is |
| cohesion: 0.0 placeholder in communities | v0.5 Plan 02-02 | S | revisit if per-component modularity needed |

### Blockers/Concerns
None.

### Git State
Last commit: be2bad5 (feat(02-daemon-central): implement axon daemon package and CLI commands)
Tag: v0.5.0
Branch: main
Uncommitted: stale handoffs (.paul/HANDOFF-2026-02-28e.md, HANDOFF-2026-03-02.md, HANDOFF-2026-03-02f.md) + uv.lock minor change

## Session Continuity

Last session: 2026-03-02
Stopped at: /paul:pause — Plan 02-02 created, APPLY pending
Next action: /paul:apply .paul/phases/02-daemon-central/02-02-PLAN.md
Resume file: .paul/HANDOFF-2026-03-02j.md
Resume context:
- Plan 02-02 READY: MCP proxy (_get_local_slug + _try_daemon_call in server.py), fallback to direct, max_tokens on all 7 tools
- Daemon args: strip repo= and max_tokens= before sending to socket; slug = repo or local slug
- New tests: tests/mcp/test_server.py (proxy happy path, fallback, max_tokens, schema)
- Plan 02-03 pending: axon_batch tool
- v0.7 planned: BM25 hybrid search (FTS5 + vector + graph distance)

---
*STATE.md — Updated after every significant action*
