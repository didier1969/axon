"""CSS parser using tree-sitter.

Extracts ID selectors, class selectors, and @import rules from CSS
(and SCSS) source files.
"""

from __future__ import annotations

import tree_sitter_css as tscss
from tree_sitter import Language, Node, Parser

from axon.core.parsers.base import (
    ImportInfo,
    LanguageParser,
    ParseResult,
    SymbolInfo,
)

CSS_LANGUAGE = Language(tscss.language())


class CssParser(LanguageParser):
    """Parses CSS/SCSS files using tree-sitter."""

    def __init__(self) -> None:
        self._parser = Parser(CSS_LANGUAGE)

    def parse(self, content: str, file_path: str) -> ParseResult:
        """Parse CSS content and return structured information."""
        result = ParseResult()

        if not content:
            return result

        tree = self._parser.parse(bytes(content, "utf8"))
        root = tree.root_node
        self._walk(root, content, result)
        return result

    # ------------------------------------------------------------------
    # Tree walking
    # ------------------------------------------------------------------

    def _walk(self, node: Node, content: str, result: ParseResult) -> None:
        """Walk tree and extract selectors and imports."""
        match node.type:
            case "id_selector":
                self._extract_id_selector(node, content, result)
            case "class_selector":
                self._extract_class_selector(node, content, result)
            case "import_statement":
                self._extract_import(node, result)

        for child in node.children:
            self._walk(child, content, result)

    # ------------------------------------------------------------------
    # Selector extractors
    # ------------------------------------------------------------------

    def _extract_id_selector(
        self, node: Node, content: str, result: ParseResult
    ) -> None:
        """Extract #id selector."""
        name_node = self._find_child_by_type(node, "id_name")
        if name_node is None:
            return

        name = f"#{name_node.text.decode('utf8')}"
        start_line = node.start_point[0] + 1
        end_line = node.end_point[0] + 1

        result.symbols.append(
            SymbolInfo(
                name=name,
                kind="function",
                start_line=start_line,
                end_line=end_line,
                content=content[node.start_byte : node.end_byte],
            )
        )

    def _extract_class_selector(
        self, node: Node, content: str, result: ParseResult
    ) -> None:
        """Extract .class selector."""
        name_node = self._find_child_by_type(node, "class_name")
        if name_node is None:
            return

        name = f".{name_node.text.decode('utf8')}"
        start_line = node.start_point[0] + 1
        end_line = node.end_point[0] + 1

        result.symbols.append(
            SymbolInfo(
                name=name,
                kind="function",
                start_line=start_line,
                end_line=end_line,
                content=content[node.start_byte : node.end_byte],
            )
        )

    # ------------------------------------------------------------------
    # Import extractors
    # ------------------------------------------------------------------

    def _extract_import(self, node: Node, result: ParseResult) -> None:
        """Extract @import statement."""
        for child in node.children:
            if child.type in ("string_value", "call_expression"):
                raw = child.text.decode("utf8")
                # Strip url(), quotes
                url = raw.strip()
                if url.startswith("url("):
                    url = url[4:].rstrip(")")
                url = url.strip("\"'")
                if url:
                    result.imports.append(ImportInfo(module=url))
                return

    # ------------------------------------------------------------------
    # Helpers
    # ------------------------------------------------------------------

    @staticmethod
    def _find_child_by_type(node: Node, type_name: str) -> Node | None:
        """Return first direct child of *node* with type *type_name*."""
        for child in node.children:
            if child.type == type_name:
                return child
        return None
