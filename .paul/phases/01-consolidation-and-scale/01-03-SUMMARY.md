---
phase: 01-consolidation-and-scale
plan: 03
subsystem: parsers
tags: [tree-sitter, parsers, go, yaml, toml, sql, html, css, markdown, language-coverage]

requires:
  - phase: 01-02
    provides: Stable kuzu_backend, clean exception handling, version 0.4.0 base

provides:
  - Markdown parser with tree-sitter backend + YAML frontmatter + pipe table extraction
  - GoParser: functions, structs, interfaces, methods, imports, calls (tree-sitter-go)
  - YamlParser: YAML top-level + depth-1 keys, TOML sections + keys (line-based)
  - SqlParser: CREATE TABLE/VIEW/FUNCTION/PROCEDURE, DROP/ALTER (regex)
  - HtmlParser: id elements, script/link imports, anchor calls (tree-sitter-html)
  - CssParser: ID selectors, class selectors, @import (tree-sitter-css)
  - 12 supported languages, 20 extensions registered

affects: [01-04, any future language/parser work]

tech-stack:
  added: [tree-sitter-go>=0.25.0, tree-sitter-html>=0.23.0, tree-sitter-css>=0.25.0]
  patterns:
    - Tree-sitter parsers follow RustParser template (walk dispatch + extractor methods)
    - Lightweight languages (YAML, TOML, SQL) use regex/line-based parsing — no tree-sitter needed
    - script_element/style_element are distinct node types in tree-sitter-html (not "element")

key-files:
  created:
    - src/axon/core/parsers/go_lang.py
    - src/axon/core/parsers/yaml_lang.py
    - src/axon/core/parsers/sql_lang.py
    - src/axon/core/parsers/html_lang.py
    - src/axon/core/parsers/css_lang.py
    - tests/core/parsers/test_go.py
    - tests/core/parsers/test_yaml.py
    - tests/core/parsers/test_sql.py
    - tests/core/parsers/test_html.py
    - tests/core/parsers/test_css.py
  modified:
    - src/axon/core/parsers/markdown.py (tree-sitter + frontmatter + tables — was already done)
    - src/axon/core/parsers/__init__.py (exports all new parsers)
    - src/axon/core/ingestion/parser_phase.py (get_parser() for 6 new languages)
    - src/axon/config/languages.py (20 extensions)
    - pyproject.toml (3 new tree-sitter deps)
    - tests/core/parsers/test_markdown.py (frontmatter + table tests added)

key-decisions:
  - "YAML/TOML/SQL use regex — simple enough, no tree-sitter overhead"
  - "struct kind maps to NodeLabel.CLASS (consistent with Elixir module → CLASS)"
  - "HTML script_element and style_element must be included alongside element in _walk"

patterns-established:
  - "New tree-sitter parser: import tsXXX, Language(tsXXX.language()), Parser(LANG), walk dispatch"
  - "Lightweight parsers: pure regex, no import dependencies beyond base.py"
  - "Go exports: name[0].isupper() (Go convention)"

duration: ~15min
started: 2026-02-27T00:00:00Z
completed: 2026-02-27T00:00:00Z
---

# Phase 1 Plan 03: Language Parser Expansion Summary

**Markdown upgraded to tree-sitter with frontmatter/table support; 5 new parsers (Go, YAML/TOML, SQL, HTML, CSS) added, expanding language coverage from 6 to 12 with 750 tests passing.**

## Performance

| Metric | Value |
|--------|-------|
| Duration | ~15 min |
| Tasks | 3 completed |
| Files modified | 10+ |
| Tests | 750 pass (63 new parser tests) |
| Languages | 6 → 12 |
| Extensions | 20 registered |

## Acceptance Criteria Results

| Criterion | Status | Notes |
|-----------|--------|-------|
| AC-1: Markdown tree-sitter headings | Pass | atx_heading nodes, section spans, level tracking |
| AC-2: Markdown tables extracted | Pass | pipe table → `table:{first_col}` SymbolInfo |
| AC-3: Go parser (functions, structs, interfaces, methods) | Pass | Full tree-sitter-go extraction |
| AC-4: YAML/TOML top-level + nested keys | Pass | Line-based, depth-1 nesting, TOML sections |
| AC-5: SQL DDL objects | Pass | CREATE TABLE/VIEW/FUNCTION/PROCEDURE regex |
| AC-6: HTML id elements, script/link imports, anchor calls | Pass | Fixed script_element bug |
| AC-7: CSS ID + class selectors, @import | Pass | tree-sitter-css |
| AC-8: All languages registered, 687+ tests pass | Pass | 750 total pass, 0 regressions |

## Accomplishments

- 5 new parser files created following established RustParser template pattern
- Markdown parser already had tree-sitter upgrade from prior work; frontmatter and table extraction verified
- `get_parser()` now handles 12 languages; `languages.py` covers 20 file extensions
- 63 new test cases across 6 test files, all passing

## Files Created/Modified

| File | Change | Purpose |
|------|--------|---------|
| `src/axon/core/parsers/go_lang.py` | Created | Go tree-sitter parser |
| `src/axon/core/parsers/yaml_lang.py` | Created | YAML/TOML line-based parser |
| `src/axon/core/parsers/sql_lang.py` | Created | SQL regex DDL parser |
| `src/axon/core/parsers/html_lang.py` | Created | HTML tree-sitter parser |
| `src/axon/core/parsers/css_lang.py` | Created | CSS tree-sitter parser |
| `src/axon/core/parsers/__init__.py` | Modified | Export all 10 parsers |
| `src/axon/core/ingestion/parser_phase.py` | Modified | `get_parser()` + `_KIND_TO_LABEL` |
| `src/axon/config/languages.py` | Modified | 20 extensions registered |
| `pyproject.toml` | Modified | tree-sitter-go, -html, -css deps |
| `tests/core/parsers/test_markdown.py` | Modified | Frontmatter + table tests |
| `tests/core/parsers/test_go.py` | Created | Go parser tests (9 tests) |
| `tests/core/parsers/test_yaml.py` | Created | YAML/TOML parser tests (9 tests) |
| `tests/core/parsers/test_sql.py` | Created | SQL parser tests (9 tests) |
| `tests/core/parsers/test_html.py` | Created | HTML parser tests (7 tests) |
| `tests/core/parsers/test_css.py` | Created | CSS parser tests (8 tests) |

## Decisions Made

| Decision | Rationale | Impact |
|----------|-----------|--------|
| YAML/TOML/SQL use regex not tree-sitter | Structure is simple and well-defined; no tree-sitter grammar needed | Fewer dependencies, faster parsing |
| `struct` kind maps to `NodeLabel.CLASS` | Consistent with Elixir module → CLASS; structs are "types" in the graph | No new NodeLabel values needed |
| script_element + style_element added to HTML walker | tree-sitter-html uses distinct node types for these, not "element" | Fixes silent drop of `<script src>` imports |

## Deviations from Plan

### Auto-fixed Issues

**1. HTML parser bug: `<script>` elements not captured**
- **Found during:** Task 3 verification (test_script_src_extracted failing)
- **Issue:** `_walk` only dispatched on `node.type == "element"`, but tree-sitter-html uses `script_element` for `<script>` tags
- **Fix:** Added `"script_element"` and `"style_element"` to the dispatch condition in `_walk`
- **Files:** `src/axon/core/parsers/html_lang.py`
- **Verification:** All 7 HTML tests pass after fix

### Deferred Items

None.

## Issues Encountered

| Issue | Resolution |
|-------|------------|
| HTML `<script src>` not captured by parser | Fixed: tree-sitter-html uses `script_element` not `element` node type |

## Next Phase Readiness

**Ready:**
- 12 languages supported with consistent ParseResult interface
- All parsers follow established patterns (easy to add more)
- `get_parser()` fully extensible for plan 01-04

**Concerns:**
- None identified

**Blockers:**
- None

---
*Phase: 01-consolidation-and-scale, Plan: 03*
*Completed: 2026-02-27*
