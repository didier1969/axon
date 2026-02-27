"""Go language parser using tree-sitter.

Extracts functions, structs, interfaces, methods, imports, and calls
from Go source code.
"""

from __future__ import annotations

import tree_sitter_go as tsgo
from tree_sitter import Language, Node, Parser

from axon.core.parsers.base import (
    CallInfo,
    ImportInfo,
    LanguageParser,
    ParseResult,
    SymbolInfo,
)

GO_LANGUAGE = Language(tsgo.language())


class GoParser(LanguageParser):
    """Parses Go source code using tree-sitter."""

    def __init__(self) -> None:
        self._parser = Parser(GO_LANGUAGE)

    def parse(self, content: str, file_path: str) -> ParseResult:
        """Parse Go source and return structured information."""
        if not content:
            return ParseResult()

        tree = self._parser.parse(bytes(content, "utf8"))
        result = ParseResult()
        root = tree.root_node
        self._walk(root, content, result)
        return result

    # ------------------------------------------------------------------
    # Tree walking
    # ------------------------------------------------------------------

    def _walk(self, node: Node, content: str, result: ParseResult) -> None:
        """Walk children of *node* and dispatch to extractors."""
        for child in node.children:
            match child.type:
                case "function_declaration":
                    self._extract_function(child, content, result)
                case "method_declaration":
                    self._extract_method(child, content, result)
                case "type_declaration":
                    self._extract_type_declaration(child, content, result)
                case "import_declaration":
                    self._extract_imports(child, result)
                case "call_expression":
                    self._extract_call(child, result)
                case _:
                    self._walk(child, content, result)

    # ------------------------------------------------------------------
    # Symbol extractors
    # ------------------------------------------------------------------

    def _extract_function(
        self, node: Node, content: str, result: ParseResult
    ) -> None:
        """Extract a function_declaration."""
        name_node = self._find_child_by_type(node, "identifier")
        if name_node is None:
            return

        name = name_node.text.decode("utf8")
        start_line = node.start_point[0] + 1
        end_line = node.end_point[0] + 1
        node_content = content[node.start_byte : node.end_byte]

        result.symbols.append(
            SymbolInfo(
                name=name,
                kind="function",
                start_line=start_line,
                end_line=end_line,
                content=node_content,
            )
        )

        # Exported if first letter is uppercase
        if name[0].isupper():
            result.exports.append(name)

        # Walk body for calls
        body = self._find_child_by_type(node, "block")
        if body is not None:
            self._walk_for_calls(body, result)

    def _extract_method(
        self, node: Node, content: str, result: ParseResult
    ) -> None:
        """Extract a method_declaration (func (receiver) name())."""
        name_node = self._find_child_by_type(node, "field_identifier")
        if name_node is None:
            return

        name = name_node.text.decode("utf8")
        start_line = node.start_point[0] + 1
        end_line = node.end_point[0] + 1
        node_content = content[node.start_byte : node.end_byte]

        # Extract receiver type
        receiver_type = ""
        param_list = self._find_child_by_type(node, "parameter_list")
        if param_list is not None:
            # Look for type_identifier in the receiver parameter
            for child in param_list.children:
                if child.type == "parameter_declaration":
                    type_node = self._find_child_by_type(child, "type_identifier")
                    if type_node is not None:
                        receiver_type = type_node.text.decode("utf8")
                    else:
                        # Pointer receiver: *Type
                        pointer_type = self._find_child_by_type(child, "pointer_type")
                        if pointer_type is not None:
                            inner = self._find_child_by_type(pointer_type, "type_identifier")
                            if inner is not None:
                                receiver_type = inner.text.decode("utf8")

        result.symbols.append(
            SymbolInfo(
                name=name,
                kind="method",
                start_line=start_line,
                end_line=end_line,
                content=node_content,
                class_name=receiver_type,
            )
        )

        if name[0].isupper():
            result.exports.append(name)

        body = self._find_child_by_type(node, "block")
        if body is not None:
            self._walk_for_calls(body, result)

    def _extract_type_declaration(
        self, node: Node, content: str, result: ParseResult
    ) -> None:
        """Extract type declarations (struct, interface, etc.)."""
        for child in node.children:
            if child.type == "type_spec":
                self._extract_type_spec(child, content, result)

    def _extract_type_spec(
        self, node: Node, content: str, result: ParseResult
    ) -> None:
        """Extract a single type_spec node."""
        name_node = self._find_child_by_type(node, "type_identifier")
        if name_node is None:
            return

        name = name_node.text.decode("utf8")
        start_line = node.start_point[0] + 1
        end_line = node.end_point[0] + 1
        node_content = content[node.start_byte : node.end_byte]

        # Determine kind from the type body
        kind = "type_alias"
        for child in node.children:
            if child.type == "struct_type":
                kind = "struct"
                break
            elif child.type == "interface_type":
                kind = "interface"
                break

        result.symbols.append(
            SymbolInfo(
                name=name,
                kind=kind,
                start_line=start_line,
                end_line=end_line,
                content=node_content,
            )
        )

        if name[0].isupper():
            result.exports.append(name)

    # ------------------------------------------------------------------
    # Import extractors
    # ------------------------------------------------------------------

    def _extract_imports(self, node: Node, result: ParseResult) -> None:
        """Extract import declarations."""
        for child in node.children:
            if child.type == "import_spec_list":
                for spec in child.children:
                    if spec.type == "import_spec":
                        self._extract_import_spec(spec, result)
            elif child.type == "import_spec":
                self._extract_import_spec(child, result)
            elif child.type == "interpreted_string_literal":
                path = child.text.decode("utf8").strip('"')
                result.imports.append(ImportInfo(module=path))

    def _extract_import_spec(self, node: Node, result: ParseResult) -> None:
        """Extract a single import spec."""
        alias = ""
        path = ""
        for child in node.children:
            if child.type == "package_identifier":
                alias = child.text.decode("utf8")
            elif child.type == "interpreted_string_literal":
                path = child.text.decode("utf8").strip('"')
            elif child.type == "dot":
                alias = "."

        if path:
            result.imports.append(ImportInfo(module=path, alias=alias))

    # ------------------------------------------------------------------
    # Call extractors
    # ------------------------------------------------------------------

    def _extract_call(self, node: Node, result: ParseResult) -> None:
        """Extract a call expression."""
        line = node.start_point[0] + 1
        func_node = node.children[0] if node.children else None
        if func_node is None:
            return

        if func_node.type == "identifier":
            result.calls.append(
                CallInfo(name=func_node.text.decode("utf8"), line=line)
            )
        elif func_node.type == "selector_expression":
            # pkg.Function() or obj.Method()
            field = self._find_child_by_type(func_node, "field_identifier")
            operand = func_node.children[0] if func_node.children else None
            name = field.text.decode("utf8") if field else ""
            receiver = operand.text.decode("utf8") if operand is not None else ""
            if name:
                result.calls.append(CallInfo(name=name, line=line, receiver=receiver))

        self._walk_for_calls(node, result, skip_first=True)

    def _walk_for_calls(
        self, node: Node, result: ParseResult, skip_first: bool = False
    ) -> None:
        """Walk for nested call expressions."""
        children = node.children[1:] if skip_first else node.children
        for child in children:
            if child.type == "call_expression":
                self._extract_call(child, result)
            else:
                self._walk_for_calls(child, result)

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
