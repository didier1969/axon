# Project State

## Project Reference

See: .paul/PROJECT.md (updated 2026-02-28 after v0.5 complete)

**Core value:** Developers and AI agents can instantly understand any codebase — files auto-indexed, agents query the DB to reduce token usage and improve quality.
**Current focus:** v0.6 Phase 3 complete — ready for milestone completion and v0.7.

## Current Position

Milestone: v0.6 Daemon & Centralisation
Phase: 3 of 3 (Watch & filtrage) — COMPLETE
Plan: 03-03 APPLY complete (3/3 plans done)
Status: Phase complete — all 3 plans applied
Last activity: 2026-03-02 — Completed .paul/phases/03-watch-filtrage/03-03-PLAN.md

Progress:
- v0.6 Daemon & Centralisation: [██████████] 100% (3/3 phases)
- Phase 3: [██████████] 100% (3/3 plans complete)

## Loop Position

Current loop state:
```
PLAN ──▶ APPLY ──▶ UNIFY
  ✓        ✓        ○     [03-03 applied; UNIFY + milestone completion pending]
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
| MCP proxy routes via daemon, fallback to direct | v0.6 Plan 02-02 | N×~10MB proxy processes share single ~200MB daemon |
| max_tokens truncation is MCP-side only | v0.6 Plan 02-02 | Applied after daemon result or direct fallback; daemon_args stripped |
| Byte offsets stored as INT64 in KuzuDB, no migration | v0.6 Plan 03-03 | Users must re-run axon analyze; old 12-col schemas still readable |
| markdown sections use heading node.start_byte as section start | v0.6 Plan 03-03 | end_byte = next heading start_byte - 1; content assembled from lines |
| sql_lang.py / yaml_lang.py left at start_byte=0 | v0.6 Plan 03-03 | Regex-based parsers have no tree-sitter node |

### Deferred Issues
| Issue | Origin | Effort | Revisit |
|-------|--------|--------|---------|
| test_watcher.py at 28s (target was 15s) | v0.5 Plan 01-02 | S | accept as-is |
| cohesion: 0.0 placeholder in communities | v0.5 Plan 02-02 | S | revisit if per-component modularity needed |

### Blockers/Concerns
None.

### Git State
Last commit: b439476 (feat(03-watch-filtrage): 03-03 task 2 — propagate byte offsets in all tree-sitter parsers)
Branch: main
Uncommitted: PAUL files (STATE.md, SUMMARY 03-03) pending docs commit

## Session Continuity

Last session: 2026-03-02T19:50Z
Stopped at: 03-03 APPLY complete — byte-offset caching
Next action: /paul:unify → then /paul:complete-milestone → v0.7
Resume file: .paul/phases/03-watch-filtrage/03-03-SUMMARY.md
Resume context:
- 03-03 adds start_byte/end_byte to SymbolInfo, GraphNode, KuzuDB schema + 8 parsers
- Full suite: 824 tests, 0 failures, 0 ruff errors
- UNIFY step pending, then milestone tag v0.6.0

---
*STATE.md — Updated after every significant action*
