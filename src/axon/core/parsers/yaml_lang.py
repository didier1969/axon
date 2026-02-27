"""YAML/TOML parser using line-based parsing (no tree-sitter).

Extracts top-level keys and nested keys at depth 1 from YAML files,
and section headers + key-value pairs from TOML files.
"""

from __future__ import annotations

import re

from axon.core.parsers.base import (
    LanguageParser,
    ParseResult,
    SymbolInfo,
)

# YAML patterns
_YAML_TOP_KEY_RE = re.compile(r"^([a-zA-Z_][\w.-]*)\s*:")
_YAML_NESTED_KEY_RE = re.compile(r"^  ([a-zA-Z_][\w.-]*)\s*:")

# TOML patterns
_TOML_SECTION_RE = re.compile(r"^\[([^\]]+)\]\s*$")
_TOML_KEY_VALUE_RE = re.compile(r"^([a-zA-Z_][\w.-]*)\s*=")


class YamlParser(LanguageParser):
    """Parses YAML and TOML files using line-based parsing."""

    def parse(self, content: str, file_path: str) -> ParseResult:
        """Parse YAML or TOML content and return structured information."""
        result = ParseResult()

        if not content:
            return result

        if file_path.endswith(".toml"):
            self._parse_toml(content, result)
        else:
            self._parse_yaml(content, result)

        return result

    # ------------------------------------------------------------------
    # YAML parsing
    # ------------------------------------------------------------------

    def _parse_yaml(self, content: str, result: ParseResult) -> None:
        """Extract keys from YAML content."""
        lines = content.splitlines()
        current_top_key = ""

        for i, line in enumerate(lines):
            line_no = i + 1

            # Skip comments and empty lines
            stripped = line.lstrip()
            if not stripped or stripped.startswith("#"):
                continue

            # Top-level key
            m = _YAML_TOP_KEY_RE.match(line)
            if m:
                key = m.group(1)
                current_top_key = key
                result.symbols.append(
                    SymbolInfo(
                        name=key,
                        kind="function",
                        start_line=line_no,
                        end_line=line_no,
                        content=line,
                    )
                )
                continue

            # Nested key at depth 1 (2-space indent)
            m = _YAML_NESTED_KEY_RE.match(line)
            if m and current_top_key:
                child_key = m.group(1)
                result.symbols.append(
                    SymbolInfo(
                        name=f"{current_top_key}.{child_key}",
                        kind="function",
                        start_line=line_no,
                        end_line=line_no,
                        content=line,
                    )
                )

    # ------------------------------------------------------------------
    # TOML parsing
    # ------------------------------------------------------------------

    def _parse_toml(self, content: str, result: ParseResult) -> None:
        """Extract sections and keys from TOML content."""
        lines = content.splitlines()
        current_section = ""

        for i, line in enumerate(lines):
            line_no = i + 1

            stripped = line.strip()
            if not stripped or stripped.startswith("#"):
                continue

            # Section header
            m = _TOML_SECTION_RE.match(stripped)
            if m:
                current_section = m.group(1)
                result.symbols.append(
                    SymbolInfo(
                        name=current_section,
                        kind="function",
                        start_line=line_no,
                        end_line=line_no,
                        content=line,
                    )
                )
                continue

            # Key = value
            m = _TOML_KEY_VALUE_RE.match(stripped)
            if m:
                key = m.group(1)
                name = f"{current_section}.{key}" if current_section else key
                result.symbols.append(
                    SymbolInfo(
                        name=name,
                        kind="function",
                        start_line=line_no,
                        end_line=line_no,
                        content=line,
                    )
                )
