"""Tests for the CSS parser."""

from __future__ import annotations

import pytest

from axon.core.parsers.css_lang import CssParser


@pytest.fixture
def parser() -> CssParser:
    return CssParser()


CSS_FIXTURE = """\
@import url("reset.css");
@import "variables.css";

#main-container {
    display: flex;
    flex-direction: column;
}

.header {
    background-color: #333;
    color: white;
}

.nav-item {
    padding: 10px;
}

#sidebar {
    width: 250px;
}

.btn.primary {
    background: blue;
}
"""


class TestCssIdSelectors:
    def test_id_selectors_extracted(self, parser: CssParser) -> None:
        result = parser.parse(CSS_FIXTURE, "style.css")
        names = {s.name for s in result.symbols}
        assert "#main-container" in names
        assert "#sidebar" in names

    def test_id_kind_is_function(self, parser: CssParser) -> None:
        result = parser.parse(CSS_FIXTURE, "style.css")
        id_sel = [s for s in result.symbols if s.name == "#main-container"]
        assert len(id_sel) == 1
        assert id_sel[0].kind == "function"


class TestCssClassSelectors:
    def test_class_selectors_extracted(self, parser: CssParser) -> None:
        result = parser.parse(CSS_FIXTURE, "style.css")
        names = {s.name for s in result.symbols}
        assert ".header" in names
        assert ".nav-item" in names

    def test_class_kind_is_function(self, parser: CssParser) -> None:
        result = parser.parse(CSS_FIXTURE, "style.css")
        cls_sel = [s for s in result.symbols if s.name == ".header"]
        assert len(cls_sel) == 1
        assert cls_sel[0].kind == "function"


class TestCssImports:
    def test_import_url_extracted(self, parser: CssParser) -> None:
        result = parser.parse(CSS_FIXTURE, "style.css")
        modules = {i.module for i in result.imports}
        assert "reset.css" in modules

    def test_import_string_extracted(self, parser: CssParser) -> None:
        result = parser.parse(CSS_FIXTURE, "style.css")
        modules = {i.module for i in result.imports}
        assert "variables.css" in modules


class TestCssEdgeCases:
    def test_empty_file(self, parser: CssParser) -> None:
        result = parser.parse("", "empty.css")
        assert result.symbols == []
        assert result.imports == []

    def test_scss_extension(self, parser: CssParser) -> None:
        code = ".btn { color: red; }\n#app { display: block; }\n"
        result = parser.parse(code, "style.scss")
        names = {s.name for s in result.symbols}
        assert ".btn" in names
        assert "#app" in names
