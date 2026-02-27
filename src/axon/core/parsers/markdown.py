"""Markdown parser using tree-sitter with frontmatter and table extraction.

Extracts headings as symbols, YAML frontmatter keys as symbols, pipe tables
as symbols, Markdown links as imports, and code-block language tags as calls.
"""

from __future__ import annotations

import re

import tree_sitter_markdown as tsmd
from tree_sitter import Language, Node, Parser

from axon.core.parsers.base import (
    CallInfo,
    ImportInfo,
    LanguageParser,
    ParseResult,
    SymbolInfo,
)

MD_LANGUAGE = Language(tsmd.language())

# Regex patterns for links and code fences (used in second pass)
_LINK_RE = re.compile(r"\[([^\]]*)\]\(([^)]+)\)")
_CODE_FENCE_RE = re.compile(r"^```(\w+)")

# Table line pattern: lines starting and ending with |
_TABLE_LINE_RE = re.compile(r"^\s*\|.+\|\s*$")
# Separator line: |---|---|
_TABLE_SEP_RE = re.compile(r"^\s*\|[\s:]*-+[\s:]*[-|\s:]*\|\s*$")


class MarkdownParser(LanguageParser):
    """Parses Markdown files using tree-sitter + regex for extras."""

    def __init__(self) -> None:
        self._parser = Parser(MD_LANGUAGE)

    def parse(self, content: str, file_path: str) -> ParseResult:
        """Parse Markdown content and return structured information."""
        result = ParseResult()

        if not content:
            return result

        lines = content.splitlines()
        total_lines = len(lines)

        # --- Pass 1: Extract YAML frontmatter ---
        frontmatter_end = 0
        frontmatter_end = self._extract_frontmatter(lines, result)

        # --- Pass 2: Tree-sitter for headings ---
        tree = self._parser.parse(bytes(content, "utf8"))
        root = tree.root_node
        self._extract_sections(root, content, lines, total_lines, result)

        # --- Pass 3: Extract tables ---
        self._extract_tables(lines, result)

        # --- Pass 4: Extract links and code fences ---
        self._extract_links_and_fences(lines, result)

        return result

    # ------------------------------------------------------------------
    # Frontmatter extraction
    # ------------------------------------------------------------------

    def _extract_frontmatter(
        self, lines: list[str], result: ParseResult
    ) -> int:
        """Extract YAML frontmatter between --- delimiters.

        Returns the line index (0-based) where frontmatter ends, or 0 if none.
        """
        if not lines or lines[0].strip() != "---":
            return 0

        end_idx = -1
        for i in range(1, len(lines)):
            if lines[i].strip() == "---":
                end_idx = i
                break

        if end_idx < 0:
            return 0

        # Parse YAML keys from frontmatter body
        for i in range(1, end_idx):
            line = lines[i]
            # Match top-level YAML keys: "key: value" or "key:"
            match = re.match(r"^([a-zA-Z_][\w.-]*)\s*:", line)
            if match:
                key = match.group(1)
                result.symbols.append(
                    SymbolInfo(
                        name=f"frontmatter:{key}",
                        kind="function",
                        start_line=i + 1,
                        end_line=i + 1,
                        content=line,
                    )
                )

        return end_idx + 1

    # ------------------------------------------------------------------
    # Tree-sitter section extraction
    # ------------------------------------------------------------------

    def _extract_sections(
        self,
        root: Node,
        content: str,
        lines: list[str],
        total_lines: int,
        result: ParseResult,
    ) -> None:
        """Extract heading sections from the tree-sitter AST."""
        headings: list[tuple[int, int, str]] = []
        self._collect_headings(root, headings)

        for idx, (start_line, level, name) in enumerate(headings):
            if idx + 1 < len(headings):
                end_line = headings[idx + 1][0] - 1
            else:
                end_line = total_lines

            section_lines = lines[start_line - 1 : end_line]
            section_content = "\n".join(section_lines)

            result.symbols.append(
                SymbolInfo(
                    name=name,
                    kind="section",
                    start_line=start_line,
                    end_line=end_line,
                    content=section_content,
                )
            )

            # Top-level headings (#) are exports
            if level == 1:
                result.exports.append(name)

    def _collect_headings(
        self, node: Node, headings: list[tuple[int, int, str]]
    ) -> None:
        """Recursively collect atx_heading nodes from the AST."""
        if node.type == "atx_heading":
            level = 0
            name = ""
            for child in node.children:
                if child.type.startswith("atx_h") and child.type.endswith("_marker"):
                    level = len(child.text.decode("utf8").strip())
                elif child.type == "inline":
                    name = child.text.decode("utf8").strip()
            if name:
                start_line = node.start_point[0] + 1
                headings.append((start_line, level, name))
            return

        for child in node.children:
            self._collect_headings(child, headings)

    # ------------------------------------------------------------------
    # Table extraction
    # ------------------------------------------------------------------

    def _extract_tables(
        self, lines: list[str], result: ParseResult
    ) -> None:
        """Extract pipe-delimited tables from Markdown content."""
        i = 0
        while i < len(lines):
            if _TABLE_LINE_RE.match(lines[i]):
                table_start = i
                # Scan forward for consecutive table lines
                j = i + 1
                while j < len(lines) and _TABLE_LINE_RE.match(lines[j]):
                    j += 1

                # Need at least 2 lines (header + separator or header + data)
                if j - table_start >= 2:
                    header_line = lines[table_start]
                    # Extract first column header
                    cells = [c.strip() for c in header_line.split("|")]
                    cells = [c for c in cells if c]
                    first_header = cells[0] if cells else "table"

                    result.symbols.append(
                        SymbolInfo(
                            name=f"table:{first_header}",
                            kind="section",
                            start_line=table_start + 1,
                            end_line=j,
                            content="\n".join(lines[table_start:j]),
                        )
                    )
                    i = j
                else:
                    i += 1
            else:
                i += 1

    # ------------------------------------------------------------------
    # Links and code fences
    # ------------------------------------------------------------------

    def _extract_links_and_fences(
        self, lines: list[str], result: ParseResult
    ) -> None:
        """Extract Markdown links and code fence language tags."""
        in_code_block = False
        for i, line in enumerate(lines):
            line_no = i + 1

            fence_m = _CODE_FENCE_RE.match(line)
            if fence_m:
                if not in_code_block:
                    in_code_block = True
                    lang_tag = fence_m.group(1)
                    if lang_tag:
                        result.calls.append(CallInfo(name=lang_tag, line=line_no))
                else:
                    in_code_block = False
                continue

            if line.strip() == "```" and in_code_block:
                in_code_block = False
                continue

            if not in_code_block:
                for link_m in _LINK_RE.finditer(line):
                    text = link_m.group(1)
                    url = link_m.group(2)
                    result.imports.append(
                        ImportInfo(
                            module=url,
                            names=[text] if text else [],
                        )
                    )
