---
phase: 01-graph-intelligence
plan: 01
subsystem: parsers
tags: [typescript, dead_code, type_refs, graph_edges, USES_TYPE]

requires:
  - phase: 02-qualite-parsers-features
    provides: Byte offsets, test_parser_typescript.py baseline, dead_code.py _is_test_file()

provides:
  - TS class property type_refs (USES_TYPE edges for `items: Array<User>`)
  - TS interface member type_refs (USES_TYPE edges for `bar: User`)
  - TS generic base class type_refs (`extends Base<User>` → TypeRef for User)
  - Go _test.go exemption in dead code detection
  - spec/ root-level path exemption in dead code detection
  - Python wildcard import regression test (confirmed working)

affects: 01-02 (node enrichment), axon_dead_code tool accuracy, axon_context type edge coverage

tech-stack:
  added: []
  patterns: [reuse _extract_variable_type_annotation for class fields and interface members]

key-files:
  modified:
    - src/axon/core/parsers/typescript.py
    - src/axon/core/ingestion/dead_code.py
    - tests/core/test_parser_typescript.py
    - tests/core/test_dead_code.py

key-decisions:
  - "Python wildcard: no bug — names=['*'] does not block IMPORTS edge; added regression test only"
  - "_extract_class_heritage() unified: generic base types (extends Base<User>) now call _extract_generic_arg_refs"

patterns-established:
  - "Reuse _extract_variable_type_annotation(node, result) for any new typed-member context"

duration: ~30min
started: 2026-03-05T00:00:00Z
completed: 2026-03-05T00:00:00Z
---

# Phase 01 Plan 01: Parser Completeness Summary

**TypeScript USES_TYPE edges extended to class properties, interface members, and generic base classes; Go _test.go + spec/ root patterns added to dead code exemptions.**

## Performance

| Metric | Value |
|--------|-------|
| Duration | ~30 min |
| Completed | 2026-03-05 |
| Tasks | 2 completed |
| Files modified | 4 |
| Tests before | 884 |
| Tests after | 891 (+7) |

## Acceptance Criteria Results

| Criterion | Status | Notes |
|-----------|--------|-------|
| AC-1: TS class property type_refs | Pass | `items: Array<User>` → TypeRef User, `config: Config` → TypeRef Config |
| AC-2: TS interface member type_refs | Pass | `bar: User` inside `interface_body` → TypeRef User |
| AC-3: TS generic base class type_refs | Pass | `extends Repository<User>` → TypeRef User from type_arguments |
| AC-4: Python wildcard import IMPORTS edge | Pass | No bug found; `names=["*"]` correctly creates IMPORTS edge; regression test added |
| AC-5: Go test file dead code exemption | Pass | `_test.go` suffix and `spec/` root-path patterns added |

## Accomplishments

- Extended `_extract_class()` to iterate `public_field_definition` nodes and call `_extract_variable_type_annotation()` — zero new methods needed
- Extended `_extract_class_heritage()` to handle `generic_type` nodes in `extends_clause` via `_extract_generic_arg_refs()`
- Extended `_extract_interface()` to iterate `property_signature` nodes and extract type annotations
- Added `or file_path.endswith("_test.go")` and `or file_path.startswith("spec/")` to `_is_test_file()`
- 7 new targeted tests covering all 5 ACs

## Files Created/Modified

| File | Change | Purpose |
|------|--------|---------|
| `src/axon/core/parsers/typescript.py` | Modified | Class property, interface member, generic base type_refs |
| `src/axon/core/ingestion/dead_code.py` | Modified | Go _test.go + spec/ root exemptions |
| `tests/core/test_parser_typescript.py` | Modified | 3 new type_ref tests |
| `tests/core/test_dead_code.py` | Modified | 3+ new dead code pattern tests |

## Decisions Made

| Decision | Rationale | Impact |
|----------|-----------|--------|
| Python wildcard: no fix, only test | Traced `from x import *` → IMPORTS edge creation — no bug existed | AC-4 closed with regression test only |
| Reuse `_extract_variable_type_annotation` | Already handles type_annotation nodes correctly for variables | No duplicated logic |
| `_extract_class_heritage()` simplified | Unified extends/implements via existing `_extract_generic_arg_refs` | Cleaner, also covers generic base types |

## Deviations from Plan

| Type | Count | Impact |
|------|-------|--------|
| Auto-fixed | 0 | — |
| Scope additions | 0 | — |
| Deferred | 0 | — |

**Total impact:** Plan executed exactly as written. Python wildcard investigation confirmed no bug.

## Issues Encountered

None.

## Next Phase Readiness

**Ready:**
- USES_TYPE edge coverage now includes class properties, interface members, and generic base classes
- Dead code detection correctly exempts Go test files and spec/ root paths
- Foundation for Plan 01-02: schema is unchanged (as specified), ready to add `tested: bool` + `centrality: float`

**Concerns:**
- None

**Blockers:**
- None

---
*Phase: 01-graph-intelligence, Plan: 01*
*Completed: 2026-03-05*
