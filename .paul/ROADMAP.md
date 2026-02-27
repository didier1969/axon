# Roadmap: axon

## Overview

Axon evolves from a functional code indexer into a production-grade tool that seamlessly handles any project at any scale. The roadmap prioritises language coverage, large-project performance, and frictionless integration into daily development workflows — making it the default intelligence layer for AI-assisted development.

## Current Milestone

**v0.4 Consolidation & Scale** (v0.4.0)
Status: In progress
Phases: 0 of 1 complete (2/6 plans done)

## Milestones

### v0.3 Workflow Integration — ✅ Complete (2026-02-27)

| Phase | Name | Plans | Status | Completed |
|-------|------|-------|--------|-----------|
| 1 | Language Coverage | 1 | ✅ Complete | 2026-02-26 |
| 2 | Large Project Performance | 3 | ✅ Complete | 2026-02-26 |
| 3 | Workflow Integration | 4 | ✅ Complete | 2026-02-27 |

### v0.4 Consolidation & Scale — 🔵 In Progress

| Phase | Name | Plans | Status | Completed |
|-------|------|-------|--------|-----------|
| 1 | Consolidation & Scale | 6 | Planning | - |

## Phase Details (v0.4)

### Phase 1: Consolidation & Scale

**Goal:** Harden performance, improve code quality, extend language coverage to 10, and enable cross-repo intelligence
**Depends on:** v0.3 complete
**Research:** Likely (Go/YAML/SQL parsers, multi-repo query patterns, batch Cypher strategies)

**Scope:**
- Performance: batch Cypher inserts, async embeddings, profile & optimize slow repos
- Code quality: fix bare except handlers, split kuzu_backend.py, version bump
- Markdown parser: upgrade from regex to tree-sitter, add frontmatter/tables
- New languages: Go, YAML/TOML, SQL, HTML/CSS (reach 10 total)
- Multi-repo: cross-repo MCP queries
- Usage analytics: event logging for MCP queries and pipeline runs, `axon stats` CLI command

**Plans:**
- [x] 01-01: Performance optimization (batch inserts, async embeddings, profiling)
- [x] 01-02: Code quality consolidation (error handling, kuzu_backend split, version bump)
- [ ] 01-03: Markdown parser upgrade (tree-sitter, frontmatter, tables)
- [ ] 01-04: New language parsers (Go, YAML/TOML, SQL, HTML/CSS)
- [ ] 01-05: Multi-repo intelligence (cross-repo MCP queries)
- [ ] 01-06: Usage analytics (event logging, `axon stats` command, per-project query/indexing counts)

---
*Roadmap created: 2026-02-26*
*Last updated: 2026-02-27 — Plan 01-02 complete (code quality consolidation)*
