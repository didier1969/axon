# Project State

## Project Reference

See: .paul/PROJECT.md (updated 2026-02-28 after v0.5 complete)

**Core value:** Developers and AI agents can instantly understand any codebase — files auto-indexed, agents query the DB to reduce token usage and improve quality.
**Current focus:** v0.9 Language Coverage — new milestone, ready to plan.

## Current Position

Milestone: v0.9 Language Coverage
Phase: Not started
Plan: Not started
Status: v0.8 complete — ready to plan v0.9
Last activity: 2026-03-07 — v0.8 complete (UNIFY 02-04 + phase transition; 936 tests)

Progress:
- v0.7 Quality & Security: [██████████] 100% ✓
- v0.8 Graph Intelligence & Search Quality: [██████████] 100% ✓
- v0.9 Language Coverage: [░░░░░░░░░░] 0% (not started)

## Loop Position

Current loop state:
```
PLAN ──▶ APPLY ──▶ UNIFY
  ○        ○        ○     [v0.9 — new milestone, ready to plan]
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
| sql_lang.py / yaml_lang.py left at start_byte=0 | v0.6 Plan 03-03 | Regex-based parsers have no tree-sitter node — RESOLVED in v0.7 Plan 02-01 |
| SQL: char offset == byte offset (ASCII assumption) | v0.7 Plan 02-01 | start_byte=m.start(), end_byte=find(';')+1 — accurate for ASCII SQL files |
| YAML: precompute line_start_bytes[] | v0.7 Plan 02-01 | Single pass before loop, passed to _parse_yaml/_parse_toml — UTF-8 accurate |
| axon_read_symbol fallback on start_byte==0 | v0.7 Plan 02-01 | Returns stored content field with note when byte offsets unavailable |
| _sanitize_repo_slug() as security gate for repo= param | v0.7 Plan 01-01 | Rejects traversal, null bytes, spaces, >200 chars — applied in _load_repo_storage() |
| execute_raw(parameters=dict) parameterized queries | v0.7 Plan 01-01 | Eliminates Cypher injection and N+1 in handle_detect_changes |
| Drop-oldest strategy for bounded watcher queue | v0.7 Plan 01-01 | asyncio.Queue(maxsize=100); newest events preserved on overflow |
| _make_snippet() semantic truncation | v0.7 Plan 01-01 | 400-char limit, newline-aware, signature-preferred; replaces content[:200] |
| Count-before-delete in remove_nodes_by_file | v0.7 Plan 01-01 | KuzuDB lacks DETACH DELETE…RETURNING; COUNT per table then DELETE |
| Python wildcard import: no bug, regression test added | v0.8 Plan 01-01 | names=['*'] correctly creates IMPORTS edge |
| _extract_class_heritage() unified for generic base types | v0.8 Plan 01-01 | Reuses _extract_generic_arg_refs for extends Base<User> |
| isinstance(float) guard in hybrid centrality boost | v0.8 Plan 01-02 | MagicMock.centrality truthy but not float — guard prevents corrupted RRF scores |
| kuzu_search._row_to_node 12-col bug fixed | v0.8 Plan 01-02 | Pre-existing: start_byte was reading content slot; fixed with explicit column guards |
| test_coverage before dead_code in pipeline | v0.8 Plan 01-02 | Future: tested+dead = refactor candidate signal |
| DEPENDS_ON exact slug-name matching only | v0.8 Plan 02-04 | Dep name must match ~/.axon/repos/ dir name exactly; no fuzzy match |
| DEPENDS_ON placeholder File nodes | v0.8 Plan 02-04 | file_path='dep:{name}' — anchor for edge; no symbol children |
| Cross-repo phase omitted from reindex_files() | v0.8 Plan 02-04 | DEPENDS_ON edges stable across incremental re-index; full analyze required |

### Deferred Issues
| Issue | Origin | Effort | Revisit |
|-------|--------|--------|---------|
| test_watcher.py at 28s (target was 15s) | v0.5 Plan 01-02 | S | accept as-is |
| cohesion: 0.0 placeholder in communities | v0.5 Plan 02-02 | S | ✓ RESOLVED in v0.8 02-02 (real intra-edge density) |
| No tests for byte offsets or axon_read_symbol | v0.7 Plan 02-01 | S | ✓ RESOLVED in 02-02 |

### Blockers/Concerns
None.

### Git State
Last commit: 9bc5726 (feat(02-mcp-tools-dx): complete — Phase 2 of v0.8)
Branch: main
Uncommitted: none (clean)

## Session Continuity

Last session: 2026-03-07
Stopped at: v0.8 complete — UNIFY 02-04 done, phase transition done, milestone ready; phase commit pending
Next action: /paul:milestone for v0.9 (or /paul:plan for first v0.9 phase after milestone init)
Resume file: .paul/ROADMAP.md
Resume context: 936 tests passing; v0.8 all shipped; pending: phase commit feat(02-mcp-tools-dx): complete

---
*STATE.md — Updated after every significant action*
