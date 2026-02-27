"""Tests for the YAML/TOML parser."""

from __future__ import annotations

import pytest

from axon.core.parsers.yaml_lang import YamlParser


@pytest.fixture
def parser() -> YamlParser:
    return YamlParser()


YAML_FIXTURE = """\
name: my-project
version: 1.0.0
dependencies:
  flask: ">=2.0"
  requests: ">=2.28"
scripts:
  test: pytest
  lint: ruff check
"""

TOML_FIXTURE = """\
[project]
name = "axon"
version = "0.4.0"

[project.dependencies]
flask = ">=2.0"
requests = ">=2.28"

[tool.ruff]
target-version = "py311"
"""


class TestYamlTopLevelKeys:
    def test_top_keys_extracted(self, parser: YamlParser) -> None:
        result = parser.parse(YAML_FIXTURE, "config.yml")
        names = {s.name for s in result.symbols}
        assert "name" in names
        assert "version" in names
        assert "dependencies" in names
        assert "scripts" in names

    def test_kind_is_function(self, parser: YamlParser) -> None:
        result = parser.parse(YAML_FIXTURE, "config.yml")
        assert all(s.kind == "function" for s in result.symbols)


class TestYamlNestedKeys:
    def test_nested_keys_at_depth1(self, parser: YamlParser) -> None:
        result = parser.parse(YAML_FIXTURE, "config.yaml")
        names = {s.name for s in result.symbols}
        assert "dependencies.flask" in names
        assert "dependencies.requests" in names
        assert "scripts.test" in names
        assert "scripts.lint" in names


class TestTomlParsing:
    def test_sections_extracted(self, parser: YamlParser) -> None:
        result = parser.parse(TOML_FIXTURE, "pyproject.toml")
        names = {s.name for s in result.symbols}
        assert "project" in names
        assert "project.dependencies" in names
        assert "tool.ruff" in names

    def test_keys_under_sections(self, parser: YamlParser) -> None:
        result = parser.parse(TOML_FIXTURE, "pyproject.toml")
        names = {s.name for s in result.symbols}
        assert "project.name" in names
        assert "project.version" in names
        assert "tool.ruff.target-version" in names


class TestYamlEdgeCases:
    def test_empty_file(self, parser: YamlParser) -> None:
        result = parser.parse("", "empty.yml")
        assert result.symbols == []

    def test_comments_only(self, parser: YamlParser) -> None:
        result = parser.parse("# just a comment\n# another\n", "comments.yml")
        assert result.symbols == []

    def test_yaml_extension_detection(self, parser: YamlParser) -> None:
        result = parser.parse(YAML_FIXTURE, "config.yaml")
        assert len(result.symbols) > 0

    def test_toml_extension_detection(self, parser: YamlParser) -> None:
        result = parser.parse(TOML_FIXTURE, "config.toml")
        assert len(result.symbols) > 0
