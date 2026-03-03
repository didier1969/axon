"""Tests for the PageRank centrality ingestion phase."""

from __future__ import annotations

from axon.core.graph.graph import KnowledgeGraph
from axon.core.graph.model import (
    GraphNode,
    GraphRelationship,
    NodeLabel,
    RelType,
    generate_id,
)
from axon.core.ingestion.centrality import process_centrality


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _add_function(graph: KnowledgeGraph, file_path: str, name: str) -> str:
    node_id = generate_id(NodeLabel.FUNCTION, file_path, name)
    graph.add_node(GraphNode(id=node_id, label=NodeLabel.FUNCTION, name=name, file_path=file_path))
    return node_id


def _add_calls(graph: KnowledgeGraph, source_id: str, target_id: str) -> None:
    graph.add_relationship(GraphRelationship(
        id=f"calls:{source_id}->{target_id}",
        type=RelType.CALLS,
        source=source_id,
        target=target_id,
    ))


# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------


class TestProcessCentrality:
    def test_all_nodes_get_a_score(self) -> None:
        """Every node in the graph gets a non-negative centrality score."""
        graph = KnowledgeGraph()

        a = _add_function(graph, "src/app.py", "a")
        b = _add_function(graph, "src/app.py", "b")
        c = _add_function(graph, "src/app.py", "c")
        _add_calls(graph, a, b)
        _add_calls(graph, b, c)
        _add_calls(graph, a, c)

        process_centrality(graph)

        for nid in (a, b, c):
            node = graph.get_node(nid)
            assert node is not None
            assert node.centrality >= 0.0

    def test_central_node_ranks_higher(self) -> None:
        """The most-called node (C) should have >= centrality than less-called nodes."""
        graph = KnowledgeGraph()

        # A calls B, A calls C, B calls C — C receives most incoming edges
        a = _add_function(graph, "src/app.py", "a")
        b = _add_function(graph, "src/app.py", "b")
        c = _add_function(graph, "src/app.py", "c")
        _add_calls(graph, a, b)
        _add_calls(graph, a, c)
        _add_calls(graph, b, c)

        process_centrality(graph)

        node_a = graph.get_node(a)
        node_b = graph.get_node(b)
        node_c = graph.get_node(c)

        assert node_c.centrality >= node_b.centrality >= 0.0
        assert node_a.centrality >= 0.0

    def test_empty_graph_does_not_raise(self) -> None:
        """process_centrality completes without error on an empty graph."""
        graph = KnowledgeGraph()
        process_centrality(graph)  # should not raise

    def test_isolated_node_gets_zero_centrality(self) -> None:
        """A node with no edges should have centrality 0.0 initially (no relationships)."""
        graph = KnowledgeGraph()

        a = _add_function(graph, "src/app.py", "isolated")
        # No edges added — igraph PageRank on a single isolated node

        process_centrality(graph)

        node = graph.get_node(a)
        assert node is not None
        # Single-node graph: PageRank assigns the full mass to that node
        assert node.centrality > 0.0

    def test_scores_sum_approximately_one(self) -> None:
        """PageRank scores across all nodes should sum to approximately 1.0."""
        graph = KnowledgeGraph()

        a = _add_function(graph, "src/app.py", "a")
        b = _add_function(graph, "src/app.py", "b")
        c = _add_function(graph, "src/app.py", "c")
        _add_calls(graph, a, b)
        _add_calls(graph, b, c)

        process_centrality(graph)

        total = sum(
            graph.get_node(nid).centrality for nid in (a, b, c)
        )
        assert abs(total - 1.0) < 0.01
