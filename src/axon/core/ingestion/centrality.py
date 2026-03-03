"""Phase: Symbol centrality computation using PageRank.

Uses igraph (already a dependency via community detection) to compute
PageRank scores on the CALLS + IMPORTS graph. Scores are stored as
node.centrality (float) and used to boost search ranking.
"""

from __future__ import annotations

import logging

import igraph as ig

from axon.core.graph.graph import KnowledgeGraph
from axon.core.graph.model import NodeLabel, RelType

logger = logging.getLogger(__name__)

_RANKED_LABELS: tuple[NodeLabel, ...] = (
    NodeLabel.FUNCTION,
    NodeLabel.METHOD,
    NodeLabel.CLASS,
    NodeLabel.FILE,
    NodeLabel.INTERFACE,
)


def process_centrality(graph: KnowledgeGraph) -> None:
    """Compute PageRank centrality and store in node.centrality.

    Builds a directed graph from CALLS and IMPORTS relationships across
    all Function, Method, Class, File, and Interface nodes, then runs
    igraph's PageRank algorithm. Each node's centrality is set to its
    PageRank score (a float in [0.0, 1.0]).

    Nodes with no relationships retain centrality=0.0.

    Args:
        graph: The knowledge graph to scan and mutate.
    """
    node_id_to_index: dict[str, int] = {}
    index_to_node_id: dict[int, str] = {}

    for label in _RANKED_LABELS:
        for node in graph.get_nodes_by_label(label):
            idx = len(node_id_to_index)
            node_id_to_index[node.id] = idx
            index_to_node_id[idx] = node.id

    if not node_id_to_index:
        return

    edge_list: list[tuple[int, int]] = []
    for rel_type in (RelType.CALLS, RelType.IMPORTS):
        for rel in graph.get_relationships_by_type(rel_type):
            src_idx = node_id_to_index.get(rel.source)
            tgt_idx = node_id_to_index.get(rel.target)
            if src_idx is not None and tgt_idx is not None:
                edge_list.append((src_idx, tgt_idx))

    ig_graph = ig.Graph(directed=True)
    ig_graph.add_vertices(len(node_id_to_index))
    ig_graph.add_edges(edge_list)

    scores: list[float] = ig_graph.pagerank(directed=True)

    for idx, score in enumerate(scores):
        node_id = index_to_node_id.get(idx)
        if node_id:
            node = graph.get_node(node_id)
            if node:
                node.centrality = float(score)

    logger.info(
        "Centrality: PageRank computed for %d nodes.", len(node_id_to_index)
    )
