"""Elixir language parser using tree-sitter.

Extracts modules, functions, macros, structs, imports (alias/use/import/require),
heritage (use/behaviour), and function calls from Elixir source code.
"""

from __future__ import annotations

import tree_sitter_elixir as tselixir
from tree_sitter import Language, Node, Parser

from axon.core.parsers.base import (
    CallInfo,
    ImportInfo,
    LanguageParser,
    ParseResult,
    SymbolInfo,
)

ELIXIR_LANGUAGE = Language(tselixir.language())

# Directives that translate to import-like relationships
_IMPORT_DIRECTIVES = frozenset({"alias", "import", "use", "require"})

# Decorators relevant to OTP / Elixir
_OTP_ENTRY_POINTS = frozenset(
    {"handle_call", "handle_cast", "handle_info", "handle_continue", "init", "start_link"}
)


class ElixirParser(LanguageParser):
    """Parses Elixir source code using tree-sitter."""

    def __init__(self) -> None:
        self._parser = Parser(ELIXIR_LANGUAGE)

    def parse(self, content: str, file_path: str) -> ParseResult:
        """Parse Elixir source and return structured information."""
        tree = self._parser.parse(bytes(content, "utf8"))
        result = ParseResult()
        root = tree.root_node
        self._walk(root, content, result, module_name="", pending_attrs=[])
        return result

    # ------------------------------------------------------------------
    # Tree walking
    # ------------------------------------------------------------------

    def _walk(
        self,
        node: Node,
        content: str,
        result: ParseResult,
        module_name: str,
        pending_attrs: list[str],
    ) -> None:
        """Walk children of *node* and dispatch to extractors."""
        attrs: list[str] = list(pending_attrs)

        for child in node.children:
            match child.type:
                case "call":
                    self._handle_call_node(child, content, result, module_name, attrs)
                    attrs = []
                case "unary_operator":
                    # @attribute — collect for next definition
                    attr_name = self._extract_attribute_name(child)
                    if attr_name:
                        attrs.append(attr_name)
                case _:
                    # Recurse into other structural nodes
                    self._walk(child, content, result, module_name, attrs)
                    attrs = []

    def _handle_call_node(
        self,
        node: Node,
        content: str,
        result: ParseResult,
        module_name: str,
        pending_attrs: list[str],
    ) -> None:
        """Dispatch a call node to the appropriate extractor."""
        identifier = self._call_identifier(node)
        if identifier is None:
            # Could be a dotted call (e.g. Module.function/...) — extract as call
            self._extract_generic_call(node, result)
            return

        match identifier:
            case "defmodule":
                self._extract_module(node, content, result, pending_attrs)
            case "def":
                self._extract_function(node, content, result, module_name, pending_attrs, private=False)
            case "defp":
                self._extract_function(node, content, result, module_name, pending_attrs, private=True)
            case "defmacro":
                self._extract_macro(node, content, result, module_name, pending_attrs, private=False)
            case "defmacrop":
                self._extract_macro(node, content, result, module_name, pending_attrs, private=True)
            case "defstruct":
                self._extract_struct(node, content, result, module_name, pending_attrs)
            case _ if identifier in _IMPORT_DIRECTIVES:
                self._extract_import_directive(node, result, identifier, module_name)
            case _:
                # Regular function/macro call
                self._extract_generic_call(node, result)

    # ------------------------------------------------------------------
    # Symbol extractors
    # ------------------------------------------------------------------

    def _extract_module(
        self,
        node: Node,
        content: str,
        result: ParseResult,
        decorators: list[str],
    ) -> None:
        """Extract a defmodule definition."""
        args = node.child_by_field_name("arguments") if hasattr(node, "child_by_field_name") else None
        if args is None:
            args = self._find_child_by_type(node, "arguments")

        module_name = ""
        if args is not None:
            alias_node = self._find_child_by_type(args, "alias")
            if alias_node is not None:
                module_name = alias_node.text.decode("utf8")

        start_line = node.start_point[0] + 1
        end_line = node.end_point[0] + 1
        node_content = content[node.start_byte : node.end_byte]

        result.symbols.append(
            SymbolInfo(
                name=module_name,
                kind="module",
                start_line=start_line,
                end_line=end_line,
                content=node_content,
                decorators=list(decorators),
            )
        )

        # Walk the do-block body
        do_block = self._find_child_by_type(node, "do_block")
        if do_block is not None:
            self._walk(do_block, content, result, module_name=module_name, pending_attrs=[])

    def _extract_function(
        self,
        node: Node,
        content: str,
        result: ParseResult,
        module_name: str,
        decorators: list[str],
        private: bool,
    ) -> None:
        """Extract a def / defp definition."""
        func_name = self._extract_def_name(node)
        if not func_name:
            return

        start_line = node.start_point[0] + 1
        end_line = node.end_point[0] + 1
        node_content = content[node.start_byte : node.end_byte]

        # OTP entry-point detection — mark as decorator
        effective_decorators = list(decorators)
        if func_name in _OTP_ENTRY_POINTS:
            effective_decorators.append(func_name)

        result.symbols.append(
            SymbolInfo(
                name=func_name,
                kind="function",
                start_line=start_line,
                end_line=end_line,
                content=node_content,
                decorators=effective_decorators,
                class_name=module_name,
            )
        )

        # Public functions are exports
        if not private:
            result.exports.append(func_name)

        # Extract calls from the body
        do_block = self._find_child_by_type(node, "do_block")
        if do_block is not None:
            self._extract_calls_from_block(do_block, result)

    def _extract_macro(
        self,
        node: Node,
        content: str,
        result: ParseResult,
        module_name: str,
        decorators: list[str],
        private: bool,
    ) -> None:
        """Extract a defmacro / defmacrop definition."""
        macro_name = self._extract_def_name(node)
        if not macro_name:
            return

        start_line = node.start_point[0] + 1
        end_line = node.end_point[0] + 1
        node_content = content[node.start_byte : node.end_byte]

        result.symbols.append(
            SymbolInfo(
                name=macro_name,
                kind="macro",
                start_line=start_line,
                end_line=end_line,
                content=node_content,
                decorators=list(decorators),
                class_name=module_name,
            )
        )

        if not private:
            result.exports.append(macro_name)

        do_block = self._find_child_by_type(node, "do_block")
        if do_block is not None:
            self._extract_calls_from_block(do_block, result)

    def _extract_struct(
        self,
        node: Node,
        content: str,
        result: ParseResult,
        module_name: str,
        decorators: list[str],
    ) -> None:
        """Extract a defstruct definition."""
        start_line = node.start_point[0] + 1
        end_line = node.end_point[0] + 1
        node_content = content[node.start_byte : node.end_byte]

        result.symbols.append(
            SymbolInfo(
                name=module_name or "struct",
                kind="struct",
                start_line=start_line,
                end_line=end_line,
                content=node_content,
                decorators=list(decorators),
                class_name=module_name,
            )
        )

    # ------------------------------------------------------------------
    # Import / directive extractors
    # ------------------------------------------------------------------

    def _extract_import_directive(
        self,
        node: Node,
        result: ParseResult,
        directive: str,
        module_name: str,
    ) -> None:
        """Extract alias/import/use/require directives."""
        args = self._find_child_by_type(node, "arguments")
        if args is None:
            return

        # First argument is the module alias
        module_alias = ""
        as_alias = ""
        for child in args.children:
            if child.type == "alias":
                module_alias = child.text.decode("utf8")
                break

        # Check for `as:` keyword
        keywords_node = self._find_child_by_type(args, "keywords")
        if keywords_node is not None:
            for pair in keywords_node.children:
                if pair.type == "pair":
                    kw = self._find_child_by_type(pair, "keyword")
                    val = self._find_child_by_type(pair, "alias")
                    if kw is not None and kw.text.decode("utf8").rstrip(": ") == "as":
                        if val is not None:
                            as_alias = val.text.decode("utf8")

        if not module_alias:
            return

        result.imports.append(
            ImportInfo(
                module=module_alias,
                names=[],
                alias=as_alias,
            )
        )

        # Heritage: use -> "uses", @behaviour -> "implements"
        if directive == "use":
            result.heritage.append((module_name, "uses", module_alias))

    # ------------------------------------------------------------------
    # Call extraction
    # ------------------------------------------------------------------

    def _extract_calls_from_block(self, node: Node, result: ParseResult) -> None:
        """Recursively extract calls from a do-block or expression tree."""
        for child in node.children:
            if child.type == "call":
                identifier = self._call_identifier(child)
                if identifier in ("def", "defp", "defmodule", "defmacro", "defmacrop", "defstruct"):
                    # Don't recurse into nested definitions here — already handled
                    continue
                if identifier in _IMPORT_DIRECTIVES:
                    continue
                self._extract_generic_call(child, result)
            else:
                self._extract_calls_from_block(child, result)

    def _extract_generic_call(self, node: Node, result: ParseResult) -> None:
        """Extract a function/method call from a call node."""
        line = node.start_point[0] + 1

        # Dotted call: Module.function(args) — the function part is a "dot" node
        dot_node = self._find_child_by_type(node, "dot")
        if dot_node is not None:
            # dot: alias . identifier
            receiver = ""
            func_name = ""
            for child in dot_node.children:
                if child.type == "alias":
                    receiver = child.text.decode("utf8")
                elif child.type == "identifier":
                    func_name = child.text.decode("utf8")
            if func_name:
                result.calls.append(
                    CallInfo(name=func_name, line=line, receiver=receiver)
                )
            return

        # Simple call: function(args) or function args
        identifier = self._call_identifier(node)
        if identifier:
            result.calls.append(CallInfo(name=identifier, line=line))

    # ------------------------------------------------------------------
    # Attribute extraction (@impl, @doc, @spec, @behaviour)
    # ------------------------------------------------------------------

    def _extract_attribute_name(self, unary_node: Node) -> str | None:
        """Extract the attribute name from a @attr unary_operator node."""
        for child in unary_node.children:
            if child.type == "call":
                ident = self._call_identifier(child)
                if ident:
                    # For @behaviour, also add heritage later (handled in _walk)
                    return f"@{ident}"
        return None

    def _handle_behaviour_attribute(
        self,
        unary_node: Node,
        result: ParseResult,
        module_name: str,
    ) -> None:
        """Extract @behaviour SomeBehaviour → heritage."""
        for child in unary_node.children:
            if child.type == "call":
                ident = self._call_identifier(child)
                if ident == "behaviour":
                    args = self._find_child_by_type(child, "arguments")
                    if args is not None:
                        alias_node = self._find_child_by_type(args, "alias")
                        if alias_node is not None:
                            behaviour_name = alias_node.text.decode("utf8")
                            result.heritage.append((module_name, "implements", behaviour_name))

    # ------------------------------------------------------------------
    # Override _walk to also handle @behaviour
    # ------------------------------------------------------------------

    def _walk(  # type: ignore[override]  # noqa: F811
        self,
        node: Node,
        content: str,
        result: ParseResult,
        module_name: str,
        pending_attrs: list[str],
    ) -> None:
        """Walk children, dispatching to extractors and collecting attributes."""
        attrs: list[str] = list(pending_attrs)

        for child in node.children:
            match child.type:
                case "call":
                    self._handle_call_node(child, content, result, module_name, attrs)
                    attrs = []
                case "unary_operator":
                    # @attribute
                    attr_name = self._extract_attribute_name(child)
                    if attr_name:
                        attrs.append(attr_name)
                    # Check for @behaviour specifically to add heritage
                    self._handle_behaviour_attribute(child, result, module_name)
                case _:
                    self._walk(child, content, result, module_name, attrs)
                    attrs = []

    # ------------------------------------------------------------------
    # Helpers
    # ------------------------------------------------------------------

    def _call_identifier(self, node: Node) -> str | None:
        """Return the identifier text of a call node, or None if dotted/complex."""
        for child in node.children:
            if child.type == "identifier":
                return child.text.decode("utf8")
            if child.type == "dot":
                return None  # Dotted call
        return None

    def _extract_def_name(self, node: Node) -> str:
        """Extract function name from def/defp/defmacro/defmacrop call node."""
        args = self._find_child_by_type(node, "arguments")
        if args is None:
            return ""
        # args contains either: alias | identifier | call(name, params)
        for child in args.children:
            if child.type == "call":
                ident = self._find_child_by_type(child, "identifier")
                if ident is not None:
                    return ident.text.decode("utf8")
            if child.type == "identifier":
                return child.text.decode("utf8")
            if child.type == "alias":
                return child.text.decode("utf8")
        return ""

    @staticmethod
    def _find_child_by_type(node: Node, type_name: str) -> Node | None:
        """Return first direct child of *node* with type *type_name*."""
        for child in node.children:
            if child.type == type_name:
                return child
        return None
