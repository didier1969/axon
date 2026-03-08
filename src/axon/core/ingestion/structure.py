"""Phase 2: Structure processing for Axon.

Takes a list of file entries (path, content, language) and populates the
knowledge graph with File and Folder nodes connected by CONTAINS relationships.
"""

from __future__ import annotations

from pathlib import PurePosixPath
from typing import TYPE_CHECKING, Generator, Union, Any

from axon.core.graph.model import (
    GraphNode,
    GraphRelationship,
    NodeLabel,
    RelType,
    generate_id,
)
from axon.core.ingestion.utils import add_to_graph

if TYPE_CHECKING:
    from axon.core.ingestion.walker import FileEntry
    from axon.core.graph.graph import KnowledgeGraph

def process_structure(files: list[FileEntry], graph: KnowledgeGraph | None = None) -> Any:
    """Yield/create File/Folder nodes and CONTAINS relationships from a list of files."""
    gen = _process_structure_generator(files, graph)
    if graph is not None:
        list(gen)
        return None
    return gen

def _process_structure_generator(files: list[FileEntry], graph: KnowledgeGraph | None = None) -> Generator[Union[GraphNode, GraphRelationship], None, None]:
    folder_paths: set[str] = set()
    yielded_ids: set[str] = set()

    for file_info in files:
        pure = PurePosixPath(file_info.path)
        for parent in pure.parents:
            parent_str = str(parent)
            if parent_str == ".":
                continue
            folder_paths.add(parent_str)

    for dir_path in sorted(list(folder_paths)):
        folder_id = generate_id(NodeLabel.FOLDER, dir_path)
        if folder_id not in yielded_ids:
            yielded_ids.add(folder_id)
            node = GraphNode(
                id=folder_id,
                label=NodeLabel.FOLDER,
                name=PurePosixPath(dir_path).name,
                file_path=dir_path,
            )
            yield add_to_graph(graph, node)

    for file_info in files:
        file_id = generate_id(NodeLabel.FILE, file_info.path)
        if file_id not in yielded_ids:
            yielded_ids.add(file_id)
            import hashlib
            content_hash = hashlib.sha256(file_info.content.encode("utf-8")).hexdigest()
            node = GraphNode(
                id=file_id,
                label=NodeLabel.FILE,
                name=PurePosixPath(file_info.path).name,
                file_path=file_info.path,
                content=file_info.content,
                language=file_info.language,
                properties={"content_hash": content_hash},
            )
            yield add_to_graph(graph, node)

    # Folder -> Folder (parent contains child)
    for dir_path in sorted(list(folder_paths)):
        pure = PurePosixPath(dir_path)
        parent_str = str(pure.parent)
        if parent_str == ".":
            continue
        parent_id = generate_id(NodeLabel.FOLDER, parent_str)
        child_id = generate_id(NodeLabel.FOLDER, dir_path)
        rel_id = f"contains:{parent_id}->{child_id}"
        rel = GraphRelationship(
            id=rel_id,
            type=RelType.CONTAINS,
            source=parent_id,
            target=child_id,
        )
        yield add_to_graph(graph, rel)

    # Folder -> File (immediate parent folder contains file)
    for file_info in files:
        pure = PurePosixPath(file_info.path)
        parent_str = str(pure.parent)
        if parent_str == ".":
            continue
        parent_id = generate_id(NodeLabel.FOLDER, parent_str)
        file_id = generate_id(NodeLabel.FILE, file_info.path)
        rel_id = f"contains:{parent_id}->{file_id}"
        rel = GraphRelationship(
            id=rel_id,
            type=RelType.CONTAINS,
            source=parent_id,
            target=file_id,
        )
        yield add_to_graph(graph, rel)
