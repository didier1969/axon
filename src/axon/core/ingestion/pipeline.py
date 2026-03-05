"""Pipeline orchestrator for Axon.

Runs ingestion phases in a streaming fashion to minimize memory usage.
"""

from __future__ import annotations

import hashlib
import logging
import time
import atexit
from collections.abc import Callable, Iterable
from concurrent.futures import Future, ThreadPoolExecutor
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Generator, Union

from axon.config.ignore import load_gitignore
from axon.core.analytics import log_event
from axon.core.graph.graph import KnowledgeGraph
from axon.core.graph.model import NodeLabel, GraphNode, GraphRelationship
from axon.core.embeddings.embedder import embed_graph
from axon.core.ingestion.calls import process_calls
from axon.core.ingestion.centrality import process_centrality
from axon.core.ingestion.community import process_communities
from axon.core.ingestion.coupling import process_coupling
from axon.core.ingestion.cross_repo import process_cross_repo_deps
from axon.core.ingestion.dead_code import process_dead_code
from axon.core.ingestion.heritage import process_heritage
from axon.core.ingestion.imports import process_imports
from axon.core.ingestion.parser_phase import process_parsing, FileParseData
from axon.core.ingestion.processes import process_processes
from axon.core.ingestion.structure import process_structure
from axon.core.ingestion.test_coverage import process_test_coverage
from axon.core.ingestion.types import process_types
from axon.core.ingestion.walker import FileEntry, walk_repo
from axon.core.storage.base import StorageBackend

@dataclass
class PhaseTimings:
    walk: float = 0.0
    structure: float = 0.0
    parsing: float = 0.0
    imports: float = 0.0
    calls: float = 0.0
    heritage: float = 0.0
    types: float = 0.0
    communities: float = 0.0
    centrality: float = 0.0
    processes: float = 0.0
    test_coverage: float = 0.0
    dead_code: float = 0.0
    coupling: float = 0.0
    cross_repo: float = 0.0
    storage_load: float = 0.0
    embeddings: float = 0.0

@dataclass
class PipelineResult:
    files: int = 0
    symbols: int = 0
    relationships: int = 0
    clusters: int = 0
    processes: int = 0
    dead_code: int = 0
    coupled_pairs: int = 0
    cross_repo_deps: int = 0
    embeddings: int = 0
    duration_seconds: float = 0.0
    incremental: bool = False
    changed_files: int = 0
    phase_timings: PhaseTimings = field(default_factory=PhaseTimings)
    embedding_future: Future | None = field(default=None, repr=False)

_SYMBOL_LABELS: frozenset[NodeLabel] = frozenset(NodeLabel) - {
    NodeLabel.FILE,
    NodeLabel.FOLDER,
    NodeLabel.COMMUNITY,
    NodeLabel.PROCESS,
}

_logger = logging.getLogger(__name__)
_EMBEDDING_POOL = ThreadPoolExecutor(max_workers=1, thread_name_prefix="axon-embed")
atexit.register(_EMBEDDING_POOL.shutdown, wait=False)

def _run_embeddings(
    graph: KnowledgeGraph,
    storage: StorageBackend,
    result: PipelineResult,
    report: Callable[[str, float], None],
) -> None:
    try:
        report("Generating embeddings", 0.0)
        _t = time.monotonic()
        node_embeddings = embed_graph(graph)
        storage.store_embeddings(node_embeddings)
        result.phase_timings.embeddings = time.monotonic() - _t
        result.embeddings = len(node_embeddings)
        report("Generating embeddings", 1.0)
    except Exception:
        _logger.warning("Embedding phase failed", exc_info=True)
        report("Generating embeddings", 1.0)

def run_pipeline(
    repo_path: Path,
    storage: StorageBackend | None = None,
    full: bool = False,
    progress_callback: Callable[[str, float], None] | None = None,
    embeddings: bool = True,
    wait_embeddings: bool = False,
) -> tuple[KnowledgeGraph, PipelineResult]:
    start = time.monotonic()
    result = PipelineResult()

    def report(phase: str, pct: float) -> None:
        if progress_callback is not None:
            progress_callback(phase, pct)

    report("Walking files", 0.0)
    gitignore = load_gitignore(repo_path)
    _t = time.monotonic()
    files = walk_repo(repo_path, gitignore)
    result.phase_timings.walk = time.monotonic() - _t
    result.files = len(files)
    report("Walking files", 1.0)

    # Note: Incremental path currently still uses full_graph in memory for simplicity
    # but we will eventually stream it too.
    if storage is not None and not full:
        manifest = storage.get_indexed_files()
        if manifest:
            current = {e.path: hashlib.sha256(e.content.encode()).hexdigest() for e in files}
            changed_or_new = [e for e in files if current[e.path] != manifest.get(e.path)]
            deleted_paths = [p for p in manifest if p not in current]

            if changed_or_new or deleted_paths:
                report("Loading existing graph", 0.0)
                full_graph = storage.export_to_graph()
                
                nodes_to_remove = []
                for node in full_graph.iter_nodes():
                    if node.file_path in deleted_paths or node.file_path in [e.path for e in changed_or_new]:
                        nodes_to_remove.append(node.id)
                for nid in nodes_to_remove:
                    full_graph.remove_node(nid)

                if changed_or_new:
                    # Use compatibility mode (passing graph=full_graph)
                    report("Processing structure", 0.0)
                    process_structure(changed_or_new, graph=full_graph)
                    report("Processing structure", 1.0)
                    
                    report("Parsing code", 0.0)
                    parse_data_list = process_parsing(changed_or_new, graph=full_graph)
                    report("Parsing code", 1.0)
                    
                    # File index for imports
                    from axon.core.ingestion.symbol_lookup import build_file_index
                    file_index = build_file_index(full_graph)
                    
                    report("Resolving imports", 0.0)
                    process_imports(parse_data_list, file_index, graph=full_graph)
                    report("Resolving imports", 1.0)
                    
                    report("Tracing calls", 0.0)
                    process_calls(parse_data_list, full_graph)
                    report("Tracing calls", 1.0)

                report("Detecting communities", 0.0)
                result.clusters = process_communities(full_graph)
                process_centrality(full_graph)
                result.processes = process_processes(full_graph)
                process_test_coverage(full_graph)
                result.dead_code = process_dead_code(full_graph)
                
                report("Loading to storage", 0.0)
                storage.bulk_load(full_graph)
                report("Loading to storage", 1.0)

            result.incremental = True
            result.changed_files = len(changed_or_new) + len(deleted_paths)
            result.duration_seconds = time.monotonic() - start
            return KnowledgeGraph(), result # Return empty graph for incremental

    # Full Index Path (Streaming)
    graph = KnowledgeGraph()

    report("Processing structure", 0.0)
    process_structure(files, graph=graph)
    report("Processing structure", 1.0)
    
    report("Parsing code", 0.0)
    parse_data_list = process_parsing(files, graph=graph)
    report("Parsing code", 1.0)

    report("Resolving imports", 0.0)
    from axon.core.ingestion.symbol_lookup import build_file_index
    file_index = build_file_index(graph)
    process_imports(parse_data_list, file_index, graph=graph)
    report("Resolving imports", 1.0)
    
    report("Tracing calls", 0.0)
    process_calls(parse_data_list, graph)
    report("Tracing calls", 1.0)

    report("Extracting heritage", 0.0)
    process_heritage(parse_data_list, graph)
    report("Extracting heritage", 1.0)

    report("Analyzing types", 0.0)
    process_types(parse_data_list, graph)
    report("Analyzing types", 1.0)

    report("Detecting communities", 0.0)
    result.clusters = process_communities(graph)
    report("Detecting communities", 1.0)
    report("Calculating centrality", 0.0)
    process_centrality(graph)
    report("Calculating centrality", 1.0)
    report("Detecting execution flows", 0.0)
    result.processes = process_processes(graph)
    report("Detecting execution flows", 1.0)
    process_test_coverage(graph)
    report("Finding dead code", 0.0)
    result.dead_code = process_dead_code(graph)
    report("Finding dead code", 1.0)
    report("Analyzing git history", 0.0)
    result.coupled_pairs = process_coupling(graph, repo_path)
    report("Analyzing git history", 1.0)
    result.cross_repo_deps = process_cross_repo_deps(graph, repo_path)

    result.symbols = sum(1 for n in graph.iter_nodes() if n.label in _SYMBOL_LABELS)
    result.relationships = graph.relationship_count

    if storage is not None:
        report("Loading to storage", 0.0)
        storage.bulk_load(graph)
        report("Loading to storage", 1.0)
        if embeddings:
            if wait_embeddings: _run_embeddings(graph, storage, result, report)
            else: result.embedding_future = _EMBEDDING_POOL.submit(_run_embeddings, graph, storage, result, report)

    result.duration_seconds = time.monotonic() - start
    return graph, result

def build_graph(repo_path: Path) -> KnowledgeGraph:
    graph, _ = run_pipeline(repo_path)
    return graph

def reindex_files(
    file_entries: list[FileEntry],
    repo_path: Path,
    storage: StorageBackend,
) -> KnowledgeGraph:
    """Re-index specific files (incremental helper)."""
    graph = KnowledgeGraph()
    def report(phase: str, pct: float) -> None: pass
    
    process_structure(file_entries, graph=graph)
    parse_data_list = process_parsing(file_entries, graph=graph)
    
    from axon.core.ingestion.symbol_lookup import build_file_index
    file_index = build_file_index(graph)
    process_imports(parse_data_list, file_index, graph=graph)
    process_calls(parse_data_list, graph)

    process_heritage(parse_data_list, graph)
    process_types(parse_data_list, graph)

    for entry in file_entries:
        storage.remove_nodes_by_file(entry.path)

    storage.bulk_load(graph)
    storage.rebuild_fts_indexes()

    return graph
