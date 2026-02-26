"""Rust language parser using tree-sitter.

Extracts functions, structs, enums, traits, impl blocks, type aliases, modules,
imports (use declarations), heritage (impl Trait for Struct), and calls from Rust
source code.
"""

from __future__ import annotations

import tree_sitter_rust as tsrust
from tree_sitter import Language, Node, Parser

from axon.core.parsers.base import (
    CallInfo,
    ImportInfo,
    LanguageParser,
    ParseResult,
    SymbolInfo,
)

RUST_LANGUAGE = Language(tsrust.language())


class RustParser(LanguageParser):
    """Parses Rust source code using tree-sitter."""

    def __init__(self) -> None:
        self._parser = Parser(RUST_LANGUAGE)

    def parse(self, content: str, file_path: str) -> ParseResult:
        """Parse Rust source and return structured information."""
        tree = self._parser.parse(bytes(content, "utf8"))
        result = ParseResult()
        root = tree.root_node
        self._walk(root, content, result, class_name="")
        return result

    # ------------------------------------------------------------------
    # Tree walking
    # ------------------------------------------------------------------

    def _walk(
        self,
        node: Node,
        content: str,
        result: ParseResult,
        class_name: str,
    ) -> None:
        """Walk children of *node* and dispatch to extractors."""
        for child in node.children:
            match child.type:
                case "function_item":
                    self._extract_function(child, content, result, class_name)
                case "function_signature_item":
                    self._extract_function_signature(child, content, result, class_name)
                case "struct_item":
                    self._extract_struct(child, content, result)
                case "enum_item":
                    self._extract_enum(child, content, result)
                case "trait_item":
                    self._extract_trait(child, content, result)
                case "impl_item":
                    self._extract_impl(child, content, result)
                case "mod_item":
                    self._extract_mod(child, content, result)
                case "type_item":
                    self._extract_type_alias(child, content, result)
                case "use_declaration":
                    self._extract_use(child, result)
                case "call_expression":
                    self._extract_call_expression(child, result)
                case "method_call_expression":
                    self._extract_method_call(child, result)
                case "macro_invocation":
                    self._extract_macro_invocation(child, result)
                case _:
                    self._walk(child, content, result, class_name)

    # ------------------------------------------------------------------
    # Symbol extractors
    # ------------------------------------------------------------------

    def _extract_function(
        self,
        node: Node,
        content: str,
        result: ParseResult,
        class_name: str,
    ) -> None:
        """Extract a fn item."""
        name_node = self._find_child_by_type(node, "identifier")
        if name_node is None:
            return

        name = name_node.text.decode("utf8")
        is_pub = self._has_visibility(node)
        start_line = node.start_point[0] + 1
        end_line = node.end_point[0] + 1
        node_content = content[node.start_byte : node.end_byte]
        kind = "method" if class_name else "function"

        result.symbols.append(
            SymbolInfo(
                name=name,
                kind=kind,
                start_line=start_line,
                end_line=end_line,
                content=node_content,
                class_name=class_name,
            )
        )

        if is_pub and not class_name:
            result.exports.append(name)

        # Extract calls from body
        block = self._find_child_by_type(node, "block")
        if block is not None:
            self._walk(block, content, result, class_name=class_name)

    def _extract_function_signature(
        self,
        node: Node,
        content: str,
        result: ParseResult,
        class_name: str,
    ) -> None:
        """Extract a fn signature (e.g. in traits)."""
        name_node = self._find_child_by_type(node, "identifier")
        if name_node is None:
            return

        name = name_node.text.decode("utf8")
        start_line = node.start_point[0] + 1
        end_line = node.end_point[0] + 1
        node_content = content[node.start_byte : node.end_byte]
        kind = "method" if class_name else "function"

        result.symbols.append(
            SymbolInfo(
                name=name,
                kind=kind,
                start_line=start_line,
                end_line=end_line,
                content=node_content,
                class_name=class_name,
            )
        )

    def _extract_struct(
        self,
        node: Node,
        content: str,
        result: ParseResult,
    ) -> None:
        """Extract a struct item."""
        name_node = self._find_child_by_type(node, "type_identifier")
        if name_node is None:
            return

        name = name_node.text.decode("utf8")
        is_pub = self._has_visibility(node)
        start_line = node.start_point[0] + 1
        end_line = node.end_point[0] + 1
        node_content = content[node.start_byte : node.end_byte]

        result.symbols.append(
            SymbolInfo(
                name=name,
                kind="struct",
                start_line=start_line,
                end_line=end_line,
                content=node_content,
            )
        )

        if is_pub:
            result.exports.append(name)

    def _extract_enum(
        self,
        node: Node,
        content: str,
        result: ParseResult,
    ) -> None:
        """Extract an enum item."""
        name_node = self._find_child_by_type(node, "type_identifier")
        if name_node is None:
            return

        name = name_node.text.decode("utf8")
        is_pub = self._has_visibility(node)
        start_line = node.start_point[0] + 1
        end_line = node.end_point[0] + 1
        node_content = content[node.start_byte : node.end_byte]

        result.symbols.append(
            SymbolInfo(
                name=name,
                kind="enum",
                start_line=start_line,
                end_line=end_line,
                content=node_content,
            )
        )

        if is_pub:
            result.exports.append(name)

    def _extract_trait(
        self,
        node: Node,
        content: str,
        result: ParseResult,
    ) -> None:
        """Extract a trait item."""
        name_node = self._find_child_by_type(node, "type_identifier")
        if name_node is None:
            return

        name = name_node.text.decode("utf8")
        is_pub = self._has_visibility(node)
        start_line = node.start_point[0] + 1
        end_line = node.end_point[0] + 1
        node_content = content[node.start_byte : node.end_byte]

        result.symbols.append(
            SymbolInfo(
                name=name,
                kind="interface",
                start_line=start_line,
                end_line=end_line,
                content=node_content,
            )
        )

        if is_pub:
            result.exports.append(name)

        # Walk trait body for function signatures
        decl_list = self._find_child_by_type(node, "declaration_list")
        if decl_list is not None:
            self._walk(decl_list, content, result, class_name=name)

    def _extract_impl(
        self,
        node: Node,
        content: str,
        result: ParseResult,
    ) -> None:
        """Extract an impl block, including impl Trait for Struct."""
        type_nodes = [c for c in node.children if c.type == "type_identifier"]

        struct_name = ""
        trait_name = ""

        # Check for "impl Trait for Struct" pattern
        has_for = any(c.type == "for" for c in node.children)
        if has_for and len(type_nodes) >= 2:
            trait_name = type_nodes[0].text.decode("utf8")
            struct_name = type_nodes[1].text.decode("utf8")
            result.heritage.append((struct_name, "implements", trait_name))
        elif len(type_nodes) == 1:
            struct_name = type_nodes[0].text.decode("utf8")

        # Walk methods in the impl block
        decl_list = self._find_child_by_type(node, "declaration_list")
        if decl_list is not None:
            self._walk(decl_list, content, result, class_name=struct_name)

    def _extract_mod(
        self,
        node: Node,
        content: str,
        result: ParseResult,
    ) -> None:
        """Extract a mod item."""
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
                kind="module",
                start_line=start_line,
                end_line=end_line,
                content=node_content,
            )
        )

        # Walk mod body
        decl_list = self._find_child_by_type(node, "declaration_list")
        if decl_list is not None:
            self._walk(decl_list, content, result, class_name="")

    def _extract_type_alias(
        self,
        node: Node,
        content: str,
        result: ParseResult,
    ) -> None:
        """Extract a type alias."""
        name_node = self._find_child_by_type(node, "type_identifier")
        if name_node is None:
            return

        name = name_node.text.decode("utf8")
        is_pub = self._has_visibility(node)
        start_line = node.start_point[0] + 1
        end_line = node.end_point[0] + 1
        node_content = content[node.start_byte : node.end_byte]

        result.symbols.append(
            SymbolInfo(
                name=name,
                kind="type_alias",
                start_line=start_line,
                end_line=end_line,
                content=node_content,
            )
        )

        if is_pub:
            result.exports.append(name)

    # ------------------------------------------------------------------
    # Import extractors
    # ------------------------------------------------------------------

    def _extract_use(self, node: Node, result: ParseResult) -> None:
        """Extract a use declaration."""
        # The use path is the second named child (after 'use' keyword)
        for child in node.children:
            if child.type in (
                "scoped_identifier",
                "scoped_use_list",
                "identifier",
                "use_wildcard",
            ):
                self._process_use_node(child, prefix="", result=result)
                return

    def _process_use_node(self, node: Node, prefix: str, result: ParseResult) -> None:
        """Recursively process a use path and emit ImportInfo entries."""
        if node.type == "scoped_identifier":
            # e.g. std::collections::HashMap
            full_path = node.text.decode("utf8")
            # The last segment is the imported name
            parts = full_path.replace("::", ".").split(".")
            module = "::".join(parts[:-1]) if len(parts) > 1 else full_path
            name = parts[-1]
            result.imports.append(
                ImportInfo(
                    module=full_path,
                    names=[name],
                )
            )

        elif node.type == "scoped_use_list":
            # e.g. foo::{A, B}
            # First child is the path prefix, then "::", then use_list
            path_prefix = ""
            for child in node.children:
                if child.type in ("scoped_identifier", "identifier"):
                    path_prefix = child.text.decode("utf8")
                elif child.type == "use_list":
                    self._process_use_list(child, prefix=path_prefix, result=result)

        elif node.type == "use_list":
            self._process_use_list(node, prefix=prefix, result=result)

        elif node.type == "identifier":
            full_path = f"{prefix}::{node.text.decode('utf8')}" if prefix else node.text.decode("utf8")
            result.imports.append(
                ImportInfo(
                    module=full_path,
                    names=[node.text.decode("utf8")],
                )
            )

    def _process_use_list(self, node: Node, prefix: str, result: ParseResult) -> None:
        """Process {A, B, C} use list."""
        names: list[str] = []
        for child in node.children:
            if child.type == "identifier":
                names.append(child.text.decode("utf8"))
            elif child.type in ("scoped_identifier", "scoped_use_list"):
                self._process_use_node(child, prefix=prefix, result=result)

        if names:
            result.imports.append(
                ImportInfo(
                    module=prefix,
                    names=names,
                )
            )

    # ------------------------------------------------------------------
    # Call extractors
    # ------------------------------------------------------------------

    def _extract_call_expression(self, node: Node, result: ParseResult) -> None:
        """Extract a call_expression node."""
        line = node.start_point[0] + 1

        func_node = node.children[0] if node.children else None
        if func_node is None:
            return

        if func_node.type == "identifier":
            result.calls.append(
                CallInfo(name=func_node.text.decode("utf8"), line=line)
            )
        elif func_node.type == "field_expression":
            # obj.method(args) — but tree-sitter Rust uses method_call_expression for this
            # This is for function pointer calls via field access
            field_id = self._find_child_by_type(func_node, "field_identifier")
            obj = func_node.children[0] if func_node.children else None
            name = field_id.text.decode("utf8") if field_id else ""
            receiver = obj.text.decode("utf8") if obj is not None else ""
            if name:
                result.calls.append(CallInfo(name=name, line=line, receiver=receiver))
        elif func_node.type == "scoped_identifier":
            # e.g. HashMap::new()
            full = func_node.text.decode("utf8")
            parts = full.split("::")
            name = parts[-1]
            receiver = "::".join(parts[:-1]) if len(parts) > 1 else ""
            result.calls.append(CallInfo(name=name, line=line, receiver=receiver))

        # Recurse into arguments for nested calls
        self._walk_for_calls(node, result, skip_first=True)

    def _extract_method_call(self, node: Node, result: ParseResult) -> None:
        """Extract a method_call_expression (receiver.method())."""
        line = node.start_point[0] + 1
        name_node = self._find_child_by_type(node, "field_identifier")
        if name_node is None:
            return
        name = name_node.text.decode("utf8")

        # Receiver is the first child
        receiver = ""
        if node.children:
            recv_node = node.children[0]
            if recv_node.type in ("identifier", "self"):
                receiver = recv_node.text.decode("utf8")

        result.calls.append(CallInfo(name=name, line=line, receiver=receiver))

        # Recurse into arguments
        self._walk_for_calls(node, result, skip_first=False)

    def _extract_macro_invocation(self, node: Node, result: ParseResult) -> None:
        """Extract a macro invocation like println! or vec!."""
        line = node.start_point[0] + 1
        name_node = self._find_child_by_type(node, "identifier")
        if name_node is None:
            return
        name = name_node.text.decode("utf8")
        result.calls.append(CallInfo(name=f"{name}!", line=line))

    def _walk_for_calls(self, node: Node, result: ParseResult, skip_first: bool) -> None:
        """Walk a node's children looking for more calls, optionally skipping first."""
        children = node.children[1:] if skip_first else node.children
        for child in children:
            match child.type:
                case "call_expression":
                    self._extract_call_expression(child, result)
                case "method_call_expression":
                    self._extract_method_call(child, result)
                case "macro_invocation":
                    self._extract_macro_invocation(child, result)
                case _:
                    self._walk_for_calls(child, result, skip_first=False)

    # ------------------------------------------------------------------
    # Helpers
    # ------------------------------------------------------------------

    @staticmethod
    def _has_visibility(node: Node) -> bool:
        """Return True if the node has a pub visibility modifier."""
        for child in node.children:
            if child.type == "visibility_modifier":
                return True
        return False

    @staticmethod
    def _find_child_by_type(node: Node, type_name: str) -> Node | None:
        """Return first direct child of *node* with type *type_name*."""
        for child in node.children:
            if child.type == type_name:
                return child
        return None
