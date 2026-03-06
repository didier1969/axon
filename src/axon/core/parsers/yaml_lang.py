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

        # Pre-compute cumulative byte offsets for each line (UTF-8).
        encoded_lines = [line.encode("utf-8") for line in content.splitlines(keepends=True)]
        line_start_bytes: list[int] = [0]
        for el in encoded_lines:
            line_start_bytes.append(line_start_bytes[-1] + len(el))

        if file_path.endswith(".toml"):
            self._parse_toml(content, result, line_start_bytes)
        else:
            self._parse_yaml(content, result, line_start_bytes)

        return result

    # ------------------------------------------------------------------
    # YAML parsing
    # ------------------------------------------------------------------

    def _parse_yaml(self, content: str, result: ParseResult, line_start_bytes: list[int]) -> None:
        """Extract keys from YAML content."""
        lines = content.splitlines()
        current_top_key = ""

        for i, line in enumerate(lines):
            line_no = i + 1
            start_byte = line_start_bytes[i] if i < len(line_start_bytes) else line_start_bytes[-1]
            end_byte = (
                line_start_bytes[i + 1] if i + 1 < len(line_start_bytes) else line_start_bytes[-1]
            )

            # Skip comments and empty lines
            stripped = line.lstrip()
            if not stripped or stripped.startswith("#"):
                continue

            # Top-level key
            m = _YAML_TOP_KEY_RE.match(line)
            if m:
                key = m.group(1)
                current_top_key = key
                
                # Expert: Detect sensitive keys or commands
                is_sensitive = any(s in key.lower() for s in ("secret", "password", "token", "key"))
                
                result.symbols.append(
                    SymbolInfo(
                        name=key,
                        kind="function",
                        start_line=line_no,
                        end_line=line_no,
                        start_byte=start_byte,
                        end_byte=end_byte,
                        content=line,
                        properties={"sensitive": is_sensitive}
                    )
                )
                continue

            # Nested key at depth 1 (2-space indent)
            m = _YAML_NESTED_KEY_RE.match(line)
            if m and current_top_key:
                child_key = m.group(1)
                
                is_sensitive = any(s in child_key.lower() for s in ("secret", "password", "token", "key"))
                
                result.symbols.append(
                    SymbolInfo(
                        name=f"{current_top_key}.{child_key}",
                        kind="function",
                        start_line=line_no,
                        end_line=line_no,
                        start_byte=start_byte,
                        end_byte=end_byte,
                        content=line,
                        properties={"sensitive": is_sensitive}
                    )
                )

    # ------------------------------------------------------------------
    # TOML parsing
    # ------------------------------------------------------------------

    def _parse_toml(self, content: str, result: ParseResult, line_start_bytes: list[int]) -> None:
        """Extract sections and keys from TOML content."""
        lines = content.splitlines()
        current_section = ""

        for i, line in enumerate(lines):
            line_no = i + 1
            start_byte = line_start_bytes[i] if i < len(line_start_bytes) else line_start_bytes[-1]
            end_byte = (
                line_start_bytes[i + 1] if i + 1 < len(line_start_bytes) else line_start_bytes[-1]
            )

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
                        start_byte=start_byte,
                        end_byte=end_byte,
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
                        start_byte=start_byte,
                        end_byte=end_byte,
                        content=line,
                    )
                )
