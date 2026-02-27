"""Tests for the upgraded Markdown parser (tree-sitter + frontmatter + tables)."""

from __future__ import annotations

import pytest

from axon.core.parsers.markdown import MarkdownParser


@pytest.fixture
def parser() -> MarkdownParser:
    return MarkdownParser()


# ---------------------------------------------------------------------------
# Frontmatter extraction
# ---------------------------------------------------------------------------


class TestFrontmatter:
    CODE = """\
---
title: My Document
date: 2026-01-15
tags: [python, axon]
---

# Main Title

Some content.
"""

    def test_frontmatter_keys_extracted(self, parser: MarkdownParser) -> None:
        result = parser.parse(self.CODE, "doc.md")
        fm_symbols = [s for s in result.symbols if s.name.startswith("frontmatter:")]
        names = {s.name for s in fm_symbols}
        assert "frontmatter:title" in names
        assert "frontmatter:date" in names
        assert "frontmatter:tags" in names

    def test_frontmatter_kind_is_function(self, parser: MarkdownParser) -> None:
        result = parser.parse(self.CODE, "doc.md")
        fm_symbols = [s for s in result.symbols if s.name.startswith("frontmatter:")]
        assert all(s.kind == "function" for s in fm_symbols)

    def test_frontmatter_and_headings_coexist(self, parser: MarkdownParser) -> None:
        result = parser.parse(self.CODE, "doc.md")
        sections = [s for s in result.symbols if s.kind == "section"]
        fm = [s for s in result.symbols if s.name.startswith("frontmatter:")]
        assert len(sections) >= 1
        assert len(fm) >= 1

    def test_no_frontmatter(self, parser: MarkdownParser) -> None:
        code = "# Title\n\nJust content.\n"
        result = parser.parse(code, "doc.md")
        fm = [s for s in result.symbols if s.name.startswith("frontmatter:")]
        assert len(fm) == 0

    def test_only_frontmatter(self, parser: MarkdownParser) -> None:
        code = "---\nkey: value\n---\n"
        result = parser.parse(code, "doc.md")
        fm = [s for s in result.symbols if s.name.startswith("frontmatter:")]
        assert len(fm) == 1
        assert fm[0].name == "frontmatter:key"


# ---------------------------------------------------------------------------
# Table extraction
# ---------------------------------------------------------------------------


class TestTables:
    CODE = """\
# Data

| Name | Age | Role |
|------|-----|------|
| Alice | 30 | Dev |
| Bob | 25 | QA |

Some text after.
"""

    def test_table_extracted(self, parser: MarkdownParser) -> None:
        result = parser.parse(self.CODE, "doc.md")
        tables = [s for s in result.symbols if s.name.startswith("table:")]
        assert len(tables) == 1

    def test_table_named_by_first_header(self, parser: MarkdownParser) -> None:
        result = parser.parse(self.CODE, "doc.md")
        tables = [s for s in result.symbols if s.name.startswith("table:")]
        assert tables[0].name == "table:Name"

    def test_table_kind_is_section(self, parser: MarkdownParser) -> None:
        result = parser.parse(self.CODE, "doc.md")
        tables = [s for s in result.symbols if s.name.startswith("table:")]
        assert tables[0].kind == "section"

    def test_no_tables(self, parser: MarkdownParser) -> None:
        code = "# Title\n\nJust text.\n"
        result = parser.parse(code, "doc.md")
        tables = [s for s in result.symbols if s.name.startswith("table:")]
        assert len(tables) == 0


# ---------------------------------------------------------------------------
# Backward compatibility with existing behavior
# ---------------------------------------------------------------------------


class TestHeadingsCompat:
    CODE = """\
# My Document

## Introduction

Some text here.

## Usage

More text.

### Advanced Usage

Even more text.
"""

    def test_heading_count(self, parser: MarkdownParser) -> None:
        result = parser.parse(self.CODE, "doc.md")
        sections = [s for s in result.symbols if s.kind == "section"]
        assert len(sections) == 4

    def test_h1_name(self, parser: MarkdownParser) -> None:
        result = parser.parse(self.CODE, "doc.md")
        h1 = [s for s in result.symbols if s.name == "My Document"]
        assert len(h1) == 1
        assert h1[0].start_line == 1

    def test_h2_names(self, parser: MarkdownParser) -> None:
        result = parser.parse(self.CODE, "doc.md")
        names = {s.name for s in result.symbols}
        assert "Introduction" in names
        assert "Usage" in names

    def test_section_end_line(self, parser: MarkdownParser) -> None:
        result = parser.parse(self.CODE, "doc.md")
        intro = [s for s in result.symbols if s.name == "Introduction"][0]
        usage = [s for s in result.symbols if s.name == "Usage"][0]
        assert intro.end_line < usage.start_line

    def test_last_section_ends_at_eof(self, parser: MarkdownParser) -> None:
        result = parser.parse(self.CODE, "doc.md")
        sections = [s for s in result.symbols if s.kind == "section"]
        last = sorted(sections, key=lambda s: s.start_line)[-1]
        total_lines = len(self.CODE.splitlines())
        assert last.end_line == total_lines


class TestLinksCompat:
    CODE = """\
# Doc

See [Introduction](./intro.md) and [API Reference](./api.md).

Also visit [GitHub](https://github.com/example/repo).
"""

    def test_links_extracted(self, parser: MarkdownParser) -> None:
        result = parser.parse(self.CODE, "doc.md")
        assert len(result.imports) == 3

    def test_link_module_is_url(self, parser: MarkdownParser) -> None:
        result = parser.parse(self.CODE, "doc.md")
        modules = {i.module for i in result.imports}
        assert "./intro.md" in modules
        assert "./api.md" in modules


class TestCodeBlocksCompat:
    CODE = """\
# Guide

```elixir
defmodule Foo do
  def bar, do: :ok
end
```

```python
def hello():
    pass
```
"""

    def test_code_block_calls(self, parser: MarkdownParser) -> None:
        result = parser.parse(self.CODE, "guide.md")
        call_names = {c.name for c in result.calls}
        assert "elixir" in call_names
        assert "python" in call_names


class TestEdgeCasesCompat:
    def test_empty_file(self, parser: MarkdownParser) -> None:
        result = parser.parse("", "empty.md")
        assert result.symbols == []
        assert result.imports == []
        assert result.calls == []
        assert result.exports == []

    def test_no_headings(self, parser: MarkdownParser) -> None:
        code = "Just some plain text without any headings.\n"
        result = parser.parse(code, "plain.md")
        sections = [s for s in result.symbols if s.kind == "section"]
        assert sections == []

    def test_h1_is_exported(self, parser: MarkdownParser) -> None:
        code = "# My Title\n\nContent here.\n"
        result = parser.parse(code, "doc.md")
        assert "My Title" in result.exports

    def test_h2_not_exported(self, parser: MarkdownParser) -> None:
        code = "# Title\n\n## Section\n\nContent.\n"
        result = parser.parse(code, "doc.md")
        assert "Section" not in result.exports
        assert "Title" in result.exports
