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
from axon.core.parsers.utils import find_child_by_type

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
        start_tag = find_child_by_type(node, "start_tag")
        if start_tag is None:
            # Self-closing tag
            start_tag = find_child_by_type(node, "self_closing_tag")
        if start_tag is None:
            return

        tag_name = self._get_tag_name(start_tag)
        attrs = self._get_attributes(start_tag)

        start_line = node.start_point[0] + 1
        end_line = node.end_point[0] + 1

        # Elements with id or class -> SymbolInfo
        if "id" in attrs or "class" in attrs:
            name = attrs.get("id") or f".{attrs.get('class').split()[0]}"
            result.symbols.append(
                SymbolInfo(
                    name=name,
                    kind="element",
                    start_line=start_line,
                    end_line=end_line,
                    start_byte=node.start_byte,
                    end_byte=node.end_byte,
                    content=node.text.decode("utf-8", errors="replace")[:200],
                    properties={"tag": tag_name, "classes": attrs.get("class", "").split()}
                )
            )

        # Form inputs and fields -> Entry Points for OWASP
        if tag_name in ("input", "textarea", "select", "form"):
            result.symbols.append(
                SymbolInfo(
                    name=attrs.get("name") or attrs.get("id") or tag_name,
                    kind="field",
                    is_entry_point=True,
                    start_line=start_line,
                    end_line=end_line,
                    start_byte=node.start_byte,
                    end_byte=node.end_byte,
                    content=node.text.decode("utf-8", errors="replace")[:100],
                    properties={"tag": tag_name, "type": attrs.get("type", "text")}
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

        # Inline JS Events (onclick, onsubmit, etc.) -> CallInfo
        for attr_name, attr_value in attrs.items():
            if attr_name.startswith("on"):
                # Simplified: try to extract function name from "myFunc(event)"
                func_name = attr_value.split("(")[0].strip()
                if func_name:
                    result.calls.append(
                        CallInfo(name=func_name, line=start_line)
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
        tag_node = find_child_by_type(start_tag, "tag_name")
        if tag_node is not None:
            return tag_node.text.decode("utf-8", errors="replace").lower()
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
