"""SQL parser using regex-based parsing (no tree-sitter).

Extracts CREATE TABLE, CREATE VIEW, CREATE FUNCTION, and CREATE PROCEDURE
statements from SQL source files.
"""

from __future__ import annotations

import re

from axon.core.parsers.base import (
    CallInfo,
    LanguageParser,
    ParseResult,
    SymbolInfo,
)

# DDL patterns (case-insensitive)
_CREATE_TABLE_RE = re.compile(
    r"^\s*CREATE\s+TABLE\s+(?:IF\s+NOT\s+EXISTS\s+)?(?:`|\")?(\w+)(?:`|\")?",
    re.IGNORECASE | re.MULTILINE,
)
_CREATE_VIEW_RE = re.compile(
    r"^\s*CREATE\s+(?:OR\s+REPLACE\s+)?(?:MATERIALIZED\s+)?VIEW\s+(?:IF\s+NOT\s+EXISTS\s+)?(?:`|\")?(\w+)(?:`|\")?",
    re.IGNORECASE | re.MULTILINE,
)
_CREATE_FUNC_RE = re.compile(
    r"^\s*CREATE\s+(?:OR\s+REPLACE\s+)?FUNCTION\s+(?:`|\")?(\w+)(?:`|\")?",
    re.IGNORECASE | re.MULTILINE,
)
_CREATE_PROC_RE = re.compile(
    r"^\s*CREATE\s+(?:OR\s+REPLACE\s+)?PROCEDURE\s+(?:`|\")?(\w+)(?:`|\")?",
    re.IGNORECASE | re.MULTILINE,
)
_DROP_RE = re.compile(
    r"^\s*DROP\s+(?:TABLE|VIEW|FUNCTION|PROCEDURE)\s+(?:IF\s+EXISTS\s+)?(?:`|\")?(\w+)(?:`|\")?",
    re.IGNORECASE | re.MULTILINE,
)
_ALTER_RE = re.compile(
    r"^\s*ALTER\s+TABLE\s+(?:`|\")?(\w+)(?:`|\")?",
    re.IGNORECASE | re.MULTILINE,
)


class SqlParser(LanguageParser):
    """Parses SQL files using regex-based DDL extraction."""

    def parse(self, content: str, file_path: str) -> ParseResult:
        """Parse SQL content and return structured information."""
        result = ParseResult()

        if not content:
            return result

        lines = content.splitlines()

        # Extract CREATE TABLE
        for m in _CREATE_TABLE_RE.finditer(content):
            name = m.group(1)
            line_no = content[:m.start()].count("\n") + 1
            end_line = self._find_statement_end(lines, line_no - 1)
            stmt_content = "\n".join(lines[line_no - 1 : end_line])
            result.symbols.append(
                SymbolInfo(
                    name=name,
                    kind="class",
                    start_line=line_no,
                    end_line=end_line,
                    content=stmt_content,
                )
            )

        # Extract CREATE VIEW
        for m in _CREATE_VIEW_RE.finditer(content):
            name = m.group(1)
            line_no = content[:m.start()].count("\n") + 1
            end_line = self._find_statement_end(lines, line_no - 1)
            stmt_content = "\n".join(lines[line_no - 1 : end_line])
            result.symbols.append(
                SymbolInfo(
                    name=name,
                    kind="function",
                    start_line=line_no,
                    end_line=end_line,
                    content=stmt_content,
                )
            )

        # Extract CREATE FUNCTION
        for m in _CREATE_FUNC_RE.finditer(content):
            name = m.group(1)
            line_no = content[:m.start()].count("\n") + 1
            end_line = self._find_statement_end(lines, line_no - 1)
            stmt_content = "\n".join(lines[line_no - 1 : end_line])
            result.symbols.append(
                SymbolInfo(
                    name=name,
                    kind="function",
                    start_line=line_no,
                    end_line=end_line,
                    content=stmt_content,
                )
            )

        # Extract CREATE PROCEDURE
        for m in _CREATE_PROC_RE.finditer(content):
            name = m.group(1)
            line_no = content[:m.start()].count("\n") + 1
            end_line = self._find_statement_end(lines, line_no - 1)
            stmt_content = "\n".join(lines[line_no - 1 : end_line])
            result.symbols.append(
                SymbolInfo(
                    name=name,
                    kind="function",
                    start_line=line_no,
                    end_line=end_line,
                    content=stmt_content,
                )
            )

        # Extract DROP/ALTER as calls
        for m in _DROP_RE.finditer(content):
            name = m.group(1)
            line_no = content[:m.start()].count("\n") + 1
            result.calls.append(CallInfo(name=f"DROP:{name}", line=line_no))

        for m in _ALTER_RE.finditer(content):
            name = m.group(1)
            line_no = content[:m.start()].count("\n") + 1
            result.calls.append(CallInfo(name=f"ALTER:{name}", line=line_no))

        return result

    @staticmethod
    def _find_statement_end(lines: list[str], start_idx: int) -> int:
        """Find the end line of a SQL statement (line with ';' or end of file)."""
        for i in range(start_idx, len(lines)):
            if ";" in lines[i]:
                return i + 1
        return len(lines)
