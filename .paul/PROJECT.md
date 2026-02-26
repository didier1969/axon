# axon

## What This Is

Axon is a code intelligence tool that indexes any codebase and exposes it for semantic search. This development effort extends Axon's capacity to handle any project at any scale, with automatic file watching and re-indexing, so that AI agents can query the database instead of reading raw files — reducing token usage and improving response quality. The goal is seamless integration into daily development workflows, including massive multi-language projects.

## Core Value

Developers and AI agents can instantly understand and navigate any codebase — files are automatically indexed on change, and AI agents query this database to reduce token usage, context overhead, and improve response quality.

## Current State

| Attribute | Value |
|-----------|-------|
| Version | 0.3.0 |
| Status | Active Development |
| Last Updated | 2026-02-27 |

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

### Active (In Progress)
(None — milestone v0.3 complete)

### Planned (Next)
(To be defined in next milestone)

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

## Success Metrics

| Metric | Target | Current | Status |
|--------|--------|---------|--------|
| Languages supported | 10+ | 6 (Python, TS, JS, Elixir, Rust, Markdown) | In progress |
| Large project indexing | <60s for 100k LOC | Warm: ~8ms; Cold: ~0.89s (85 files) | In progress |
| Workflow integration | Zero-friction on session start | shell-hook + direnv + CI templates + docs | Shipped |

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
*Last updated: 2026-02-27 after Phase 3 (Workflow Integration) — Milestone v0.3 complete*
