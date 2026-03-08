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
from axon.core.ingestion.utils import add_to_graph, get_node_label
from axon.core.ingestion.walker import FileEntry
from axon.core.parsers.base import LanguageParser, ParseResult

logger = logging.getLogger(__name__)

@dataclass
class FileParseData:
    """Parse results for a single file, kept for later phases."""

    file_path: str
    language: str
    parse_result: ParseResult

_PARSER_CACHE: dict[str, LanguageParser] = {}

def get_parser(language: str) -> LanguageParser | None:
    """Return the appropriate tree-sitter parser for *language*, or ``None``."""
    cached = _PARSER_CACHE.get(language)
    if cached is not None:
        return cached

    parser: LanguageParser | None = None
    try:
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
    except (ImportError, ValueError):
        from axon.core.parsers.base import TextParser
        parser = TextParser()

    if parser is None:
        from axon.core.parsers.base import TextParser
        parser = TextParser()

    _PARSER_CACHE[language] = parser
    return parser


def parse_file(file_path: str, content: str, language: str) -> FileParseData:
    """Parse a single file and return structured parse data."""
    parser = get_parser(language)
    if parser is None:
        return FileParseData(file_path=file_path, language=language, parse_result=ParseResult())

    try:
        result = parser.parse(content, file_path)
    except (RuntimeError, ValueError, OSError):
        logger.warning("Failed to parse %s (%s), indexing as plain text", file_path, language, exc_info=True)
        result = ParseResult()

    return FileParseData(file_path=file_path, language=language, parse_result=result)


def process_parsing(
    files: list[FileEntry],
    graph: KnowledgeGraph | None = None,
    max_workers: int | None = None,
) -> Any:
    """Parse files and return/yield results. Supports both list and generator return."""
    gen = _process_parsing_generator(files, graph, max_workers)
    
    if graph is not None:
        results = list(gen)
        return [item for item in results if isinstance(item, FileParseData)]
    
    return gen

def _process_parsing_generator(
    files: list[FileEntry],
    graph: KnowledgeGraph | None = None,
    max_workers: int | None = None,
) -> Generator[Union[GraphNode, GraphRelationship, FileParseData], None, None]:
    # Use executor.map to stream results as they are parsed,
    # avoiding a massive intermediate list of all results.
    with ThreadPoolExecutor(max_workers=max_workers) as executor:
        for parse_data in executor.map(
            lambda f: parse_file(f.path, f.content, f.language),
            files,
        ):
            file_id = generate_id(NodeLabel.FILE, parse_data.file_path)
            exported_names: set[str] = set(parse_data.parse_result.exports)

            class_bases: dict[str, list[str]] = {}
            for cls_name, kind, parent_name in parse_data.parse_result.heritage:
                if kind == "extends":
                    class_bases.setdefault(cls_name, []).append(parent_name)

            for symbol in parse_data.parse_result.symbols:
                label = get_node_label(symbol.kind)
                symbol_name = (
                    f"{symbol.class_name}.{symbol.name}"
                    if symbol.kind == "method" and symbol.class_name
                    else symbol.name
                )

                symbol_id = generate_id(label, parse_data.file_path, symbol_name)

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
                    file_path=parse_data.file_path,
                    start_line=symbol.start_line,
                    end_line=symbol.end_line,
                    start_byte=symbol.start_byte,
                    end_byte=symbol.end_byte,
                    content=symbol.content,
                    signature=symbol.signature,
                    class_name=symbol.class_name,
                    language=parse_data.language,
                    is_exported=is_exported,
                    properties=props,
                )
                yield add_to_graph(graph, node)

                rel_id = f"defines:{file_id}->{symbol_id}"
                rel = GraphRelationship(
                    id=rel_id,
                    type=RelType.DEFINES,
                    source=file_id,
                    target=symbol_id,
                )
                yield add_to_graph(graph, rel)

            yield parse_data
