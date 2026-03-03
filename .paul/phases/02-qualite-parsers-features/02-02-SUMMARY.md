---
phase: 02-qualite-parsers-features
plan: 02
subsystem: parsers, testing
tags: [dead-code, python-parser, typescript-parser, sql-parser, yaml-parser, mcp-tools, test-coverage]

requires:
  - phase: 02-01
    provides: sql/yaml byte offsets, axon_read_symbol (without tests)

provides:
  - _is_test_file() handles 6 new test-file patterns (spec/, __tests__/, _spec.rb, _test.exs, .spec.*, .test.*)
  - Python wildcard import (from x import *) → ImportInfo(names=["*"])
  - TypeScript generic type args (Array<User>) → TypeRef(name="User")
  - TypeScript type parameter constraints (<T extends Schema>) → TypeRef(name="Schema")
  - +19 tests closing 02-01 coverage debt and covering all new behaviors

affects: [dead-code-detection, import-graph, type-graph, axon_read_symbol]

tech-stack:
  added: []
  patterns: [parametrized pytest classes, TypeRef extraction from generic_type nodes]

key-files:
  created: []
  modified:
    - src/axon/core/ingestion/dead_code.py
    - src/axon/core/parsers/python_lang.py
    - src/axon/core/parsers/typescript.py
    - tests/core/test_dead_code.py
    - tests/core/test_parser_python.py
    - tests/core/test_parser_typescript.py
    - tests/core/parsers/test_sql.py
    - tests/core/parsers/test_yaml.py
    - tests/mcp/test_tools.py

key-decisions:
  - "__tests__/ without leading slash — already correct before session; no change needed"
  - "TS test uses direct param annotation (simpler than plan's callback form) — equivalent coverage"

patterns-established:
  - "TypeRef extraction: _extract_generic_arg_refs() + _extract_type_param_constraints() as static methods"
  - "Dead code exemption: dot-joined pattern chain in _is_test_file()"

duration: ~45min (split across 2 sessions)
started: 2026-03-02T00:00:00Z
completed: 2026-03-03T00:00:00Z
---

# Phase 02 Plan 02: Parser Quality + Test Coverage Summary

**Parser quality gaps fixed and 02-01 coverage debt closed — 871 tests passing (+19), all 6 AC met.**

## Performance

| Metric | Value |
|--------|-------|
| Duration | ~45min (split across 2 sessions) |
| Started | 2026-03-02 |
| Completed | 2026-03-03 |
| Tasks | 3 completed |
| Files modified | 9 |

## Acceptance Criteria Results

| Criterion | Status | Notes |
|-----------|--------|-------|
| AC-1: Dead code test file patterns | Pass | 11-case parametrize covers all 6 new patterns |
| AC-2: Python wildcard imports | Pass | `from x import *` → `ImportInfo(names=["*"])` |
| AC-3: TS generic type annotations → USES_TYPE | Pass | `Array<UserData>` → TypeRef(name="UserData") |
| AC-4: TS type parameter constraints → USES_TYPE | Pass | `<T extends Schema>` → TypeRef(name="Schema") |
| AC-5: SQL/YAML byte offset tests | Pass | Both byte offset assertions passing |
| AC-6: axon_read_symbol tests | Pass | not-found + fallback-to-stored-content tests |

## Accomplishments

- `_is_test_file()` now exempts symbols in `spec/`, `__tests__/`, `_spec.rb`, `_test.exs`, `.spec.*`, `.test.*` files from dead-code detection
- Python wildcard imports correctly produce `ImportInfo(names=["*"])` via `wildcard_import` tree-sitter node
- TypeScript: `_extract_generic_arg_refs()` extracts TypeRefs from `Array<User>` style annotations; `_extract_type_param_constraints()` extracts constraints from `<T extends Schema>`
- 02-01 coverage debt closed: SQL byte offsets, YAML byte offsets, `handle_read_symbol` not-found and fallback paths all tested

## Task Commits

| Task | Commit | Description |
|------|--------|-------------|
| Tasks 1+2 (impl) | `31bd23e` | sql/yaml byte offsets + axon_read_symbol (prior session) |
| Tasks 1+2 (code) | prev session | dead_code patterns, python wildcard, TS generics |
| Task 3 (tests) | `6016343` | +19 tests, all AC covered |

## Files Created/Modified

| File | Change | Purpose |
|------|--------|---------|
| `src/axon/core/ingestion/dead_code.py` | Modified | 6 new `_is_test_file()` patterns |
| `src/axon/core/parsers/python_lang.py` | Modified | wildcard_import node → `names=["*"]` |
| `src/axon/core/parsers/typescript.py` | Modified | generic_type handling + 2 new helpers |
| `tests/core/test_dead_code.py` | Modified | `TestIsTestFile` with 11 parametrized cases |
| `tests/core/test_parser_python.py` | Modified | `test_wildcard_import` in `TestParseImports` |
| `tests/core/test_parser_typescript.py` | Modified | 2 new generic/constraint test functions |
| `tests/core/parsers/test_sql.py` | Modified | `TestSqlByteOffsets` (2 assertions) |
| `tests/core/parsers/test_yaml.py` | Modified | `TestYamlByteOffsets` (1 assertion) |
| `tests/mcp/test_tools.py` | Modified | `TestHandleReadSymbol` (not-found + fallback) |

## Decisions Made

| Decision | Rationale | Impact |
|----------|-----------|--------|
| `"__tests__/"` without leading slash | Already correct in code; root-level `__tests__/` paths also match | Broader test-file detection |
| TS test uses simpler direct annotation | `function fetch(data: Array<UserData>): void` clearer than nested callback form | Equivalent coverage, simpler test |

## Deviations from Plan

| Type | Count | Impact |
|------|-------|--------|
| Minor test variation | 1 | TS test source slightly simplified vs PLAN — same AC coverage |
| Pattern pre-applied | 1 | `__tests__/` fix was already in code from prior session |

**Total impact:** Zero scope change, implementation cleaner than plan.

## Issues Encountered

| Issue | Resolution |
|-------|------------|
| `__tests__/` already had correct pattern | No fix needed — verified via parametrized test |

## Next Phase Readiness

**Ready:**
- 02-03: file size limits in walker.py, compute_repo_slug() extraction
- 02-04: socket buffer and axon_batch fixes
- Parser infrastructure solid: TypeRef/ImportInfo extraction patterns established

**Concerns:** None

**Blockers:** None

---
*Phase: 02-qualite-parsers-features, Plan: 02*
*Completed: 2026-03-03*
