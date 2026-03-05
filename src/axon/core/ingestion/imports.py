"""Phase 4: Import resolution for Axon.

Takes the FileParseData produced by the parsing phase and resolves import
statements to actual File nodes in the knowledge graph, creating IMPORTS
relationships between the importing file and the imported file.
"""

from __future__ import annotations

import logging
from pathlib import PurePosixPath
from typing import Any, Generator, Iterable

from axon.core.graph.graph import KnowledgeGraph
from axon.core.graph.model import (
    GraphRelationship,
    NodeLabel,
    RelType,
    generate_id,
)
from axon.core.ingestion.parser_phase import FileParseData

logger = logging.getLogger(__name__)

# Extensions to try when resolving JS/TS imports without explicit extensions.
_JS_TS_EXTENSIONS = (".ts", ".tsx", ".js", ".jsx")

def process_imports(
    parse_data: Iterable[FileParseData],
    file_index: dict[str, str] | KnowledgeGraph | None = None,
    graph: KnowledgeGraph | None = None,
) -> Any:
    """Resolve imports and yield/create IMPORTS relationships.

    Args:
        parse_data: Parse results from the parsing phase.
        file_index: Mapping of relative file paths to their graph node IDs or a KnowledgeGraph.
        graph: Optional KnowledgeGraph to populate.
    """
    if isinstance(file_index, KnowledgeGraph):
        graph = file_index
        file_index = None

    if file_index is None and graph is not None:
        from axon.core.ingestion.symbol_lookup import build_file_index
        file_index = build_file_index(graph)
    
    if file_index is None:
        file_index = {}

    gen = _process_imports_generator(parse_data, file_index, graph)
    if graph is not None:
        list(gen) # Realize to populate
        return None
    return gen

def _process_imports_generator(
    parse_data: Iterable[FileParseData],
    file_index: dict[str, str],
    graph: KnowledgeGraph | None = None,
) -> Generator[GraphRelationship, None, None]:
    seen: set[tuple[str, str]] = set()

    def _output(item: GraphRelationship):
        if graph is not None:
            graph.add_relationship(item)
        return item

    for fpd in parse_data:
        source_file_id = generate_id(NodeLabel.FILE, fpd.file_path)

        for imp in fpd.parse_result.imports:
            target_id = resolve_import_path(fpd.file_path, imp, file_index)
            if target_id is None:
                continue

            pair = (source_file_id, target_id)
            if pair in seen:
                continue
            seen.add(pair)

            rel_id = f"imports:{source_file_id}->{target_id}"
            yield _output(GraphRelationship(
                id=rel_id,
                type=RelType.IMPORTS,
                source=source_file_id,
                target=target_id,
                properties={"symbols": ",".join(imp.names)},
            ))

def resolve_import_path(
    importing_file: str,
    import_info: Any,
    file_index: dict[str, str],
) -> str | None:
    """Resolve an import string to a file node ID using the file index."""
    # Language-specific resolution logic
    lang = _detect_language(importing_file)
    if lang == "python":
        return _resolve_python(importing_file, import_info, file_index)
    if lang in ("typescript", "javascript"):
        return _resolve_js_ts(importing_file, import_info, file_index)
    return None

def _resolve_python(
    importing_file: str,
    import_info: Any,
    file_index: dict[str, str],
) -> str | None:
    module = import_info.module
    if not module:
        return None

    # Handle relative imports
    if import_info.is_relative:
        # Simplified relative resolution
        parts = importing_file.split("/")[:-1]
        # remove dots from start
        dots = 0
        while module.startswith("."):
            dots += 1
            module = module[1:]
        
        for _ in range(dots - 1):
            if parts: parts.pop()
        
        base_path = "/".join(parts)
        if base_path:
            candidate = f"{base_path}/{module.replace('.', '/')}.py"
        else:
            candidate = f"{module.replace('.', '/')}.py"
            
        if candidate in file_index:
            return file_index[candidate]
        
        # Try as __init__.py
        candidate_init = candidate.replace(".py", "/__init__.py")
        if candidate_init in file_index:
            return file_index[candidate_init]

    # Global resolution (best effort)
    candidate = f"{module.replace('.', '/')}.py"
    if candidate in file_index:
        return file_index[candidate]
    
    candidate_init = f"{module.replace('.', '/')}/__init__.py"
    if candidate_init in file_index:
        return file_index[candidate_init]

    return None

def _resolve_js_ts(
    importing_file: str,
    import_info: Any,
    file_index: dict[str, str],
) -> str | None:
    module = import_info.module
    if not module or not (module.startswith("./") or module.startswith("../")):
        return None

    # Resolve relative to importing file
    importing_dir = str(PurePosixPath(importing_file).parent)
    base_path = str(PurePosixPath(importing_dir) / module)

    # 1. Try with exact extensions
    for ext in _JS_TS_EXTENSIONS:
        candidate = f"{base_path}{ext}"
        if candidate in file_index:
            return file_index[candidate]

    # 2. Try as file without extension (index.ts etc handled below)
    if base_path in file_index:
        return file_index[base_path]

    # 3. Try as directory with index file.
    for ext in _JS_TS_EXTENSIONS:
        candidate = f"{base_path}/index{ext}"
        if candidate in file_index:
            return file_index[candidate]

    return None

def _detect_language(file_path: str) -> str:
    """Infer language from a file's extension."""
    suffix = PurePosixPath(file_path).suffix.lower()
    if suffix == ".py":
        return "python"
    if suffix in (".ts", ".tsx"):
        return "typescript"
    if suffix in (".js", ".jsx"):
        return "javascript"
    return ""
