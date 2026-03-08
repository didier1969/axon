# axon

## What This Is

Axon is a code intelligence tool that indexes any codebase and exposes it for semantic search. This development effort extends Axon's capacity to handle any project at any scale, with automatic file watching and re-indexing, so that AI agents can query the database instead of reading raw files — reducing token usage and improving response quality. The goal is seamless integration into daily development workflows, including massive multi-language projects.

## Core Value

Developers and AI agents can instantly understand and navigate any codebase — files are automatically indexed on change, and AI agents query this database to reduce token usage, context overhead, and improve response quality.

## Current State

| Attribute | Value |
|-----------|-------|
| Version | 1.0.0 |
| Status | Active Development |
| Last Updated | 2026-03-07 |

## Requirements

### Validated (Shipped)
- [x] Core parsing and indexing pipeline — v0.2.x
- [x] File watcher for automatic re-indexing — v0.2.x
- [x] Multi-language parser support (Python, JS/TS, etc.) — v0.2.x
- [x] Ignore patterns and language configuration — v0.2.x
- [x] Extended language support (Elixir, Rust, Markdown) — Phase 1
- [x] Incremental indexing (content-hash delta, warm starts ~8ms) — Phase 2
- [x] CPU-adaptive parallel parsing (walk + parse scale with hardware) — Phase 2
- [x] Shell integration (shell-hook + direnv auto-start on cd) — Phase 3
- [x] CI integration (dead-code --exit-code gate, GitHub Actions + pre-commit templates) — Phase 3
- [x] MCP query ergonomics (language filter, file:symbol disambiguation, deprecation warnings) — Phase 3
- [x] Developer documentation (README updated, getting-started guide) — Phase 3
- [x] Performance optimization (batch Cypher inserts, async embeddings, profiling) — v0.4 Phase 1, Plan 01-01
- [x] Code quality consolidation (exception specificity, kuzu_backend split, version 0.4.0) — v0.4 Phase 1, Plan 01-02
- [x] Markdown parser upgrade (tree-sitter, frontmatter, pipe tables) — v0.4 Phase 1, Plan 01-03
- [x] New language parsers (Go, YAML/TOML, SQL, HTML, CSS → 12 total) — v0.4 Phase 1, Plan 01-03
- [x] Multi-repo intelligence (cross-repo MCP queries via optional repo= param) — v0.4 Phase 1, Plan 01-04
- [x] Usage analytics (events.jsonl logging + axon stats CLI command) — v0.4 Phase 1, Plan 01-04
- [x] Test infrastructure hardened (isolation fixture, session-scoped KuzuDB templates, async race fix) — v0.5 Phase 1
- [x] Watcher production-safe (embeddings on 60s interval, not 30s) — v0.5 Phase 1, Plan 01-02
- [x] Elixir `use Module` → USES relationship in graph — v0.5 Phase 2, Plan 02-01
- [x] Community detection parallelized (WCC + ThreadPoolExecutor) — v0.5 Phase 2, Plan 02-02
- [x] Central KuzuDB storage at ~/.axon/repos/{slug}/kuzu — v0.6 Phase 1
- [x] Auto-migration of legacy local KuzuDB on axon analyze — v0.6 Phase 1
- [x] Slug-based repo identity in local meta.json — v0.6 Phase 1
- [x] Backward-compat fallback for pre-v0.6 repos (no slug in meta.json) — v0.6 Phase 1
- [x] Daemon central (axon daemon start/stop/status, Unix socket, LRU cache, MCP proxy) — v0.6 Phase 2
- [x] MCP proxy routes via daemon, fallback to direct (N×~10MB → single ~200MB daemon) — v0.6 Phase 2
- [x] axon_batch tool: N calls on 1 socket connection, daemon-first with direct fallback — v0.6 Phase 2
- [x] Watch filter (.paul/.git/.axon ignored), configurable debounce (--debounce CLI flag) — v0.6 Phase 3, Plan 03-01
- [x] Sequential watcher queue (asyncio.Queue producer/consumer, no producer stall under MCP lock) — v0.6 Phase 3, Plan 03-02
- [x] Byte-offset caching (start_byte/end_byte in SymbolInfo, GraphNode, KuzuDB schema) — v0.6 Phase 3, Plan 03-03

- [x] Security hardening Phase 1: path traversal, Cypher injection, DDL bypass, race condition, socket permissions — v0.7 Phase 1
- [x] Snippet quality improvement: `_make_snippet()` 400-char semantic truncation — v0.7 Phase 1
- [x] Callers/callees output cap (20 items + overflow) — v0.7 Phase 1
- [x] Bounded watcher queue (maxsize=100, drop-oldest) — v0.7 Phase 1
- [x] `remove_nodes_by_file()` correct return count — v0.7 Phase 1
- [x] Atomic meta.json writes via tempfile + os.replace() — v0.7 Phase 1

- [x] `axon_read_symbol` MCP tool — O(1) via start_byte/end_byte, exact source slice — v0.7 Phase 2, Plan 02-01
- [x] Byte offsets for sql_lang.py + yaml_lang.py (regex span(), precomputed line_start_bytes[]) — v0.7 Phase 2, Plan 02-01
- [x] Parser quality fixes + test coverage for byte offsets and axon_read_symbol — v0.7 Phase 2, Plan 02-02
- [x] File size limit 512KB in walker.py (OOM protection, warning logged) — v0.7 Phase 2, Plan 02-03
- [x] `compute_repo_slug()` helper extracted to core/paths.py (deduplicates 3 CLI call sites) — v0.7 Phase 2, Plan 02-03
- [x] axon_batch partial failure summary `[BATCH WARNING: K/N failed: indices [...]]` — v0.7 Phase 2, Plan 02-04
- [x] `AXON_LRU_SIZE` env var for daemon LRU maxsize (CLI `--max-dbs` overrides) — v0.7 Phase 2, Plan 02-04
- [x] Socket buffer: `makefile("rb").readline()` replaces `recv(4096)` loop — v0.7 Phase 2, Plan 02-04

- [x] TypeScript USES_TYPE edges for class properties, interface members, generic base classes — v0.8 Phase 1, Plan 01-01
- [x] Dead code test pattern exemptions (Go `_test.go`, `spec/` root paths) — v0.8 Phase 1, Plan 01-01
- [x] Python wildcard imports `from x import *` confirmed correct (regression test added) — v0.8 Phase 1, Plan 01-01
- [x] `tested: bool` on GraphNode — marked via CALLS/IMPORTS from test files — v0.8 Phase 1, Plan 01-02
- [x] `centrality: float` on GraphNode — PageRank on CALLS+IMPORTS graph (igraph) — v0.8 Phase 1, Plan 01-02
- [x] Centrality boost in hybrid search (`score * (1 + centrality)`) — v0.8 Phase 1, Plan 01-02
- [x] Code-aware embedding text (source snippet in `generate_text()` for function/class nodes) — v0.8 Phase 1, Plan 01-03
- [x] `axon_find_similar` MCP tool — semantic duplicate detection via stored embeddings — v0.8 Phase 1, Plan 01-03
- [x] Attribute surfacing in `axon_context` (tested, exported, centrality) and `axon_query` (`[exported]`/`[untested]` tags) — v0.8 Phase 1, Plan 01-03
- [x] Query expansion via `AXON_QUERY_EXPAND` env var (synonym heuristic, off by default) — v0.8 Phase 1, Plan 01-03

- [x] `axon_find_usages` MCP tool — exhaustive call-site search across repo — v0.8 Phase 2, Plan 02-01
- [x] MCP tool descriptions improved — expected input formats documented for all tools — v0.8 Phase 2, Plan 02-01
- [x] `axon_lint` MCP tool — structural rules: high coupling, god classes, IMPORTS cycles — v0.8 Phase 2, Plan 02-02
- [x] Community cohesion metric — real intra-edge density replaces placeholder 0.0 — v0.8 Phase 2, Plan 02-02
- [x] `axon_summarize` MCP tool — LLM-ready file/module/class summary with symbol inventory — v0.8 Phase 2, Plan 02-03
- [x] Multi-repo DEPENDS_ON edges — manifest parsing (pyproject.toml, package.json, go.mod) links indexed repos — v0.8 Phase 2, Plan 02-04
- [x] `axon analyze --progress` / `AXON_ANALYZE_PROGRESS` — phase-completion output to stderr for CI/scripting — v0.8 Phase 2, Plan 02-04

### Active (In Progress)
None — v0.8 complete.

### Planned (Next)

### Out of Scope
- GUI / web interface — CLI and MCP-first

## Target Users

**Primary:** Developers using AI coding assistants (Claude Code, Cursor, etc.)
- Work on large, multi-language codebases
- Want AI agents to understand their codebase without wasting tokens
- Integrate tools into daily dev workflow

**Secondary:** AI agents themselves (automated querying)

## Constraints

### Technical Constraints
- Python package (pyproject.toml / uv)
- Must support incremental indexing (file-level updates, not full re-index)
- MCP server interface for AI agent consumption

### Business Constraints
- Solo developer project — keep complexity manageable

## Key Decisions

| Decision | Rationale | Date | Status |
|----------|-----------|------|--------|
| Python as language | Ecosystem fit for parsing/embeddings | - | Active |
| Tree-sitter for parsing | Multi-language, robust AST | - | Active |
| Markdown headings → NodeLabel.FUNCTION | Reuses existing graph label, no schema change | 2026-02-26 | Active |
| Elixir module → NodeLabel.CLASS | Modules are units of encapsulation, analogous to classes | 2026-02-26 | Active |
| Rust struct/enum/trait → NodeLabel.CLASS | Type-defining constructs unified under CLASS for simpler queries | 2026-02-26 | Active |
| Content hash (sha256) for incremental diff | Reliable across copies/moves; content already in memory | 2026-02-26 | Active |
| max_workers=None → ThreadPoolExecutor default | Let stdlib pick min(32, cpu_count+4); no app-level os.cpu_count() | 2026-02-26 | Active |
| storage_load is 98%+ of indexing time | Future perf work must target KuzuDB bulk_load, not pipeline phases | 2026-02-27 | Active |
| Async embeddings via ThreadPoolExecutor | Pipeline returns immediately, embeddings continue in background | 2026-02-27 | Active |
| KuzuDB raises plain RuntimeError | No kuzu-specific exception type exists; all except blocks use RuntimeError | 2026-02-27 | Active |
| events.jsonl global at ~/.axon/events.jsonl | One log for all repos on the machine; consistent with global registry | 2026-02-27 | Active |
| log_event() never raises | Analytics failure must never block main flow; BLE001 catch-all | 2026-02-27 | Active |
| repo= opens/closes KuzuBackend per request | No caching needed for read-only queries; avoids connection leaks | 2026-02-27 | Active |
| KuzuDB creates a single FILE (not directory) | shutil.copy2 for template copies in tests | 2026-02-28 | Active |
| Watcher embeddings on 60s EMBEDDING_INTERVAL | _run_global_phases(embeddings=False); last_embed timer enforces 60s cadence | 2026-02-28 | Active |
| USES is a distinct RelType (not USES_TYPE) | Elixir `use` is macro injection — semantically different from type usage | 2026-02-28 | Active |
| Community detection: ThreadPoolExecutor default | Let stdlib pick min(32, cpu_count+4) per WCC component | 2026-02-28 | Active |
| Central KuzuDB at ~/.axon/repos/{slug}/kuzu | All DBs in one place; daemon Phase 2 can implement LRU cache over this directory | 2026-03-02 | Active |
| Placeholder meta.json before KuzuDB init | _register_in_global_registry deletes slots without meta.json; placeholder prevents rmtree | 2026-03-02 | Active |
| Slug computation inlined (not extracted to helper) | 3 call sites; minimal blast radius; no shared state needed | 2026-03-02 | Active |
| Double-checked locking in LRU cache | KuzuBackend.initialize() I/O outside lock; insert+evict inside lock — avoids holding lock during disk I/O | 2026-03-02 | Active |
| MCP proxy: daemon-first, fallback to direct | N MCP proxy processes (~10MB each) share single ~200MB daemon; max_tokens truncation is MCP-side | 2026-03-02 | Active |
| axon_batch is MCP-layer only | Daemon receives individual calls; axon_batch is transparent to the daemon protocol | 2026-03-02 | Active |
| debounce_ms exposed as CLI param | --debounce flag on axon serve --watch; default 50ms; configures watchfiles rust_timeout | 2026-03-02 | Active |
| asyncio.Queue producer/consumer in watch_repo() | _producer never stalls under MCP lock; _consumer drains sequentially; None sentinel for shutdown | 2026-03-02 | Active |
| Byte offsets in schema, no migration | start_byte/end_byte INT64 in all node tables; old 12-col DBs still readable with len(row) guard | 2026-03-02 | Active |
| _sanitize_repo_slug() as security gate | All filesystem path construction via repo= param now validated before use | 2026-03-02 | Active |
| Parameterized KuzuDB queries | execute_raw(query, parameters={"key": val}) — no f-string Cypher, no N+1 | 2026-03-02 | Active |
| Drop-oldest strategy for watcher queue | maxsize=100; overflow drops oldest batch (most recent = most relevant) | 2026-03-02 | Active |
| _make_snippet() semantic truncation | 400-char limit, newline-aware, signature-preferred; replaces content[:200] slice | 2026-03-02 | Active |
| isinstance(float) guard in hybrid centrality boost | MagicMock.centrality is truthy but not float — guard prevents corrupting RRF scores in tests | 2026-03-05 | Active |
| kuzu_search._row_to_node fixed: 12-col offsets vs 14-col schema | Pre-existing bug caused content to read start_byte value; fixed with explicit column guards | 2026-03-05 | Active |
| test_coverage phase before dead_code | Allows future: tested+dead_code = refactor candidate signal | 2026-03-05 | Active |
| Python wildcard import: no bug found | names=['*'] correctly creates IMPORTS edge; regression test added as guard | 2026-03-05 | Active |
| DEPENDS_ON exact slug-name matching | Dep name must match ~/.axon/repos/ dir name exactly; no fuzzy match or registry lookup | 2026-03-07 | Active |
| DEPENDS_ON placeholder File nodes | file_path='dep:{name}' anchors edges without full repo context; queryable but no symbol children | 2026-03-07 | Active |
| Cross-repo phase omitted from reindex_files() | DEPENDS_ON edges stable across incremental re-index; full axon analyze required to refresh | 2026-03-07 | Active |
| go.mod last path segment only | github.com/gin-gonic/gin → "gin"; intentionally imprecise — sufficient for slug matching | 2026-03-07 | Active |

## Success Metrics

| Metric | Target | Current | Status |
|--------|--------|---------|--------|
| Languages supported | 10+ | 12 (Python, TS, JS, Elixir, Rust, Markdown, Go, YAML/TOML, SQL, HTML, CSS) | ✅ Shipped |
| Large project indexing | <60s for 100k LOC | Warm: ~8ms; Cold: ~0.89s (85 files) | ✅ Shipped |
| Workflow integration | Zero-friction on session start | shell-hook + direnv + CI templates + docs | ✅ Shipped |

## Tech Stack

| Layer | Technology | Notes |
|-------|------------|-------|
| Language | Python 3.12+ | |
| Package manager | uv | |
| Parsing | Tree-sitter | Multi-language AST |
| Watching | watchdog | File system events |
| Interface | MCP server | AI agent integration |

---
*PROJECT.md — Updated when requirements or context change*
*Last updated: 2026-03-07 — v0.8 complete (Graph Intelligence + MCP Tools & DX: 7 plans, 936 tests)*
