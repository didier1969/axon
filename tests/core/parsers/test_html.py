"""Tests for the HTML parser."""

from __future__ import annotations

import pytest

from axon.core.parsers.html_lang import HtmlParser


@pytest.fixture
def parser() -> HtmlParser:
    return HtmlParser()


HTML_FIXTURE = """\
<!DOCTYPE html>
<html>
<head>
    <title>My Page</title>
    <link rel="stylesheet" href="styles.css">
    <script src="app.js"></script>
</head>
<body>
    <div id="main-container">
        <h1 id="title">Hello World</h1>
        <nav id="navigation">
            <a href="/about">About</a>
            <a href="/contact">Contact</a>
            <a href="https://example.com">External</a>
        </nav>
    </div>
    <script src="vendor.js"></script>
</body>
</html>
"""


class TestHtmlElements:
    def test_id_elements_extracted(self, parser: HtmlParser) -> None:
        result = parser.parse(HTML_FIXTURE, "index.html")
        names = {s.name for s in result.symbols}
        assert "main-container" in names
        assert "title" in names
        assert "navigation" in names

    def test_id_kind_is_function(self, parser: HtmlParser) -> None:
        result = parser.parse(HTML_FIXTURE, "index.html")
        id_symbols = [s for s in result.symbols if s.name == "title"]
        assert len(id_symbols) == 1
        assert id_symbols[0].kind == "function"


class TestHtmlImports:
    def test_script_src_extracted(self, parser: HtmlParser) -> None:
        result = parser.parse(HTML_FIXTURE, "index.html")
        modules = {i.module for i in result.imports}
        assert "app.js" in modules
        assert "vendor.js" in modules

    def test_link_href_extracted(self, parser: HtmlParser) -> None:
        result = parser.parse(HTML_FIXTURE, "index.html")
        modules = {i.module for i in result.imports}
        assert "styles.css" in modules


class TestHtmlCalls:
    def test_anchor_hrefs_extracted(self, parser: HtmlParser) -> None:
        result = parser.parse(HTML_FIXTURE, "index.html")
        hrefs = {c.name for c in result.calls}
        assert "/about" in hrefs
        assert "/contact" in hrefs
        assert "https://example.com" in hrefs


class TestHtmlEdgeCases:
    def test_empty_file(self, parser: HtmlParser) -> None:
        result = parser.parse("", "empty.html")
        assert result.symbols == []
        assert result.imports == []
        assert result.calls == []

    def test_no_ids(self, parser: HtmlParser) -> None:
        code = "<html><body><p>Hello</p></body></html>"
        result = parser.parse(code, "simple.html")
        assert result.symbols == []
