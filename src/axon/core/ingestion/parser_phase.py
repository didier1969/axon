"""Phase 3: Code parsing for Axon.

Takes file entries from the walker, parses each one with the appropriate
tree-sitter parser, and adds symbol nodes (Function, Class, Method, Interface,
TypeAlias, Enum) to the knowledge graph with DEFINES relationships from File
to Symbol.
"""

from __future__ import annotations

import logging
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass
from typing import Any, Generator, Union

from axon.core.graph.graph import KnowledgeGraph
from axon.core.graph.model import (
    GraphNode,
    GraphRelationship,
    NodeLabel,
    RelType,
    generate_id,
)
from axon.core.ingestion.walker import FileEntry
from axon.core.parsers.base import LanguageParser, ParseResult

logger = logging.getLogger(__name__)

_KIND_TO_LABEL: dict[str, NodeLabel] = {
    "function": NodeLabel.FUNCTION,
    "class": NodeLabel.CLASS,
    "method": NodeLabel.METHOD,
    "interface": NodeLabel.INTERFACE,
    "type_alias": NodeLabel.TYPE_ALIAS,
    "enum": NodeLabel.ENUM,
    # Elixir
    "module": NodeLabel.CLASS,
    "macro": NodeLabel.FUNCTION,
    "struct": NodeLabel.CLASS,
    # Markdown
    "section": NodeLabel.FUNCTION,
}

@dataclass
class FileParseData:
    """Parse results for a single file, kept for later phases."""

    file_path: str
    language: str
    parse_result: ParseResult

_PARSER_CACHE: dict[str, LanguageParser] = {}

def get_parser(language: str) -> LanguageParser:
    """Return the appropriate tree-sitter parser for *language*."""
    cached = _PARSER_CACHE.get(language)
    if cached is not None:
        return cached

    if language == "python":
        from axon.core.parsers.python_lang import PythonParser
        parser = PythonParser()
    elif language == "typescript":
        from axon.core.parsers.typescript import TypeScriptParser
        parser = TypeScriptParser(dialect="typescript")
    elif language == "javascript":
        from axon.core.parsers.typescript import TypeScriptParser
        parser = TypeScriptParser(dialect="javascript")
    elif language == "elixir":
        from axon.core.parsers.elixir_lang import ElixirParser
        parser = ElixirParser()
    elif language == "rust":
        from axon.core.parsers.rust_lang import RustParser
        parser = RustParser()
    elif language == "markdown":
        from axon.core.parsers.markdown import MarkdownParser
        parser = MarkdownParser()
    elif language == "go":
        from axon.core.parsers.go_lang import GoParser
        parser = GoParser()
    elif language in ("yaml", "toml"):
        from axon.core.parsers.yaml_lang import YamlParser
        parser = YamlParser()
    elif language == "sql":
        from axon.core.parsers.sql_lang import SqlParser
        parser = SqlParser()
    elif language == "html":
        from axon.core.parsers.html_lang import HtmlParser
        parser = HtmlParser()
    elif language == "css":
        from axon.core.parsers.css_lang import CssParser
        parser = CssParser()
    elif language == "java":
        from axon.core.parsers.java_lang import JavaParser
        parser = JavaParser()
    else:
        raise ValueError(f"Unsupported language {language!r}")

    _PARSER_CACHE[language] = parser
    return parser

def _detect_language(file_path: str) -> str:
    """Infer language from a file's extension."""
    from pathlib import PurePosixPath
    suffix = PurePosixPath(file_path).suffix.lower()
    if suffix == ".py":
        return "python"
    if suffix in (".ts", ".tsx"):
        return "typescript"
    if suffix in (".js", ".jsx"):
        return "javascript"
    if suffix == ".ex" or suffix == ".exs":
        return "elixir"
    if suffix == ".rs":
        return "rust"
    if suffix == ".go":
        return "go"
    if suffix == ".java":
        return "java"
    if suffix == ".md":
        return "markdown"
    if suffix in (".yaml", ".yml"):
        return "yaml"
    if suffix == ".toml":
        return "toml"
    if suffix == ".sql":
        return "sql"
    if suffix == ".html":
        return "html"
    if suffix == ".css":
        return "css"
    return ""

def parse_file(file_path: str, content: str, language: str) -> FileParseData:
    """Parse a single file and return structured parse data."""
    try:
        parser = get_parser(language)
        result = parser.parse(content, file_path)
    except (RuntimeError, ValueError, OSError):
        logger.warning("Failed to parse %s (%s), skipping", file_path, language, exc_info=True)
        result = ParseResult()

    return FileParseData(file_path=file_path, language=language, parse_result=result)

def process_parsing(
    files: list[FileEntry],
    graph: KnowledgeGraph | None = None,
    max_workers: int | None = None,
) -> Any:
    """Parse files and return/yield results. Supports both list and generator return."""
    gen = _process_parsing_generator(files, graph, max_workers)
    
    # If graph is passed, we might be in a test expecting a list return
    if graph is not None:
        # Realize the generator to populate the graph and return the list of FileParseData
        results = list(gen)
        return [item for item in results if isinstance(item, FileParseData)]
    
    return gen

def _process_parsing_generator(
    files: list[FileEntry],
    graph: KnowledgeGraph | None = None,
    max_workers: int | None = None,
) -> Generator[Union[GraphNode, GraphRelationship, FileParseData], None, None]:
    # Phase 1: Parse all files in parallel.
    with ThreadPoolExecutor(max_workers=max_workers) as executor:
        all_parse_data = list(
            executor.map(
                lambda f: parse_file(f.path, f.content, f.language),
                files,
            )
        )

    def _output(item: Union[GraphNode, GraphRelationship]):
        if graph is not None:
            if isinstance(item, GraphNode): graph.add_node(item)
            else: graph.add_relationship(item)
        return item

    # Phase 2: Yield nodes/rels and FileParseData.
    for file_entry, parse_data in zip(files, all_parse_data):
        file_id = generate_id(NodeLabel.FILE, file_entry.path)
        exported_names: set[str] = set(parse_data.parse_result.exports)

        # Build class -> base class names for storing on class nodes.
        class_bases: dict[str, list[str]] = {}
        for cls_name, kind, parent_name in parse_data.parse_result.heritage:
            if kind == "extends":
                class_bases.setdefault(cls_name, []).append(parent_name)

        for symbol in parse_data.parse_result.symbols:
            label = _KIND_TO_LABEL.get(symbol.kind)
            if label is None:
                continue

            symbol_name = (
                f"{symbol.class_name}.{symbol.name}"
                if symbol.kind == "method" and symbol.class_name
                else symbol.name
            )

            symbol_id = generate_id(label, file_entry.path, symbol_name)

            props: dict[str, Any] = {}
            if symbol.decorators:
                props["decorators"] = symbol.decorators
            if symbol.kind == "class" and symbol.name in class_bases:
                props["bases"] = class_bases[symbol.name]

            is_exported = symbol.name in exported_names

            node = GraphNode(
                id=symbol_id,
                label=label,
                name=symbol.name,
                file_path=file_entry.path,
                start_line=symbol.start_line,
                end_line=symbol.end_line,
                start_byte=symbol.start_byte,
                end_byte=symbol.end_byte,
                content=symbol.content,
                signature=symbol.signature,
                class_name=symbol.class_name,
                language=file_entry.language,
                is_exported=is_exported,
                properties=props,
            )
            yield _output(node)

            rel_id = f"defines:{file_id}->{symbol_id}"
            rel = GraphRelationship(
                id=rel_id,
                type=RelType.DEFINES,
                source=file_id,
                target=symbol_id,
            )
            yield _output(rel)

        yield parse_data
