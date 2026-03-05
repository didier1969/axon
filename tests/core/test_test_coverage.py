"""Tests for the test coverage marking phase."""

from __future__ import annotations

from axon.core.graph.graph import KnowledgeGraph
from axon.core.graph.model import (
    GraphNode,
    GraphRelationship,
    NodeLabel,
    RelType,
    generate_id,
)
from axon.core.ingestion.test_coverage import process_test_coverage

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _add_file(graph: KnowledgeGraph, file_path: str) -> str:
    node_id = generate_id(NodeLabel.FILE, file_path)
    graph.add_node(GraphNode(id=node_id, label=NodeLabel.FILE, name=file_path, file_path=file_path))
    return node_id


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


def _add_imports(graph: KnowledgeGraph, source_id: str, target_id: str) -> None:
    graph.add_relationship(GraphRelationship(
        id=f"imports:{source_id}->{target_id}",
        type=RelType.IMPORTS,
        source=source_id,
        target=target_id,
    ))


def _add_defines(graph: KnowledgeGraph, file_id: str, symbol_id: str) -> None:
    graph.add_relationship(GraphRelationship(
        id=f"defines:{file_id}->{symbol_id}",
        type=RelType.DEFINES,
        source=file_id,
        target=symbol_id,
    ))


# ---------------------------------------------------------------------------
# Tests: CALLS-based coverage
# ---------------------------------------------------------------------------


class TestTestCoverageViaCalls:
    def test_called_from_test_file_marks_tested(self) -> None:
        """Symbol called from a test file is marked as tested."""
        graph = KnowledgeGraph()

        src_fn = _add_function(graph, "src/utils.py", "parse_item")
        test_fn = _add_function(graph, "tests/test_utils.py", "test_parse_item")
        _add_calls(graph, test_fn, src_fn)

        count = process_test_coverage(graph)

        node = graph.get_node(src_fn)
        assert node is not None
        assert node.tested is True
        assert count == 1

    def test_not_called_from_test_file_stays_untested(self) -> None:
        """Symbol with no callers from test files stays untested."""
        graph = KnowledgeGraph()

        helper_fn = _add_function(graph, "src/utils.py", "_internal_helper")

        count = process_test_coverage(graph)

        node = graph.get_node(helper_fn)
        assert node is not None
        assert node.tested is False
        assert count == 0

    def test_called_from_non_test_file_stays_untested(self) -> None:
        """A call from a non-test file does not mark the target as tested."""
        graph = KnowledgeGraph()

        src_fn = _add_function(graph, "src/utils.py", "helper")
        caller = _add_function(graph, "src/main.py", "main")
        _add_calls(graph, caller, src_fn)

        process_test_coverage(graph)

        node = graph.get_node(src_fn)
        assert node is not None
        assert node.tested is False

    def test_count_does_not_double_count(self) -> None:
        """Symbol called by two test functions is only counted once."""
        graph = KnowledgeGraph()

        src_fn = _add_function(graph, "src/utils.py", "parse_item")
        test_fn1 = _add_function(graph, "tests/test_a.py", "test_a")
        test_fn2 = _add_function(graph, "tests/test_b.py", "test_b")
        _add_calls(graph, test_fn1, src_fn)
        _add_calls(graph, test_fn2, src_fn)

        count = process_test_coverage(graph)
        assert count == 1


# ---------------------------------------------------------------------------
# Tests: IMPORTS-based coverage
# ---------------------------------------------------------------------------


class TestTestCoverageViaImports:
    def test_imported_file_symbols_marked_tested(self) -> None:
        """Symbols defined in a file imported by a test file are marked tested."""
        graph = KnowledgeGraph()

        src_file = _add_file(graph, "src/utils.py")
        test_file = _add_file(graph, "tests/test_utils.py")

        parse_fn = _add_function(graph, "src/utils.py", "parse_item")
        helper_fn = _add_function(graph, "src/utils.py", "_helper")

        _add_defines(graph, src_file, parse_fn)
        _add_defines(graph, src_file, helper_fn)
        _add_imports(graph, test_file, src_file)

        count = process_test_coverage(graph)

        assert graph.get_node(parse_fn).tested is True
        assert graph.get_node(helper_fn).tested is True
        assert count == 2

    def test_import_from_non_test_file_no_coverage(self) -> None:
        """Imports from non-test files do not trigger test coverage marking."""
        graph = KnowledgeGraph()

        src_file = _add_file(graph, "src/utils.py")
        main_file = _add_file(graph, "src/main.py")
        parse_fn = _add_function(graph, "src/utils.py", "parse_item")

        _add_defines(graph, src_file, parse_fn)
        _add_imports(graph, main_file, src_file)

        process_test_coverage(graph)

        assert graph.get_node(parse_fn).tested is False
