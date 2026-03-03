# Plan 01-03 SUMMARY — Intelligence Layer

## Status: COMPLETE ✓

**Executed:** 2026-03-05
**Tests before:** 902 → **Tests after:** 913 (+11 new tests)

## Tasks Completed

### Task 1: Code-aware embedding text ✓
- Updated `_text_for_callable()` and `_text_for_class()` in `text.py` to include the first ~400 chars of `node.content` as a `source:` section
- FILE nodes explicitly excluded (no content included for large files)
- Added 5 new tests in `TestContentInclusion` class in `tests/core/test_embedding_text.py`

### Task 2: axon_find_similar MCP tool ✓
- Added `get_embedding(conn, node_id) -> list[float] | None` to `kuzu_search.py`
- Exposed `get_embedding()` method on `KuzuBackend`
- Added `handle_find_similar(storage, symbol, limit=10, repo=None)` to `tools.py`
- Registered `axon_find_similar` tool in `server.py` (TOOLS list + `_dispatch_tool`)
- Added 4 new tests in `TestHandleFindSimilar` in `tests/mcp/test_tools.py`
- Self-exclusion, no-embedding error, not-found all covered

### Task 3: Attribute surfacing + query expansion ✓
- `handle_context`: added `Attributes: tested=yes/no  exported=yes/no  centrality=N.NNN` line (centrality omitted when 0.0)
- `_format_query_results`: added `_result_tags()` helper for compact `[exported]`/`[untested]` inline tags
- `handle_query`: added `AXON_QUERY_EXPAND` env var gate with `_expand_query()` synonym heuristic
- Added 2 new tests in `TestContextAttributeSurfacing` in `tests/mcp/test_tools.py`

## Deviations
None.

## Files Modified
- `src/axon/core/embeddings/text.py`
- `src/axon/core/storage/kuzu_search.py` — added `get_embedding()`
- `src/axon/core/storage/kuzu_backend.py` — exposed `get_embedding()` method
- `src/axon/mcp/tools.py` — `handle_find_similar`, `_result_tags`, `_expand_query`, attribute surfacing
- `src/axon/mcp/server.py` — `axon_find_similar` tool registration

## Files Added/Extended
- `tests/core/test_embedding_text.py` — added `TestContentInclusion` (5 tests)
- `tests/mcp/test_tools.py` — added `TestContextAttributeSurfacing` (2) + `TestHandleFindSimilar` (4)
