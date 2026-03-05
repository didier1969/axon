from __future__ import annotations

from typing import TYPE_CHECKING

from tree_sitter import Language, Parser
import tree_sitter_java as tsjava

from axon.core.parsers.base import (
    CallInfo,
    ImportInfo,
    LanguageParser,
    ParseResult,
    SymbolInfo,
)

if TYPE_CHECKING:
    from tree_sitter import Node

JAVA_LANGUAGE = Language(tsjava.language())

class JavaParser(LanguageParser):
    """Parses Java source code using tree-sitter."""

    def __init__(self) -> None:
        self._parser = Parser()
        self._parser.set_language(JAVA_LANGUAGE)

    def parse(self, content: str, file_path: str = "") -> ParseResult:
        tree = self._parser.parse(bytes(content, "utf-8"))
        result = ParseResult()
        self._walk(tree.root_node, content, result, "")
        return result

    def _walk(self, node: Node, content: str, result: ParseResult, class_name: str) -> None:
        for child in node.children:
            if child.type == "class_declaration":
                self._extract_class(child, content, result)
            elif child.type == "method_declaration":
                self._extract_method(child, content, result, class_name)
            elif child.type == "import_declaration":
                self._extract_import(child, result)
            elif child.type == "method_invocation":
                self._extract_call(child, result)
            
            # Recurse for nested classes
            new_class = class_name
            if child.type == "class_declaration":
                name_node = child.child_by_field_name("name")
                if name_node:
                    new_class = name_node.text.decode("utf-8", errors="replace")
            
            self._walk(child, content, result, new_class)

    def _extract_class(self, node: Node, content: str, result: ParseResult) -> None:
        name_node = node.child_by_field_name("name")
        if not name_node:
            return
        name = name_node.text.decode("utf-8", errors="replace")
        
        result.symbols.append(SymbolInfo(
            name=name,
            kind="class",
            start_line=node.start_point[0] + 1,
            end_line=node.end_point[0] + 1,
            start_byte=node.start_byte,
            end_byte=node.end_byte,
            content=node.text.decode("utf-8", errors="replace")
        ))

    def _extract_method(self, node: Node, content: str, result: ParseResult, class_name: str) -> None:
        name_node = node.child_by_field_name("name")
        if not name_node:
            return
        name = name_node.text.decode("utf-8", errors="replace")
        
        result.symbols.append(SymbolInfo(
            name=name,
            kind="method",
            class_name=class_name,
            start_line=node.start_point[0] + 1,
            end_line=node.end_point[0] + 1,
            start_byte=node.start_byte,
            end_byte=node.end_byte,
            content=node.text.decode("utf-8", errors="replace")
        ))

    def _extract_import(self, node: Node, result: ParseResult) -> None:
        # Java imports look like: import com.foo.Bar;
        path_node = node.named_child(0)
        if path_node:
            path = path_node.text.decode("utf-8", errors="replace")
            result.imports.append(ImportInfo(module=path, names=[path.split(".")[-1]]))

    def _extract_call(self, node: Node, result: ParseResult) -> None:
        name_node = node.child_by_field_name("name")
        if name_node:
            name = name_node.text.decode("utf-8", errors="replace")
            receiver_node = node.child_by_field_name("object")
            receiver = receiver_node.text.decode("utf-8", errors="replace") if receiver_node else ""
            result.calls.append(CallInfo(name=name, receiver=receiver, line=node.start_point[0] + 1))
