# Roadmap: axon

## Overview

Axon evolves from a functional code indexer into a production-grade tool that seamlessly handles any project at any scale. The roadmap prioritises language coverage, large-project performance, and frictionless integration into daily development workflows — making it the default intelligence layer for AI-assisted development.

## Current Milestone

**v0.4 Consolidation & Scale** (v0.4.0)
Status: ✅ Complete
Phases: 1 of 1 complete (4/4 plans done)

## Milestones

### v0.3 Workflow Integration — ✅ Complete (2026-02-27)

| Phase | Name | Plans | Status | Completed |
|-------|------|-------|--------|-----------|
| 1 | Language Coverage | 1 | ✅ Complete | 2026-02-26 |
| 2 | Large Project Performance | 3 | ✅ Complete | 2026-02-26 |
| 3 | Workflow Integration | 4 | ✅ Complete | 2026-02-27 |

### v0.4 Consolidation & Scale — ✅ Complete (2026-02-27)

| Phase | Name | Plans | Status | Completed |
|-------|------|-------|--------|-----------|
| 1 | Consolidation & Scale | 4 | ✅ Complete | 2026-02-27 |

## Phase Details (v0.4)

### Phase 1: Consolidation & Scale

**Goal:** Harden performance, improve code quality, extend language coverage to 10, and enable cross-repo intelligence
**Depends on:** v0.3 complete
**Research:** Likely (Go/YAML/SQL parsers, multi-repo query patterns, batch Cypher strategies)

**Scope:**
- Performance: batch Cypher inserts, async embeddings, profile & optimize slow repos
- Code quality: fix bare except handlers, split kuzu_backend.py, version bump
- Markdown parser: upgrade from regex to tree-sitter, add frontmatter/tables
- New languages: Go, YAML/TOML, SQL, HTML/CSS (reach 10 total — achieved 12)
- Multi-repo: cross-repo MCP queries
- Usage analytics: event logging for MCP queries and pipeline runs, `axon stats` CLI command

**Plans:**
- [x] 01-01: Performance optimization (batch inserts, async embeddings, profiling)
- [x] 01-02: Code quality consolidation (error handling, kuzu_backend split, version bump)
- [x] 01-03: Language expansion (markdown tree-sitter upgrade + Go, YAML/TOML, SQL, HTML/CSS parsers — reach 12 languages)
- [x] 01-04: Platform features (multi-repo MCP queries + usage analytics + `axon stats` command)

---
*Roadmap created: 2026-02-26*
*Last updated: 2026-02-27 — v0.4 complete: 4 plans shipped, 12 languages, multi-repo MCP, analytics*
