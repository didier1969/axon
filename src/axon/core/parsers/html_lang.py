"""HTML parser using tree-sitter.

Extracts elements with id attributes as symbols, script/link sources as
imports, and anchor hrefs as calls.
"""

from __future__ import annotations

import tree_sitter_html as tshtml
from tree_sitter import Language, Node, Parser

from axon.core.parsers.base import (
    CallInfo,
    ImportInfo,
    LanguageParser,
    ParseResult,
    SymbolInfo,
)

HTML_LANGUAGE = Language(tshtml.language())


class HtmlParser(LanguageParser):
    """Parses HTML files using tree-sitter."""

    def __init__(self) -> None:
        self._parser = Parser(HTML_LANGUAGE)

    def parse(self, content: str, file_path: str) -> ParseResult:
        """Parse HTML content and return structured information."""
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
        """Walk the tree and extract elements, imports, and calls."""
        if node.type in ("element", "script_element", "style_element"):
            self._process_element(node, content, result)

        for child in node.children:
            self._walk(child, content, result)

    def _process_element(
        self, node: Node, content: str, result: ParseResult
    ) -> None:
        """Process an HTML element node."""
        start_tag = self._find_child_by_type(node, "start_tag")
        if start_tag is None:
            # Self-closing tag
            start_tag = self._find_child_by_type(node, "self_closing_tag")
        if start_tag is None:
            return

        tag_name = self._get_tag_name(start_tag)
        attrs = self._get_attributes(start_tag)

        start_line = node.start_point[0] + 1
        end_line = node.end_point[0] + 1

        # Elements with id attribute -> SymbolInfo
        if "id" in attrs:
            result.symbols.append(
                SymbolInfo(
                    name=attrs["id"],
                    kind="function",
                    start_line=start_line,
                    end_line=end_line,
                    content=content[node.start_byte : node.end_byte][:200],
                )
            )

        # <script src="..."> -> ImportInfo
        if tag_name == "script" and "src" in attrs:
            result.imports.append(
                ImportInfo(module=attrs["src"])
            )

        # <link href="..."> -> ImportInfo
        if tag_name == "link" and "href" in attrs:
            result.imports.append(
                ImportInfo(module=attrs["href"])
            )

        # <a href="..."> -> CallInfo
        if tag_name == "a" and "href" in attrs:
            result.calls.append(
                CallInfo(name=attrs["href"], line=start_line)
            )

    # ------------------------------------------------------------------
    # Helpers
    # ------------------------------------------------------------------

    def _get_tag_name(self, start_tag: Node) -> str:
        """Extract the tag name from a start_tag or self_closing_tag."""
        tag_node = self._find_child_by_type(start_tag, "tag_name")
        if tag_node is not None:
            return tag_node.text.decode("utf8").lower()
        return ""

    def _get_attributes(self, start_tag: Node) -> dict[str, str]:
        """Extract all attributes from a start_tag as a dict."""
        attrs: dict[str, str] = {}
        for child in start_tag.children:
            if child.type == "attribute":
                attr_name = ""
                attr_value = ""
                for ac in child.children:
                    if ac.type == "attribute_name":
                        attr_name = ac.text.decode("utf8").lower()
                    elif ac.type == "quoted_attribute_value":
                        # Strip surrounding quotes
                        raw = ac.text.decode("utf8")
                        attr_value = raw.strip("\"'")
                    elif ac.type == "attribute_value":
                        attr_value = ac.text.decode("utf8")
                if attr_name:
                    attrs[attr_name] = attr_value
        return attrs

    @staticmethod
    def _find_child_by_type(node: Node, type_name: str) -> Node | None:
        """Return first direct child of *node* with type *type_name*."""
        for child in node.children:
            if child.type == type_name:
                return child
        return None
