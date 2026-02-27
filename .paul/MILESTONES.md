# Milestones

Completed milestone log for this project.

| Milestone | Completed | Duration | Stats |
|-----------|-----------|----------|-------|
| v0.4 Consolidation & Scale | 2026-02-27 | 1 day | 1 phase, 4 plans, ~35 files |

---

## ✅ v0.4 Consolidation & Scale

**Completed:** 2026-02-27
**Duration:** 1 day

### Stats

| Metric | Value |
|--------|-------|
| Phases | 1 |
| Plans | 4 |
| Files changed | ~35 |
| Tests | 751 passing (18 new) |
| Languages | 6 → 12 |

### Key Accomplishments

- **Performance hardened:** Batch Cypher inserts, async embeddings via ThreadPoolExecutor (fire-and-forget), profiling baseline established — storage_load identified as 98%+ of indexing time
- **Code quality consolidated:** Bare except handlers replaced with specific RuntimeError catches, `kuzu_backend.py` split into schema/search/bulk submodules, version bumped to 0.4.0
- **Language coverage doubled:** Markdown upgraded from regex to tree-sitter with frontmatter + pipe table support; 5 new parsers (Go, YAML/TOML, SQL, HTML, CSS) — 6 → 12 languages, 20 extensions
- **Multi-repo MCP queries:** `axon_query`, `axon_context`, `axon_impact` accept optional `repo=` param routing to any registered repo via `~/.axon/repos/`; missing repo returns clean error string
- **Usage analytics:** `log_event()` appends to `~/.axon/events.jsonl` on every MCP call and `axon analyze` run — fire-and-forget, never raises
- **`axon stats` CLI:** Reads events.jsonl and prints aggregated metrics: total queries, unique queries, top-5, index runs, last activity per repo

### Key Decisions

| Decision | Rationale |
|----------|-----------|
| storage_load is 98%+ of indexing time | Future perf work targets KuzuDB bulk_load, not pipeline phases |
| Async embeddings via ThreadPoolExecutor | Pipeline returns immediately; embeddings continue in background |
| KuzuDB raises plain RuntimeError | No kuzu-specific exception type — all except blocks use RuntimeError |
| events.jsonl global at ~/.axon/events.jsonl | One log for all repos on the machine |
| log_event() never raises (BLE001 catch-all) | Analytics failure must never block main flow |
| repo= opens/closes KuzuBackend per request | Safe for read-only; avoids connection leaks |
| YAML/TOML/SQL use regex, not tree-sitter | Simple structure; no overhead of tree-sitter grammar needed |
| Go struct kind → NodeLabel.CLASS | Consistent with Elixir module → CLASS; no new graph label needed |

---
