"""Tests for the Markdown parser."""

from __future__ import annotations

import pytest

from axon.core.parsers.markdown import MarkdownParser


@pytest.fixture
def parser() -> MarkdownParser:
    return MarkdownParser()


# ---------------------------------------------------------------------------
# Heading extraction (symbols)
# ---------------------------------------------------------------------------


class TestParseHeadings:
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

    def test_h3_name(self, parser: MarkdownParser) -> None:
        result = parser.parse(self.CODE, "doc.md")
        names = {s.name for s in result.symbols}
        assert "Advanced Usage" in names

    def test_section_end_line(self, parser: MarkdownParser) -> None:
        result = parser.parse(self.CODE, "doc.md")
        intro = [s for s in result.symbols if s.name == "Introduction"][0]
        usage = [s for s in result.symbols if s.name == "Usage"][0]
        # Introduction ends just before Usage starts
        assert intro.end_line < usage.start_line

    def test_last_section_ends_at_eof(self, parser: MarkdownParser) -> None:
        result = parser.parse(self.CODE, "doc.md")
        last = sorted(result.symbols, key=lambda s: s.start_line)[-1]
        total_lines = len(self.CODE.splitlines())
        assert last.end_line == total_lines


# ---------------------------------------------------------------------------
# Exports (top-level headings)
# ---------------------------------------------------------------------------


class TestExports:
    def test_h1_is_exported(self, parser: MarkdownParser) -> None:
        code = "# My Title\n\nContent here.\n"
        result = parser.parse(code, "doc.md")
        assert "My Title" in result.exports

    def test_h2_not_exported(self, parser: MarkdownParser) -> None:
        code = "# Title\n\n## Section\n\nContent.\n"
        result = parser.parse(code, "doc.md")
        assert "Section" not in result.exports
        assert "Title" in result.exports

    def test_multiple_h1_all_exported(self, parser: MarkdownParser) -> None:
        code = "# First\n\n# Second\n"
        result = parser.parse(code, "doc.md")
        assert "First" in result.exports
        assert "Second" in result.exports


# ---------------------------------------------------------------------------
# Link extraction (imports)
# ---------------------------------------------------------------------------


class TestParseLinks:
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
        assert "https://github.com/example/repo" in modules

    def test_link_names_contain_text(self, parser: MarkdownParser) -> None:
        result = parser.parse(self.CODE, "doc.md")
        intro_imp = [i for i in result.imports if i.module == "./intro.md"]
        assert len(intro_imp) == 1
        assert "Introduction" in intro_imp[0].names


# ---------------------------------------------------------------------------
# Code block extraction (calls)
# ---------------------------------------------------------------------------


class TestParseCodeBlocks:
    CODE = """\
# Guide

Here is some Elixir code:

```elixir
defmodule Foo do
  def bar, do: :ok
end
```

And some Python:

```python
def hello():
    pass
```
"""

    def test_elixir_code_block_call(self, parser: MarkdownParser) -> None:
        result = parser.parse(self.CODE, "guide.md")
        elixir_calls = [c for c in result.calls if c.name == "elixir"]
        assert len(elixir_calls) == 1

    def test_python_code_block_call(self, parser: MarkdownParser) -> None:
        result = parser.parse(self.CODE, "guide.md")
        py_calls = [c for c in result.calls if c.name == "python"]
        assert len(py_calls) == 1

    def test_links_inside_code_blocks_not_extracted(self, parser: MarkdownParser) -> None:
        code = """\
# Doc

```markdown
[not a link](./skip.md)
```

[real link](./real.md)
"""
        result = parser.parse(code, "doc.md")
        modules = {i.module for i in result.imports}
        assert "./skip.md" not in modules
        assert "./real.md" in modules


# ---------------------------------------------------------------------------
# Edge cases
# ---------------------------------------------------------------------------


class TestEdgeCases:
    def test_empty_file(self, parser: MarkdownParser) -> None:
        result = parser.parse("", "empty.md")
        assert result.symbols == []
        assert result.imports == []
        assert result.calls == []
        assert result.exports == []

    def test_no_headings(self, parser: MarkdownParser) -> None:
        code = "Just some plain text without any headings.\n"
        result = parser.parse(code, "plain.md")
        assert result.symbols == []
        assert result.exports == []

    def test_code_block_without_language_no_call(self, parser: MarkdownParser) -> None:
        code = "# Doc\n\n```\nsome code\n```\n"
        result = parser.parse(code, "doc.md")
        # Fence without language tag should not produce a call with empty name
        calls_with_name = [c for c in result.calls if c.name]
        assert len(calls_with_name) == 0

    def test_section_content_includes_heading(self, parser: MarkdownParser) -> None:
        code = "# Title\n\nParagraph text.\n"
        result = parser.parse(code, "doc.md")
        assert len(result.symbols) == 1
        assert "Title" in result.symbols[0].content

    def test_single_heading_only(self, parser: MarkdownParser) -> None:
        code = "# Just a Title\n"
        result = parser.parse(code, "doc.md")
        assert len(result.symbols) == 1
        assert result.symbols[0].name == "Just a Title"
        assert result.symbols[0].kind == "section"
