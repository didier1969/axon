import logging
import time
from concurrent.futures import ProcessPoolExecutor
from dataclasses import dataclass
from pathlib import Path
from typing import List, Tuple

from axon.core.graph.graph import KnowledgeGraph
from axon.core.graph.model import GraphNode, GraphRelationship
from axon.core.ingestion.cross_repo import process_cross_repo_deps
from axon.core.ingestion.dead_code import process_dead_code
from axon.core.ingestion.heritage import process_heritage
from axon.core.ingestion.imports import process_imports
from axon.core.ingestion.structure import process_structure
from axon.core.ingestion.types import process_types
from axon.core.ingestion.calls import process_calls
from axon.core.ingestion.symbol_lookup import build_name_index
from axon.core.storage.base import NodeEmbedding, StorageBackend

logger = logging.getLogger("axon.core.ingestion.pipeline")


@dataclass
class PipelineResult:
    """Statistics for a pipeline run."""
    files_processed: int
    nodes_added: int
    relationships_added: int
    duration_sec: float

def run_pipeline(
    dir_path: str | Path,
    storage: StorageBackend,
    embeddings: bool = True,
    embed_model: str = "BAAI/bge-small-en-v1.5",
) -> PipelineResult:
    """Run the complete ingestion pipeline on a directory and write to storage.

    In v1.0, this prepares the nodes and basic relationships, but delegates
    heavy graph computations (centrality, community) to Pod C via bulk_load.
    """
    start_time = time.time()
    dir_path = Path(dir_path).resolve()
    
    # Exclude files ignored by git/gemini ignore
    from pathspec import PathSpec
    from pathspec.patterns import GitWildMatchPattern
    from axon.core.ingestion.utils import get_ignore_spec
    
    ignore_spec = get_ignore_spec(dir_path)

    # 1. Structure phase (Parse ASTs locally)
    logger.info(f"Phase 1: Indexing directory {dir_path}")
    graph = KnowledgeGraph()
    
    from axon.core.ingestion.walker import discover_files, process_files
    
    file_paths = discover_files(dir_path)
    file_entries = process_files(dir_path, file_paths)
    
    process_structure(file_entries, graph)
    
    # We also need to parse the files to get symbols
    from axon.core.ingestion.parser_phase import process_parsers
    process_parsers(file_entries, graph)
    
    files_processed = len(file_entries)
    
    if files_processed == 0:
        logger.warning(f"No files processed in {dir_path}.")
        return PipelineResult(0, 0, 0, time.time() - start_time)

    # 2. Relationship phase (Local resolution before transport)
    logger.info("Phase 2: Resolving local relationships")
    lookup = build_name_index(graph)
    process_imports(graph, dir_path)
    process_types(graph, lookup)
    process_heritage(graph, lookup)
    process_calls(graph, lookup)
    process_cross_repo_deps(graph, dir_path)
    
    # Dead code analysis is kept local to tag nodes before sending
    process_dead_code(graph)

    # 3. Export to Pod C (HydraDB)
    logger.info("Phase 3: Bulk load to Storage Backend (Pod C)")
    storage.bulk_load(graph.get_all_nodes(), graph.get_all_relationships())
    
    # 4. Optional Embeddings
    if embeddings:
        logger.info(f"Phase 4: Generating embeddings ({embed_model})")
        from axon.core.embeddings.embedder import NodeEmbedder
        embedder = NodeEmbedder(model_name=embed_model)
        embeds = embedder.embed_nodes(graph.get_all_nodes())
        storage.store_embeddings(embeds)
        
    duration = time.time() - start_time
    logger.info(f"Pipeline finished in {duration:.2f}s")
    
    return PipelineResult(
        files_processed=files_processed,
        nodes_added=len(graph.get_all_nodes()),
        relationships_added=len(graph.get_all_relationships()),
        duration_sec=duration
    )

def reindex_files(
    repo_path: Path,
    file_paths: list[str],
    storage: StorageBackend,
    embeddings: bool = True,
    embed_model: str = "BAAI/bge-small-en-v1.5",
) -> None:
    """Reindex a subset of files in an already indexed repository."""
    # Implementation simplified for v1.0, to be expanded if watcher uses Python directly.
    pass
