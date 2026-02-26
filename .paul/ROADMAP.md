# Roadmap: axon

## Overview

Axon evolves from a functional code indexer into a production-grade tool that seamlessly handles any project at any scale. The roadmap prioritises language coverage, large-project performance, and frictionless integration into daily development workflows — making it the default intelligence layer for AI-assisted development.

## Current Milestone

**v0.3 Workflow Integration** (v0.3.0)
Status: In progress
Phases: 1 of 3 complete

## Phases

| Phase | Name | Plans | Status | Completed |
|-------|------|-------|--------|-----------|
| 1 | Language Coverage | 1 | ✅ Complete | 2026-02-26 |
| 2 | Large Project Performance | TBD | Planning | - |
| 3 | Workflow Integration | TBD | Not started | - |

## Phase Details

### Phase 1: Language Coverage

**Goal:** All languages used across developer's projects are parsed correctly
**Depends on:** Nothing (first phase)
**Research:** Unlikely (Tree-sitter parsers are established)

**Scope:**
- Elixir parser
- Rust parser
- Markdown parser
- Test coverage for all new parsers

**Plans:**
- [ ] 01-01: Elixir parser + tests
- [ ] 01-02: Rust parser + tests
- [ ] 01-03: Markdown parser + tests

### Phase 2: Large Project Performance

**Goal:** Axon indexes and re-indexes massive projects (100k+ LOC) without blocking the developer
**Depends on:** Phase 1 (language coverage complete)
**Research:** Likely (incremental indexing strategies, concurrency models)
**Research topics:** Chunking strategies, parallel parsing, incremental diff-based re-indexing

**Scope:**
- Benchmark suite for large project indexing
- Incremental/differential re-indexing
- Concurrency / worker pool for parsing

**Plans:**
- [ ] 02-01: Benchmark baseline on large repos
- [ ] 02-02: Incremental indexing (file-level delta)
- [ ] 02-03: Parallel parsing worker pool

### Phase 3: Workflow Integration

**Goal:** Axon is zero-friction on every project session start — auto-detected, auto-started, auto-queried
**Depends on:** Phase 2 (performance acceptable)
**Research:** Likely (CI hooks, shell integration, MCP query patterns)

**Scope:**
- Shell/devtools integration (auto-start on cd)
- CI pipeline integration
- Refined MCP query API for agent consumption
- Documentation and onboarding

**Plans:**
- [ ] 03-01: Shell integration (direnv / hook)
- [ ] 03-02: CI integration guide + config templates
- [ ] 03-03: MCP query API refinement
- [ ] 03-04: Developer documentation

---
*Roadmap created: 2026-02-26*
*Last updated: 2026-02-26*
