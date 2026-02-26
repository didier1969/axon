"""Markdown parser (plain Python, no tree-sitter).

Extracts headings as symbols, Markdown links as imports, code-block language
tags as calls, and top-level headings as exports.
"""

from __future__ import annotations

import re

from axon.core.parsers.base import (
    CallInfo,
    ImportInfo,
    LanguageParser,
    ParseResult,
    SymbolInfo,
)

# Regex patterns
_HEADING_RE = re.compile(r"^(#{1,6})\s+(.+)$")
_LINK_RE = re.compile(r"\[([^\]]*)\]\(([^)]+)\)")
_CODE_FENCE_RE = re.compile(r"^```(\w+)")


class MarkdownParser(LanguageParser):
    """Parses Markdown files using line-by-line parsing."""

    def parse(self, content: str, file_path: str) -> ParseResult:
        """Parse Markdown content and return structured information."""
        result = ParseResult()

        if not content:
            return result

        lines = content.splitlines()
        total_lines = len(lines)

        # Collect heading positions: (line_number_1based, level, name)
        headings: list[tuple[int, int, str]] = []
        for i, line in enumerate(lines):
            m = _HEADING_RE.match(line)
            if m:
                level = len(m.group(1))
                name = m.group(2).strip()
                headings.append((i + 1, level, name))

        # Build symbols from headings with computed end_line
        for idx, (start_line, level, name) in enumerate(headings):
            # end_line is the line before the next heading, or end of file
            if idx + 1 < len(headings):
                end_line = headings[idx + 1][0] - 1
            else:
                end_line = total_lines

            # Extract content for this heading section
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

            # Top-level headings (#) are exports (document title)
            if level == 1:
                result.exports.append(name)

        # Extract links and code fences in a single pass
        in_code_block = False
        for i, line in enumerate(lines):
            line_no = i + 1

            # Toggle code block state
            fence_m = _CODE_FENCE_RE.match(line)
            if fence_m:
                if not in_code_block:
                    in_code_block = True
                    lang_tag = fence_m.group(1)
                    if lang_tag:
                        result.calls.append(
                            CallInfo(name=lang_tag, line=line_no)
                        )
                else:
                    in_code_block = False
                continue

            if line.strip() == "```" and in_code_block:
                in_code_block = False
                continue

            # Only extract links from non-code-block lines
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

        return result
