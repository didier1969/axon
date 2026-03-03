"""Phase: Test coverage marking.

Marks symbols as tested=True if they are called or imported from a test file.
Uses the existing _is_test_file heuristic from dead_code.py.
"""

from __future__ import annotations

import logging

from axon.core.graph.graph import KnowledgeGraph
from axon.core.graph.model import RelType
from axon.core.ingestion.dead_code import _is_test_file

logger = logging.getLogger(__name__)


def process_test_coverage(graph: KnowledgeGraph) -> int:
    """Mark symbols as tested if called or imported from a test file.

    Two signals are used:
    - CALLS from a test file → mark the called symbol as tested.
    - IMPORTS from a test file → mark all symbols DEFINED in the
      imported file as tested (file-level coverage signal).

    Args:
        graph: The knowledge graph to scan and mutate.

    Returns:
        The number of symbols newly marked as tested.
    """
    marked: set[str] = set()

    for rel in graph.get_relationships_by_type(RelType.CALLS):
        source = graph.get_node(rel.source)
        if source and _is_test_file(source.file_path):
            target = graph.get_node(rel.target)
            if target and not target.tested:
                target.tested = True
                marked.add(target.id)
                logger.debug("Marked tested (CALLS): %s", target.id)

    for rel in graph.get_relationships_by_type(RelType.IMPORTS):
        source = graph.get_node(rel.source)
        if source and _is_test_file(source.file_path):
            for define_rel in graph.get_outgoing(rel.target, RelType.DEFINES):
                symbol = graph.get_node(define_rel.target)
                if symbol and not symbol.tested:
                    symbol.tested = True
                    marked.add(symbol.id)
                    logger.debug("Marked tested (IMPORTS): %s", symbol.id)

    logger.info("Test coverage: %d symbols marked as tested.", len(marked))
    return len(marked)
