# axon

## What This Is

Axon is a code intelligence tool that indexes any codebase and exposes it for semantic search. This development effort extends Axon's capacity to handle any project at any scale, with automatic file watching and re-indexing, so that AI agents can query the database instead of reading raw files — reducing token usage and improving response quality. The goal is seamless integration into daily development workflows, including massive multi-language projects.

## Core Value

Developers and AI agents can instantly understand and navigate any codebase — files are automatically indexed on change, and AI agents query this database to reduce token usage, context overhead, and improve response quality.

## Current State

| Attribute | Value |
|-----------|-------|
| Version | 0.5.0 |
| Status | Active Development |
| Last Updated | 2026-02-28 |

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

### Active (In Progress)
(None — v0.5 complete)

### Planned (Next)
(To be defined for v0.6)

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
*Last updated: 2026-02-28 — v0.5 Hardening complete (2 phases, 4 plans shipped)*
