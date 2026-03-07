"""Base parser interface and shared data structures.

Defines the intermediate representation produced by language-specific parsers
before the data is mapped into the knowledge graph.
"""

from __future__ import annotations

from abc import ABC, abstractmethod
from dataclasses import dataclass, field


@dataclass
class SymbolInfo:
    """A parsed symbol (function, class, method, etc.)."""

    name: str
    kind: str  # "function", "class", "method", "interface", "type_alias", "enum"
    start_line: int
    end_line: int
    start_byte: int = 0
    end_byte: int = 0
    content: str = ""
    signature: str = ""
    class_name: str = ""  # for methods: the owning class
    is_entry_point: bool = False
    # e.g. ["staticmethod", "server.list_tools"]
    decorators: list[str] = field(default_factory=list)
    properties: dict[str, Any] = field(default_factory=dict)

@dataclass
class ImportInfo:
    """A parsed import statement."""

    module: str  # the module path (e.g., "os.path", "./utils")
    names: list[str] = field(default_factory=list)  # imported names (e.g., ["join", "exists"])
    is_relative: bool = False
    alias: str = ""

@dataclass
class CallInfo:
    """A parsed function call."""

    name: str  # the called function/method name
    line: int
    receiver: str = ""  # for method calls: the object (e.g., "self", "user")
    arguments: list[str] = field(default_factory=list)  # bare identifier arguments (callbacks)
    properties: dict[str, Any] = field(default_factory=dict)

@dataclass
class TypeRef:
    """A parsed type annotation reference."""

    name: str  # the type name (e.g., "User", "list", "str")
    kind: str  # "param", "return", "variable"
    line: int
    param_name: str = ""  # for param types: the parameter name

@dataclass
class ParseResult:
    """Complete parse result for a single file."""

    symbols: list[SymbolInfo] = field(default_factory=list)
    imports: list[ImportInfo] = field(default_factory=list)
    calls: list[CallInfo] = field(default_factory=list)
    type_refs: list[TypeRef] = field(default_factory=list)
    heritage: list[tuple[str, str, str]] = field(
        default_factory=list
    )  # (class_name, kind, parent_name) where kind is "extends" or "implements"
    exports: list[str] = field(default_factory=list)  # names from __all__ or export statements

class LanguageParser(ABC):
    """Base interface for language-specific parsers."""

    @abstractmethod
    def parse(self, content: str, file_path: str) -> ParseResult: ...

class TextParser(LanguageParser):
    """Fallback parser for plain text or unsupported files.
    
    Treats the whole file as a single 'text' symbol.
    """

    def parse(self, content: str, file_path: str) -> ParseResult:
        result = ParseResult()
        # Create a single symbol representing the whole file
        result.symbols.append(
            SymbolInfo(
                name=file_path.rsplit("/", 1)[-1],
                kind="function",
                start_line=1,
                end_line=content.count("\n") + 1,
                content=content[:500], # Preview content
            )
        )
        return result
