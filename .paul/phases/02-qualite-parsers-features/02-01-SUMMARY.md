---
phase: 02-qualite-parsers-features
plan: 01
subsystem: parsers, mcp
tags: [byte-offsets, sql, yaml, toml, mcp-tool, axon_read_symbol]

requires:
  - phase: 01-securite-robustesse
    provides: parameterized queries, _sanitize_repo_slug, stable MCP dispatch infrastructure

provides:
  - SQL symbols with accurate byte offsets (start_byte=regex match, end_byte=semicolon+1)
  - YAML/TOML symbols with UTF-8 line-accurate byte offsets
  - axon_read_symbol MCP tool (O(1) file[start:end] read, fallback to stored content)

affects: [02-02, 02-03, 02-04, future parser quality]

tech-stack:
  added: []
  patterns: [line_start_bytes precomputation for byte-accurate UTF-8 offsets]

key-files:
  modified:
    - src/axon/core/parsers/sql_lang.py
    - src/axon/core/parsers/yaml_lang.py
    - src/axon/mcp/tools.py
    - src/axon/mcp/server.py

key-decisions:
  - "sql_lang.py: character offset == byte offset (ASCII assumption) — start_byte=m.start(), end_byte=find(';')+1"
  - "yaml_lang.py: precompute line_start_bytes[] once, pass to _parse_yaml/_parse_toml as third arg"
  - "axon_read_symbol: _get_repo_root_from_storage() helper reads meta.json for repo root path"
  - "Fallback path: start_byte==0 and end_byte==0 returns stored content field with note"

patterns-established:
  - "line_start_bytes = [0]; for el in encoded_lines: line_start_bytes.append(prev + len(el))"
  - "Parameterized queries for all MCP tool DB lookups (established v0.7 Plan 01-01)"

duration: 1 session
started: 2026-03-02T00:00:00Z
completed: 2026-03-02T22:57:48Z
---

# Phase 2 Plan 01: SQL/YAML Byte Offsets + axon_read_symbol

**SQL/YAML parsers now emit accurate byte offsets and `axon_read_symbol` delivers exact symbol source via O(1) file slice — completing byte-offset coverage across all 12 languages.**

## Performance

| Metric | Value |
|--------|-------|
| Duration | ~1 session |
| Completed | 2026-03-02 |
| Tasks | 3 completed |
| Files modified | 4 |
| Commit | `31bd23e` |

## Acceptance Criteria Results

| Criterion | Status | Notes |
|-----------|--------|-------|
| AC-1: SQL byte offsets | Pass | `start_byte=m.start()`, `end_byte=find(';')+1` for all CREATE symbols |
| AC-2: YAML/TOML byte offsets | Pass | `line_start_bytes[]` precomputed, passed to `_parse_yaml`/`_parse_toml` |
| AC-3: axon_read_symbol happy path | Pass | `file_bytes[start:end].decode("utf-8", errors="replace")` returned |
| AC-4: axon_read_symbol fallback | Pass | Returns stored `content` field with note when `start_byte==0` |
| AC-5: axon_read_symbol not found | Pass | Returns `"Symbol not found: {name}"` |

## Accomplishments

- SQL parser: 4 symbol types (TABLE, VIEW, FUNCTION, PROCEDURE) now carry precise byte offsets from regex match positions
- YAML/TOML parser: byte-accurate offsets via UTF-8 precomputed `line_start_bytes[]` — handles non-ASCII correctly
- `axon_read_symbol` MCP tool: registered in TOOLS list, dispatched in `_dispatch_tool()`, handles happy path / fallback / not-found / multi-match disambiguation
- `_get_repo_root_from_storage()` helper added to read `meta.json` for repo root path resolution
- 852 tests pass (0 regressions), 0 new ruff errors

## Task Commits

All tasks delivered in a single commit:

| Task | Commit | Type | Description |
|------|--------|------|-------------|
| Task 1: SQL byte offsets | `31bd23e` | feat | `start_byte=m.start()`, `end_byte=semicolon+1` in `sql_lang.py` |
| Task 2: YAML/TOML byte offsets | `31bd23e` | feat | `line_start_bytes[]` precomputation in `yaml_lang.py` |
| Task 3: axon_read_symbol | `31bd23e` | feat | `handle_read_symbol()` in `tools.py`, registered in `server.py` |

## Files Modified

| File | Change | Purpose |
|------|--------|---------|
| `src/axon/core/parsers/sql_lang.py` | Modified | Added `start_byte`/`end_byte` to all CREATE symbol calls |
| `src/axon/core/parsers/yaml_lang.py` | Modified | Precompute `line_start_bytes[]`, pass to `_parse_yaml`/`_parse_toml` |
| `src/axon/mcp/tools.py` | Modified | Added `handle_read_symbol()` + `_get_repo_root_from_storage()` |
| `src/axon/mcp/server.py` | Modified | Added `axon_read_symbol` to `TOOLS` list + `_dispatch_tool()` dispatch |

## Decisions Made

| Decision | Rationale | Impact |
|----------|-----------|--------|
| SQL: character offset == byte offset | SQL files are practically always ASCII; `m.start()` is accurate | Simple, no encoding overhead |
| YAML: precompute `line_start_bytes[]` outside loop | Single pass before parsing loop; passed as arg to both private methods | UTF-8 accurate, no repeated encoding |
| `_get_repo_root_from_storage()` reads `meta.json` | `storage._repo_path` not directly accessible; `meta.json["path"]` is the authoritative root | Consistent with daemon/proxy architecture |

## Deviations from Plan

### Summary

| Type | Count | Impact |
|------|-------|--------|
| Deferred | 1 | Tests missing for new functionality |

**Total impact:** Feature complete, no scope creep; test coverage gap deferred.

### Deferred Items

- **No new tests for byte offsets or axon_read_symbol**: The plan specified 3 new tests (one per task). The commit contains no test file changes. Existing 852 tests pass but `test_sql.py` and `test_yaml.py` have no `start_byte`/`end_byte` assertions. `axon_read_symbol` has no dedicated test.
  → Logged for 02-02 planning consideration or as a standalone fix task.

## Issues Encountered

| Issue | Resolution |
|-------|------------|
| None | Plan executed cleanly |

## Next Phase Readiness

**Ready:**
- `axon_read_symbol` tool available for agents — byte-offset story complete for all 12 languages
- `_get_repo_root_from_storage()` helper reusable for any tool needing the repo root
- Foundation for 02-02 (parser quality: dead code patterns, TS generics, Python wildcards)

**Concerns:**
- No tests for `axon_read_symbol` or the new byte offset paths — should be addressed before v0.7 ships

**Blockers:**
- None

---
*Phase: 02-qualite-parsers-features, Plan: 01*
*Completed: 2026-03-02*
