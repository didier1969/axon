"""Tests for Axon MCP tool handlers.

All tests mock the storage backend to avoid needing a real database.
Each tool handler is tested for both success and edge-case paths.
"""

from __future__ import annotations

import json
import tempfile
from pathlib import Path
from unittest.mock import MagicMock

import pytest

from axon.core.graph.model import GraphNode, NodeLabel
from axon.core.storage.base import SearchResult
from axon.mcp.tools import (
    _confidence_tag,
    _format_query_results,
    _group_by_process,
    _load_repo_storage,
    _sanitize_repo_slug,
    handle_context,
    handle_coverage_gaps,
    handle_cypher,
    handle_dead_code,
    handle_detect_changes,
    handle_entry_points,
    handle_find_similar,
    handle_find_usages,
    handle_impact,
    handle_lint,
    handle_list_repos,
    handle_path,
    handle_query,
    handle_read_symbol,
    handle_summarize,
)


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------


@pytest.fixture
def mock_storage():
    """Create a mock storage backend with common default return values."""
    storage = MagicMock()
    storage.fts_search.return_value = [
        SearchResult(
            node_id="function:src/auth.py:validate",
            score=1.0,
            node_name="validate",
            file_path="src/auth.py",
            label="function",
            snippet="def validate(user): ...",
        ),
    ]
    storage.get_node.return_value = GraphNode(
        id="function:src/auth.py:validate",
        label=NodeLabel.FUNCTION,
        name="validate",
        file_path="src/auth.py",
        start_line=10,
        end_line=30,
    )
    storage.get_callers.return_value = []
    storage.get_callees.return_value = []
    storage.get_type_refs.return_value = []
    storage.vector_search.return_value = []
    storage.traverse.return_value = []
    storage.traverse_with_depth.return_value = []
    storage.get_callers_with_confidence.return_value = []
    storage.get_callees_with_confidence.return_value = []
    storage.get_process_memberships.return_value = {}
    storage.execute_raw.return_value = []
    return storage


@pytest.fixture
def mock_storage_with_relations(mock_storage):
    """Storage mock with callers, callees, and type refs populated."""
    _caller = GraphNode(
        id="function:src/routes/auth.py:login_handler",
        label=NodeLabel.FUNCTION,
        name="login_handler",
        file_path="src/routes/auth.py",
        start_line=12,
        end_line=40,
    )
    _callee = GraphNode(
        id="function:src/auth/crypto.py:hash_password",
        label=NodeLabel.FUNCTION,
        name="hash_password",
        file_path="src/auth/crypto.py",
        start_line=5,
        end_line=20,
    )
    mock_storage.get_callers.return_value = [_caller]
    mock_storage.get_callees.return_value = [_callee]
    mock_storage.get_callers_with_confidence.return_value = [(_caller, 1.0)]
    mock_storage.get_callees_with_confidence.return_value = [(_callee, 0.8)]
    mock_storage.get_type_refs.return_value = [
        GraphNode(
            id="class:src/models.py:User",
            label=NodeLabel.CLASS,
            name="User",
            file_path="src/models.py",
            start_line=1,
            end_line=50,
        ),
    ]
    return mock_storage


# ---------------------------------------------------------------------------
# 1. axon_list_repos
# ---------------------------------------------------------------------------


class TestHandleListRepos:
    def test_no_registry_dir(self, tmp_path):
        """Returns 'no repos' message when registry directory does not exist."""
        result = handle_list_repos(registry_dir=tmp_path / "nonexistent")
        assert "No indexed repositories found" in result

    def test_empty_registry_dir(self, tmp_path):
        """Returns 'no repos' message when registry directory is empty."""
        registry = tmp_path / "repos"
        registry.mkdir()
        result = handle_list_repos(registry_dir=registry)
        assert "No indexed repositories found" in result

    def test_with_repos(self, tmp_path):
        """Returns formatted repo list when meta.json files are present."""
        registry = tmp_path / "repos"
        repo_dir = registry / "my-project"
        repo_dir.mkdir(parents=True)
        meta = {
            "name": "my-project",
            "path": "/home/user/my-project",
            "stats": {
                "files": 25,
                "symbols": 150,
                "relationships": 200,
            },
        }
        (repo_dir / "meta.json").write_text(json.dumps(meta))

        result = handle_list_repos(registry_dir=registry)
        assert "my-project" in result
        assert "150" in result
        assert "200" in result
        assert "Indexed repositories (1)" in result


# ---------------------------------------------------------------------------
# 2. axon_query
# ---------------------------------------------------------------------------


class TestHandleQuery:
    def test_returns_results(self, mock_storage):
        """Successful query returns formatted results."""
        result = handle_query(mock_storage, "validate")
        assert "validate" in result
        assert "Function" in result
        assert "src/auth.py" in result
        assert "Next:" in result

    def test_no_results(self, mock_storage):
        """Empty search returns no-results message."""
        mock_storage.fts_search.return_value = []
        mock_storage.vector_search.return_value = []
        result = handle_query(mock_storage, "nonexistent")
        assert "No results found" in result

    def test_snippet_included(self, mock_storage):
        """Search results include snippet text."""
        result = handle_query(mock_storage, "validate")
        assert "def validate" in result

    def test_custom_limit(self, mock_storage):
        """Limit parameter is passed through to hybrid_search."""
        handle_query(mock_storage, "validate", limit=5)
        # hybrid_search calls fts_search with candidate_limit = limit * 3
        mock_storage.fts_search.assert_called_once_with("validate", limit=15)


# ---------------------------------------------------------------------------
# 3. axon_context
# ---------------------------------------------------------------------------


class TestHandleContext:
    def test_basic_context(self, mock_storage):
        """Returns symbol name, file, and line range."""
        result = handle_context(mock_storage, "validate")
        assert "Symbol: validate (Function)" in result
        assert "src/auth.py:10-30" in result
        assert "Next:" in result

    def test_not_found_fts_empty(self, mock_storage):
        """Returns not-found message when FTS returns nothing."""
        mock_storage.exact_name_search.return_value = []
        mock_storage.fts_search.return_value = []
        result = handle_context(mock_storage, "nonexistent")
        assert "not found" in result.lower()

    def test_not_found_node_none(self, mock_storage):
        """Returns not-found message when get_node returns None."""
        mock_storage.get_node.return_value = None
        result = handle_context(mock_storage, "validate")
        assert "not found" in result.lower()

    def test_with_callers_callees_type_refs(self, mock_storage_with_relations):
        """Full context includes callers, callees, and type refs."""
        result = handle_context(mock_storage_with_relations, "validate")
        assert "Callers (1):" in result
        assert "login_handler" in result
        assert "Callees (1):" in result
        assert "hash_password" in result
        assert "Type references (1):" in result
        assert "User" in result

    def test_dead_code_flag(self, mock_storage):
        """Dead code status is shown when is_dead is True."""
        mock_storage.get_node.return_value = GraphNode(
            id="function:src/old.py:deprecated",
            label=NodeLabel.FUNCTION,
            name="deprecated",
            file_path="src/old.py",
            start_line=1,
            end_line=5,
            is_dead=True,
        )
        result = handle_context(mock_storage, "deprecated")
        assert "DEAD CODE" in result


# ---------------------------------------------------------------------------
# 4. axon_impact
# ---------------------------------------------------------------------------


class TestHandleImpact:
    def test_no_downstream(self, mock_storage):
        """Returns no-dependencies message when traverse is empty."""
        result = handle_impact(mock_storage, "validate")
        assert "No upstream callers found" in result or "No downstream dependencies" in result

    def test_with_affected_symbols(self, mock_storage):
        """Returns formatted impact list when traverse finds nodes."""
        _login = GraphNode(
            id="function:src/api.py:login",
            label=NodeLabel.FUNCTION,
            name="login",
            file_path="src/api.py",
            start_line=5,
            end_line=20,
        )
        _register = GraphNode(
            id="function:src/api.py:register",
            label=NodeLabel.FUNCTION,
            name="register",
            file_path="src/api.py",
            start_line=25,
            end_line=50,
        )
        mock_storage.traverse.return_value = [_login, _register]
        mock_storage.traverse_with_depth.return_value = [(_login, 1), (_register, 2)]
        mock_storage.get_callers_with_confidence.return_value = [(_login, 1.0)]
        result = handle_impact(mock_storage, "validate", depth=2)
        assert "Impact analysis for: validate" in result
        assert "Total: 2 symbols" in result
        assert "login" in result
        assert "register" in result
        assert "Depth: 2" in result

    def test_symbol_not_found(self, mock_storage):
        """Returns not-found when symbol does not exist."""
        mock_storage.exact_name_search.return_value = []
        mock_storage.fts_search.return_value = []
        result = handle_impact(mock_storage, "nonexistent")
        assert "not found" in result.lower()


# ---------------------------------------------------------------------------
# 5. axon_dead_code
# ---------------------------------------------------------------------------


class TestHandleDeadCode:
    def test_no_dead_code(self, mock_storage):
        """Returns clean message when no dead code found."""
        result = handle_dead_code(mock_storage)
        assert "No dead code detected" in result

    def test_with_dead_code(self, mock_storage):
        """Returns formatted dead code list (delegates to get_dead_code_list)."""
        mock_storage.execute_raw.return_value = [
            ["unused_func", "src/old.py", 10],
            ["DeprecatedModel", "src/models.py", 5],
        ]
        result = handle_dead_code(mock_storage)
        assert "Dead Code Report (2 symbols)" in result
        assert "unused_func" in result
        assert "DeprecatedModel" in result

    def test_execute_raw_exception(self, mock_storage):
        """Gracefully handles storage errors."""
        mock_storage.execute_raw.side_effect = RuntimeError("DB error")
        result = handle_dead_code(mock_storage)
        assert "Could not retrieve dead code list" in result


# ---------------------------------------------------------------------------
# 6. axon_detect_changes
# ---------------------------------------------------------------------------


SAMPLE_DIFF = """\
diff --git a/src/auth.py b/src/auth.py
index abc1234..def5678 100644
--- a/src/auth.py
+++ b/src/auth.py
@@ -10,5 +10,7 @@ def validate(user):
     if not user:
         return False
+    # Added new validation
+    check_permissions(user)
     return True
"""


class TestHandleDetectChanges:
    def test_parses_diff(self, mock_storage):
        """Successfully parses diff and identifies changed files."""
        # handle_detect_changes now uses execute_raw() with a Cypher query
        # to find symbols in the changed file.
        mock_storage.execute_raw.return_value = [
            ["function:src/auth.py:validate", "validate", "src/auth.py", 10, 30],
        ]

        result = handle_detect_changes(mock_storage, SAMPLE_DIFF)
        assert "src/auth.py" in result
        assert "validate" in result
        assert "Total affected symbols:" in result

    def test_empty_diff(self, mock_storage):
        """Returns message for empty diff input."""
        result = handle_detect_changes(mock_storage, "")
        assert "Empty diff provided" in result

    def test_unparseable_diff(self, mock_storage):
        """Returns message when diff contains no recognisable hunks."""
        result = handle_detect_changes(mock_storage, "just some random text")
        assert "Could not parse" in result

    def test_no_symbols_in_changed_lines(self, mock_storage):
        """Reports file but no symbols when nothing overlaps."""
        mock_storage.execute_raw.return_value = []
        result = handle_detect_changes(mock_storage, SAMPLE_DIFF)
        assert "src/auth.py" in result
        assert "no indexed symbols" in result


# ---------------------------------------------------------------------------
# 7. axon_cypher
# ---------------------------------------------------------------------------


class TestHandleCypher:
    def test_returns_results(self, mock_storage):
        """Formats raw query results."""
        mock_storage.execute_raw.return_value = [
            ["validate", "src/auth.py", 10],
            ["login", "src/api.py", 5],
        ]
        result = handle_cypher(mock_storage, "MATCH (n) RETURN n.name, n.file_path, n.start_line")
        assert "Results (2 rows)" in result
        assert "validate" in result
        assert "src/api.py" in result

    def test_no_results(self, mock_storage):
        """Returns no-results message for empty query output."""
        result = handle_cypher(mock_storage, "MATCH (n:Nonexistent) RETURN n")
        assert "no results" in result.lower()

    def test_query_error(self, mock_storage):
        """Returns error message when query execution fails."""
        mock_storage.execute_raw.side_effect = RuntimeError("Syntax error")
        result = handle_cypher(mock_storage, "INVALID QUERY")
        assert "failed" in result.lower()
        assert "Syntax error" in result


# ---------------------------------------------------------------------------
# Resource handlers
# ---------------------------------------------------------------------------


class TestResources:
    def test_get_schema(self):
        """Schema resource returns static schema text."""
        from axon.mcp.resources import get_schema

        result = get_schema()
        assert "Node Labels:" in result
        assert "Relationship Types:" in result
        assert "CALLS" in result
        assert "Function" in result

    def test_get_overview(self, mock_storage):
        """Overview resource queries storage for stats."""
        from axon.mcp.resources import get_overview

        mock_storage.execute_raw.return_value = [["Function", 42]]
        result = get_overview(mock_storage)
        assert "Axon Codebase Overview" in result

    def test_get_dead_code_list(self, mock_storage):
        """Dead code resource returns formatted report."""
        from axon.mcp.resources import get_dead_code_list

        mock_storage.execute_raw.return_value = [
            ["old_func", "src/old.py", 10],
        ]
        result = get_dead_code_list(mock_storage)
        assert "Dead Code Report" in result
        assert "old_func" in result

    def test_get_dead_code_list_empty(self, mock_storage):
        """Dead code resource returns clean message when empty."""
        from axon.mcp.resources import get_dead_code_list

        result = get_dead_code_list(mock_storage)
        assert "No dead code detected" in result


# ---------------------------------------------------------------------------
# Confidence tags
# ---------------------------------------------------------------------------


class TestConfidenceTag:
    """_confidence_tag() returns the correct visual indicator."""

    def test_high_confidence(self):
        assert _confidence_tag(1.0) == ""
        assert _confidence_tag(0.95) == ""
        assert _confidence_tag(0.9) == ""

    def test_medium_confidence(self):
        assert _confidence_tag(0.89) == " (~)"
        assert _confidence_tag(0.5) == " (~)"
        assert _confidence_tag(0.7) == " (~)"

    def test_low_confidence(self):
        assert _confidence_tag(0.49) == " (?)"
        assert _confidence_tag(0.1) == " (?)"
        assert _confidence_tag(0.0) == " (?)"


class TestConfidenceInContext:
    """handle_context() displays confidence tags in output."""

    def test_medium_confidence_tag_shown(self, mock_storage_with_relations):
        """Callees with confidence 0.8 show the (~) tag."""
        result = handle_context(mock_storage_with_relations, "validate")
        # _callee has confidence 0.8, which produces " (~)"
        assert "(~)" in result

    def test_high_confidence_no_tag(self, mock_storage_with_relations):
        """Callers with confidence 1.0 show no extra tag."""
        result = handle_context(mock_storage_with_relations, "validate")
        # login_handler has confidence 1.0 — no tag after its line
        assert "login_handler" in result
        # There should be no "(?)" for the high-confidence caller
        lines = result.split("\n")
        caller_line = [l for l in lines if "login_handler" in l][0]
        assert "(?)" not in caller_line
        assert "(~)" not in caller_line


# ---------------------------------------------------------------------------
# Process-grouped search
# ---------------------------------------------------------------------------


class TestGroupByProcess:
    """_group_by_process() groups search results by process membership."""

    def test_empty_results(self, mock_storage):
        """Returns empty dict for empty results list."""
        groups = _group_by_process([], mock_storage)
        assert groups == {}

    def test_with_memberships(self, mock_storage):
        """Returns correct grouping when process memberships exist."""
        results = [
            SearchResult(node_id="func:a", score=1.0, node_name="a"),
            SearchResult(node_id="func:b", score=0.9, node_name="b"),
            SearchResult(node_id="func:c", score=0.8, node_name="c"),
        ]
        mock_storage.get_process_memberships.return_value = {
            "func:a": "Auth Flow",
            "func:c": "Auth Flow",
        }
        groups = _group_by_process(results, mock_storage)
        assert "Auth Flow" in groups
        assert len(groups["Auth Flow"]) == 2

    def test_backend_missing_method(self, mock_storage):
        """Returns empty dict if backend raises AttributeError."""
        mock_storage.get_process_memberships.side_effect = AttributeError
        results = [SearchResult(node_id="func:a", score=1.0)]
        groups = _group_by_process(results, mock_storage)
        assert groups == {}


class TestFormatQueryResults:
    """_format_query_results() renders grouped and ungrouped results."""

    def test_ungrouped_only(self):
        """With no groups, results appear inline."""
        results = [
            SearchResult(
                node_id="func:a", score=1.0, node_name="foo",
                file_path="src/a.py", label="function",
            ),
        ]
        output = _format_query_results(results, {})
        assert "foo (Function)" in output
        assert "src/a.py" in output
        assert "Next:" in output

    def test_with_groups(self):
        """Grouped results appear under process section headers."""
        r1 = SearchResult(
            node_id="func:a", score=1.0, node_name="login",
            file_path="src/auth.py", label="function",
        )
        r2 = SearchResult(
            node_id="func:b", score=0.9, node_name="helper",
            file_path="src/utils.py", label="function",
        )
        groups = {"Auth Flow": [r1]}
        output = _format_query_results([r1, r2], groups)
        assert "=== Auth Flow ===" in output
        assert "=== Other results ===" in output
        assert "login" in output
        assert "helper" in output

    def test_snippet_truncation(self):
        """Snippets longer than 200 chars are truncated."""
        long_snippet = "x" * 300
        results = [
            SearchResult(
                node_id="func:a", score=1.0, node_name="foo",
                file_path="src/a.py", label="function", snippet=long_snippet,
            ),
        ]
        output = _format_query_results(results, {})
        # Snippet in output should be at most 200 chars
        lines = output.split("\n")
        snippet_lines = [l for l in lines if l.strip().startswith("xxx")]
        for line in snippet_lines:
            assert len(line.strip()) <= 200


# ---------------------------------------------------------------------------
# Impact depth grouping
# ---------------------------------------------------------------------------


class TestImpactDepthGrouping:
    """handle_impact() groups results by depth with labels."""

    def test_depth_section_headers(self, mock_storage):
        """Output contains depth section headers with labels."""
        _login = GraphNode(
            id="function:src/api.py:login",
            label=NodeLabel.FUNCTION,
            name="login",
            file_path="src/api.py",
            start_line=5,
            end_line=20,
        )
        _register = GraphNode(
            id="function:src/api.py:register",
            label=NodeLabel.FUNCTION,
            name="register",
            file_path="src/api.py",
            start_line=25,
            end_line=50,
        )
        mock_storage.traverse_with_depth.return_value = [
            (_login, 1), (_register, 2),
        ]
        mock_storage.get_callers_with_confidence.return_value = [(_login, 0.8)]

        result = handle_impact(mock_storage, "validate", depth=2)
        assert "Depth 1" in result
        assert "Direct callers (will break)" in result
        assert "Depth 2" in result
        assert "Indirect (may break)" in result

    def test_depth_3_transitive_label(self, mock_storage):
        """Depth >= 3 shows 'Transitive (review)' label."""
        _node = GraphNode(
            id="function:src/far.py:distant",
            label=NodeLabel.FUNCTION,
            name="distant",
            file_path="src/far.py",
            start_line=1,
            end_line=10,
        )
        mock_storage.traverse_with_depth.return_value = [(_node, 3)]
        mock_storage.get_callers_with_confidence.return_value = []

        result = handle_impact(mock_storage, "validate", depth=3)
        assert "Transitive (review)" in result

    def test_confidence_shown_for_direct_callers(self, mock_storage):
        """Direct callers show inline confidence score."""
        _login = GraphNode(
            id="function:src/api.py:login",
            label=NodeLabel.FUNCTION,
            name="login",
            file_path="src/api.py",
            start_line=5,
            end_line=20,
        )
        mock_storage.traverse_with_depth.return_value = [(_login, 1)]
        mock_storage.get_callers_with_confidence.return_value = [(_login, 0.75)]

        result = handle_impact(mock_storage, "validate", depth=1)
        assert "confidence: 0.75" in result

    def test_depth_clamped_to_max(self, mock_storage):
        """Depth > MAX_TRAVERSE_DEPTH is clamped (no crash)."""
        mock_storage.traverse_with_depth.return_value = []
        result = handle_impact(mock_storage, "validate", depth=100)
        assert "No upstream callers found" in result


# ---------------------------------------------------------------------------
# Language filter (axon_query)
# ---------------------------------------------------------------------------


class TestHandleQueryLanguageFilter:
    """handle_query() filters results by language when requested."""

    def _make_result(self, node_id, name, file_path, language):
        return SearchResult(
            node_id=node_id,
            score=1.0,
            node_name=name,
            file_path=file_path,
            label="function",
            snippet="",
            language=language,
        )

    def test_language_filter_returns_only_matching(self, mock_storage):
        """Results are filtered to the requested language."""
        py_result = self._make_result("func:a", "parse_py", "src/a.py", "python")
        ex_result = self._make_result("func:b", "parse_ex", "src/b.ex", "elixir")
        mock_storage.fts_search.return_value = [py_result, ex_result]
        mock_storage.vector_search.return_value = []
        mock_storage.get_process_memberships.return_value = {}

        result = handle_query(mock_storage, "parse", language="python")

        assert "parse_py" in result
        assert "parse_ex" not in result

    def test_language_filter_no_match_returns_message(self, mock_storage):
        """Empty filtered list returns a message naming the language."""
        ex_result = self._make_result("func:b", "parse_ex", "src/b.ex", "elixir")
        mock_storage.fts_search.return_value = [ex_result]
        mock_storage.vector_search.return_value = []

        result = handle_query(mock_storage, "parse", language="rust")

        assert "rust" in result
        assert "No results found" in result

    def test_language_filter_none_returns_all(self, mock_storage):
        """When language=None all results are returned unchanged."""
        py_result = self._make_result("func:a", "parse_py", "src/a.py", "python")
        ex_result = self._make_result("func:b", "parse_ex", "src/b.ex", "elixir")
        mock_storage.fts_search.return_value = [py_result, ex_result]
        mock_storage.vector_search.return_value = []
        mock_storage.get_process_memberships.return_value = {}

        result = handle_query(mock_storage, "parse", language=None)

        assert "parse_py" in result
        assert "parse_ex" in result


# ---------------------------------------------------------------------------
# File-path disambiguation (axon_context)
# ---------------------------------------------------------------------------


class TestHandleContextDisambiguation:
    """handle_context() disambiguation and file:symbol lookup."""

    def _make_search_result(self, node_id, name, file_path):
        return SearchResult(
            node_id=node_id,
            score=1.0,
            node_name=name,
            file_path=file_path,
            label="function",
            snippet="",
        )

    def test_single_result_no_disambiguation(self, mock_storage):
        """Single match proceeds normally without disambiguation message."""
        mock_storage.exact_name_search.return_value = []
        mock_storage.fts_search.return_value = [
            self._make_search_result("func:a", "parse", "src/parser.py"),
        ]
        result = handle_context(mock_storage, "parse")
        assert "Multiple symbols" not in result
        assert "Retry with" not in result

    def test_multiple_files_returns_disambiguation(self, mock_storage):
        """Multiple distinct file paths trigger disambiguation list."""
        # exact_name_search must return [] so _resolve_symbol falls through to fts_search
        mock_storage.exact_name_search.return_value = []
        mock_storage.fts_search.return_value = [
            self._make_search_result("func:a", "parse", "src/parser.py"),
            self._make_search_result("func:b", "parse", "src/legacy/parser.py"),
            self._make_search_result("func:c", "parse", "tests/test_parser.py"),
        ]
        result = handle_context(mock_storage, "parse")
        assert "Multiple symbols" in result
        assert "src/parser.py" in result
        assert "src/legacy/parser.py" in result
        assert "Retry with" in result

    def test_file_colon_symbol_exact_match(self, mock_storage):
        """'file.py:symbol' format resolves to the matching file."""
        mock_storage.fts_search.return_value = [
            self._make_search_result("func:a", "parse", "src/parser.py"),
            self._make_search_result("func:b", "parse", "src/legacy/parser.py"),
        ]
        mock_storage.get_node.return_value = GraphNode(
            id="func:a",
            label=NodeLabel.FUNCTION,
            name="parse",
            file_path="src/parser.py",
            start_line=1,
            end_line=10,
        )
        result = handle_context(mock_storage, "src/parser.py:parse")
        assert "Multiple symbols" not in result
        # Should return the node context, not an error
        assert "parse" in result
        assert "src/parser.py" in result

    def test_file_colon_symbol_not_found(self, mock_storage):
        """Returns not-found message when file hint matches no results."""
        mock_storage.fts_search.return_value = [
            self._make_search_result("func:a", "parse", "src/parser.py"),
        ]
        result = handle_context(mock_storage, "src/nonexistent.py:parse")
        assert "not found" in result
        assert "nonexistent.py" in result


# ---------------------------------------------------------------------------
# Multi-repo routing
# ---------------------------------------------------------------------------


class TestMultiRepoRouting:
    """Tests for _load_repo_storage and repo= parameter on query/context/impact."""

    def test_load_repo_storage_missing_meta(self, tmp_path: Path, monkeypatch):
        """Returns None when meta.json does not exist for the named repo."""
        monkeypatch.setattr(Path, "home", lambda: tmp_path)
        result = _load_repo_storage("nonexistent-repo")
        assert result is None

    def test_load_repo_storage_corrupt_json(self, tmp_path: Path, monkeypatch):
        """Returns None when meta.json contains invalid JSON."""
        monkeypatch.setattr(Path, "home", lambda: tmp_path)
        repo_dir = tmp_path / ".axon" / "repos" / "bad-repo"
        repo_dir.mkdir(parents=True)
        (repo_dir / "meta.json").write_text("NOT JSON", encoding="utf-8")
        result = _load_repo_storage("bad-repo")
        assert result is None

    def test_load_repo_storage_db_not_found(self, tmp_path: Path, monkeypatch):
        """Returns None when meta.json is valid but the kuzu db path does not exist."""
        monkeypatch.setattr(Path, "home", lambda: tmp_path)
        repo_dir = tmp_path / ".axon" / "repos" / "valid-meta"
        repo_dir.mkdir(parents=True)
        meta = {"name": "valid-meta", "path": str(tmp_path / "some-repo")}
        (repo_dir / "meta.json").write_text(json.dumps(meta), encoding="utf-8")
        # No kuzu db at that path — initialize should fail → returns None
        result = _load_repo_storage("valid-meta")
        assert result is None

    def test_handle_query_repo_not_found(self, mock_storage):
        """handle_query returns registry error when named repo does not exist."""
        # Use a patched home so the registry is empty
        from unittest.mock import patch
        from pathlib import Path as _Path

        with patch.object(_Path, "home", return_value=_Path("/nonexistent_registry_xyz")):
            result = handle_query(mock_storage, "test query", repo="missing-repo")
        assert "missing-repo" in result
        assert "not found" in result.lower()

    def test_handle_context_repo_not_found(self, mock_storage):
        """handle_context returns registry error when named repo does not exist."""
        from unittest.mock import patch
        from pathlib import Path as _Path

        with patch.object(_Path, "home", return_value=_Path("/nonexistent_registry_xyz")):
            result = handle_context(mock_storage, "some_symbol", repo="missing-repo")
        assert "missing-repo" in result
        assert "not found" in result.lower()

    def test_handle_impact_repo_not_found(self, mock_storage):
        """handle_impact returns registry error when named repo does not exist."""
        from unittest.mock import patch
        from pathlib import Path as _Path

        with patch.object(_Path, "home", return_value=_Path("/nonexistent_registry_xyz")):
            result = handle_impact(mock_storage, "some_symbol", repo="missing-repo")
        assert "missing-repo" in result
        assert "not found" in result.lower()

    def test_handle_query_no_repo_uses_default_storage(self, mock_storage):
        """handle_query without repo= uses the passed storage backend."""
        mock_storage.fts_search.return_value = []
        result = handle_query(mock_storage, "test", repo=None)
        # Default storage was queried (fts_search called via hybrid_search path)
        assert "No results" in result

    def test_handle_query_repo_none_does_not_open_registry(self, mock_storage, monkeypatch):
        """When repo=None, _load_repo_storage is never called."""
        called = []
        original = _load_repo_storage

        def spy(repo):
            called.append(repo)
            return original(repo)

        monkeypatch.setattr("axon.mcp.tools._load_repo_storage", spy)
        mock_storage.fts_search.return_value = []
        handle_query(mock_storage, "hello", repo=None)
        assert called == []

    def test_load_repo_storage_uses_central_path_when_exists(
        self, tmp_path: Path, monkeypatch
    ):
        """Uses ~/.axon/repos/{repo}/kuzu when central kuzu file exists."""
        from unittest.mock import patch, MagicMock

        monkeypatch.setattr(Path, "home", lambda: tmp_path)
        repo_dir = tmp_path / ".axon" / "repos" / "myapp"
        repo_dir.mkdir(parents=True)
        meta = {"name": "myapp", "path": str(tmp_path / "some-repo"), "slug": "myapp"}
        (repo_dir / "meta.json").write_text(json.dumps(meta), encoding="utf-8")
        # Simulate central kuzu file existing
        (repo_dir / "kuzu").touch()

        mock_backend = MagicMock()
        with patch("axon.core.storage.kuzu_backend.KuzuBackend", return_value=mock_backend):
            result = _load_repo_storage("myapp")

        mock_backend.initialize.assert_called_once_with(repo_dir / "kuzu", read_only=True)
        assert result is mock_backend

    def test_load_repo_storage_legacy_fallback_when_no_central(
        self, tmp_path: Path, monkeypatch
    ):
        """Falls back to {meta[path]}/.axon/kuzu when central kuzu doesn't exist."""
        from unittest.mock import patch, MagicMock

        monkeypatch.setattr(Path, "home", lambda: tmp_path)
        repo_dir = tmp_path / ".axon" / "repos" / "myapp"
        repo_dir.mkdir(parents=True)
        legacy_kuzu = tmp_path / "some-repo" / ".axon" / "kuzu"
        legacy_kuzu.parent.mkdir(parents=True, exist_ok=True)
        legacy_kuzu.touch()
        meta = {"name": "myapp", "path": str(tmp_path / "some-repo"), "slug": "myapp"}
        (repo_dir / "meta.json").write_text(json.dumps(meta), encoding="utf-8")
        # No central kuzu file

        mock_backend = MagicMock()
        with patch("axon.core.storage.kuzu_backend.KuzuBackend", return_value=mock_backend):
            result = _load_repo_storage("myapp")

        mock_backend.initialize.assert_called_once_with(legacy_kuzu, read_only=True)
        assert result is mock_backend


# ---------------------------------------------------------------------------
# Security: path traversal, Cypher injection, WRITE_KEYWORDS, callers cap
# ---------------------------------------------------------------------------


class TestSanitizeRepoSlug:
    """_sanitize_repo_slug() rejects unsafe slugs and accepts valid ones."""

    @pytest.mark.parametrize("repo", [
        "../../.ssh/id_rsa",
        "../evil",
        "/absolute/path",
        "repo with spaces",
        "a" * 201,
        "repo\x00null",
    ])
    def test_rejects_traversal(self, repo):
        assert _sanitize_repo_slug(repo) is None

    def test_accepts_valid_slug(self):
        assert _sanitize_repo_slug("my-repo_v2.0") == "my-repo_v2.0"

    def test_load_repo_storage_rejects_traversal(self, tmp_path, monkeypatch):
        """_load_repo_storage returns None for a traversal slug without touching fs."""
        monkeypatch.setattr(Path, "home", lambda: tmp_path)
        result = _load_repo_storage("../../etc/passwd")
        assert result is None


class TestDetectChangesSecurity:
    """handle_detect_changes() uses a single batched parameterised query."""

    def test_single_query_for_multi_file_diff(self, mock_storage):
        """Two files in diff must result in exactly ONE execute_raw call."""
        mock_storage.execute_raw.return_value = []
        diff = (
            "diff --git a/a.py b/a.py\n@@ -1,1 +1,1 @@\n-x\n+y\n"
            "diff --git a/b.py b/b.py\n@@ -1,1 +1,1 @@\n-x\n+y\n"
        )
        handle_detect_changes(mock_storage, diff)
        assert mock_storage.execute_raw.call_count == 1

    def test_uses_named_parameters_not_fstring(self, mock_storage):
        """Query must use named parameters (no f-string Cypher injection)."""
        mock_storage.execute_raw.return_value = []
        diff = "diff --git a/evil'; MATCH (n) RETURN n b/evil\n@@ -1 +1 @@\n-x\n+y\n"
        handle_detect_changes(mock_storage, diff)
        call = mock_storage.execute_raw.call_args
        query_str = call.args[0] if call.args else ""
        # Must not have interpolated quotes in the query string
        assert "evil'" not in query_str
        assert "$fps" in query_str


class TestWriteKeywords:
    """handle_cypher() rejects RENAME, ALTER, IMPORT (new additions)."""

    def test_rejects_rename(self, mock_storage):
        result = handle_cypher(mock_storage, "RENAME NODE foo TO bar")
        assert "rejected" in result.lower()

    def test_rejects_alter(self, mock_storage):
        result = handle_cypher(mock_storage, "ALTER TABLE foo ADD col INT")
        assert "rejected" in result.lower()

    def test_rejects_import(self, mock_storage):
        result = handle_cypher(mock_storage, "IMPORT DATABASE foo")
        assert "rejected" in result.lower()


class TestCallersCap:
    """handle_context() caps callers and callees at 20."""

    def _make_node(self, name: str) -> "GraphNode":
        return GraphNode(
            id=f"function:src/f.py:{name}",
            label=NodeLabel.FUNCTION,
            name=name,
            file_path="src/f.py",
            start_line=1,
            end_line=5,
        )

    def test_caps_callers_at_20(self, mock_storage):
        """100 callers → only 20 shown, '... and 80 more' appended."""
        callers = [(self._make_node(f"caller_{i}"), 1.0) for i in range(100)]
        mock_storage.get_callers_with_confidence.return_value = callers
        result = handle_context(mock_storage, "validate")
        caller_lines = [ln for ln in result.split("\n") if "-> caller_" in ln]
        assert len(caller_lines) <= 20
        assert "and 80 more" in result

    def test_no_truncation_under_20(self, mock_storage):
        """15 callers → all 15 shown, no 'more' line."""
        callers = [(self._make_node(f"c_{i}"), 1.0) for i in range(15)]
        mock_storage.get_callers_with_confidence.return_value = callers
        result = handle_context(mock_storage, "validate")
        caller_lines = [ln for ln in result.split("\n") if "-> c_" in ln]
        assert len(caller_lines) == 15
        assert "more" not in result


# ---------------------------------------------------------------------------
# handle_read_symbol tests
# ---------------------------------------------------------------------------


class TestHandleReadSymbol:
    def test_not_found(self, mock_storage: MagicMock) -> None:
        mock_storage.execute_raw.return_value = []
        result = handle_read_symbol(mock_storage, symbol="NonExistent")
        assert "Symbol not found: NonExistent" in result

    def test_fallback_to_stored_content(self, mock_storage: MagicMock) -> None:
        """When start_byte=0 and end_byte=0, returns stored content with note."""
        mock_storage.execute_raw.return_value = [
            ["foo", "src/foo.py", 1, 0, 0, "def foo(): pass"]
        ]
        result = handle_read_symbol(mock_storage, symbol="foo")
        assert "byte offsets unavailable" in result
        assert "def foo(): pass" in result


# ---------------------------------------------------------------------------
# axon_find_similar tests
# ---------------------------------------------------------------------------


# ---------------------------------------------------------------------------
# Attribute surfacing in axon_context (AC-3)
# ---------------------------------------------------------------------------


class TestContextAttributeSurfacing:
    def test_shows_tested_exported_centrality(self, mock_storage: MagicMock) -> None:
        """axon_context shows tested/exported/centrality attributes."""
        mock_storage.get_node.return_value = GraphNode(
            id="function:src/auth.py:validate",
            label=NodeLabel.FUNCTION,
            name="validate",
            file_path="src/auth.py",
            start_line=10,
            end_line=30,
            tested=True,
            is_exported=True,
            centrality=0.15,
        )
        result = handle_context(mock_storage, "validate")

        assert "tested=yes" in result
        assert "exported=yes" in result
        assert "0.150" in result

    def test_shows_no_centrality_when_zero(self, mock_storage: MagicMock) -> None:
        """centrality is omitted from Attributes line when 0.0."""
        mock_storage.get_node.return_value = GraphNode(
            id="function:src/auth.py:validate",
            label=NodeLabel.FUNCTION,
            name="validate",
            file_path="src/auth.py",
            start_line=10,
            end_line=30,
            tested=False,
            is_exported=False,
            centrality=0.0,
        )
        result = handle_context(mock_storage, "validate")

        assert "tested=no" in result
        assert "exported=no" in result
        assert "centrality=" not in result


class TestHandleFindSimilar:
    def test_returns_similar_symbols(self, mock_storage: MagicMock) -> None:
        """Returns formatted list excluding the queried symbol itself."""
        mock_storage.exact_name_search.return_value = []
        mock_storage.fts_search.return_value = [
            SearchResult(
                node_id="function:src/parser.py:parse_file",
                score=1.0,
                node_name="parse_file",
                file_path="src/parser.py",
                label="function",
            )
        ]
        mock_storage.get_embedding.return_value = [0.1, 0.2, 0.3]
        mock_storage.vector_search.return_value = [
            SearchResult(
                node_id="function:src/parser.py:parse_file",  # self — should be excluded
                score=1.0,
                node_name="parse_file",
                file_path="src/parser.py",
                label="function",
            ),
            SearchResult(
                node_id="function:src/lexer.py:tokenize",
                score=0.87,
                node_name="tokenize",
                file_path="src/lexer.py",
                label="function",
                snippet="def tokenize(src): ...",
            ),
        ]

        result = handle_find_similar(mock_storage, "parse_file")

        assert "Similar to:" in result
        assert "parse_file" in result
        # queried symbol excluded from results list
        assert "tokenize" in result
        assert "87%" in result

    def test_no_embedding_returns_error(self, mock_storage: MagicMock) -> None:
        """Returns descriptive error when no embedding exists."""
        mock_storage.exact_name_search.return_value = []
        mock_storage.fts_search.return_value = [
            SearchResult(
                node_id="function:src/auth.py:validate",
                score=1.0,
                node_name="validate",
                file_path="src/auth.py",
                label="function",
            )
        ]
        mock_storage.get_embedding.return_value = None

        result = handle_find_similar(mock_storage, "validate")

        assert "No embedding found" in result

    def test_symbol_not_found(self, mock_storage: MagicMock) -> None:
        """Returns not-found message when symbol lookup fails."""
        mock_storage.exact_name_search.return_value = []
        mock_storage.fts_search.return_value = []

        result = handle_find_similar(mock_storage, "ghost_func")

        assert "not found" in result.lower()

    def test_self_excluded_from_results(self, mock_storage: MagicMock) -> None:
        """The queried symbol does not appear in the results list."""
        mock_storage.exact_name_search.return_value = []
        node_id = "function:src/app.py:my_func"
        mock_storage.fts_search.return_value = [
            SearchResult(
                node_id=node_id,
                score=1.0,
                node_name="my_func",
                file_path="src/app.py",
                label="function",
            )
        ]
        mock_storage.get_embedding.return_value = [0.5, 0.5]
        mock_storage.vector_search.return_value = [
            SearchResult(
                node_id=node_id,
                score=1.0,
                node_name="my_func",
                file_path="src/app.py",
                label="function",
            ),
        ]

        result = handle_find_similar(mock_storage, "my_func")

        # With only self returned and filtered out, no similar found
        assert "No similar symbols found" in result


# ---------------------------------------------------------------------------
# axon_find_usages tests
# ---------------------------------------------------------------------------


class TestHandleFindUsages:
    def test_find_usages_calls_only(self, mock_storage: MagicMock) -> None:
        """Returns CALLS section when symbol has callers but no importers."""
        mock_storage.exact_name_search.return_value = []
        mock_storage.fts_search.return_value = [
            SearchResult(
                node_id="function:src/parsers/python.py:parse_file",
                score=1.0,
                node_name="parse_file",
                file_path="src/parsers/python.py",
                label="function",
            )
        ]

        def execute_raw_side_effect(query, parameters=None):
            if parameters and "nid" in parameters:
                # CALLS query
                return [
                    ["pipeline", "src/pipeline.py", 42],
                    ["indexer", "src/indexer.py", 10],
                ]
            # IMPORTS query
            return []

        mock_storage.execute_raw.side_effect = execute_raw_side_effect

        result = handle_find_usages(mock_storage, "parse_file")

        assert "CALLS" in result
        assert "pipeline" in result
        assert "2 call sites" in result
        assert "No usages found" not in result

    def test_find_usages_imports_only(self, mock_storage: MagicMock) -> None:
        """Returns IMPORTS section when symbol has importers but no callers."""
        mock_storage.exact_name_search.return_value = []
        mock_storage.fts_search.return_value = [
            SearchResult(
                node_id="function:src/parsers/python.py:parse_file",
                score=1.0,
                node_name="parse_file",
                file_path="src/parsers/python.py",
                label="function",
            )
        ]

        def execute_raw_side_effect(query, parameters=None):
            if parameters and "nid" in parameters:
                return []
            # IMPORTS query
            return [
                ["src/cli.py", "src/cli.py", 1],
                ["src/main.py", "src/main.py", 1],
            ]

        mock_storage.execute_raw.side_effect = execute_raw_side_effect

        result = handle_find_usages(mock_storage, "parse_file")

        assert "IMPORTS" in result
        assert "src/cli.py" in result

    def test_find_usages_not_found(self, mock_storage: MagicMock) -> None:
        """Returns not-found message when symbol is not in the graph."""
        mock_storage.exact_name_search.return_value = []
        mock_storage.fts_search.return_value = []

        result = handle_find_usages(mock_storage, "ghost_func")

        assert "not found" in result.lower()

    def test_find_usages_no_usages(self, mock_storage: MagicMock) -> None:
        """Returns no-usages message when symbol exists but has no callers or importers."""
        mock_storage.exact_name_search.return_value = []
        mock_storage.fts_search.return_value = [
            SearchResult(
                node_id="function:src/utils.py:helper",
                score=1.0,
                node_name="helper",
                file_path="src/utils.py",
                label="function",
            )
        ]
        mock_storage.execute_raw.return_value = []

        result = handle_find_usages(mock_storage, "helper")

        assert "No usages found" in result


# ---------------------------------------------------------------------------
# axon_lint tests
# ---------------------------------------------------------------------------


class TestHandleLint:
    def test_lint_high_coupling(self, mock_storage: MagicMock) -> None:
        """Returns High Coupling section when fan-out exceeds threshold."""
        call_seq = [
            [["god_func", "src/god.py", 25]],  # fan-out query
            [],                                  # god class query
            [],                                  # import cycles query
        ]
        mock_storage.execute_raw.side_effect = call_seq

        result = handle_lint(mock_storage)

        assert "High Coupling" in result
        assert "god_func" in result

    def test_lint_god_class(self, mock_storage: MagicMock) -> None:
        """Returns God Classes section when class has too many methods."""
        call_seq = [
            [],                                        # fan-out query
            [["BigClass", "src/models.py", 18]],       # god class query
            [],                                        # import cycles query
        ]
        mock_storage.execute_raw.side_effect = call_seq

        result = handle_lint(mock_storage)

        assert "God" in result
        assert "BigClass" in result

    def test_lint_import_cycle(self, mock_storage: MagicMock) -> None:
        """Returns Import Cycles section when mutual imports detected."""
        call_seq = [
            [],                                    # fan-out query
            [],                                    # god class query
            [["src/a.py", "src/b.py"]],            # import cycles query
        ]
        mock_storage.execute_raw.side_effect = call_seq

        result = handle_lint(mock_storage)

        assert "Import Cycle" in result or "cycles" in result.lower()
        assert "src/a.py" in result

    def test_lint_clean(self, mock_storage: MagicMock) -> None:
        """Returns clean message when no structural issues found."""
        mock_storage.execute_raw.side_effect = [[], [], []]

        result = handle_lint(mock_storage)

        assert "No structural issues" in result


# ---------------------------------------------------------------------------
# axon_summarize tests
# ---------------------------------------------------------------------------


class TestHandleSummarize:
    def test_summarize_file_path(self, mock_storage: MagicMock) -> None:
        """File path input returns symbol inventory with Classes and Functions sections."""
        mock_storage.execute_raw.side_effect = [
            # File node lookup
            [["file:src/py.py", "python.py", "src/parsers/python.py", "python"]],
            # Children query
            [
                ["class:src/py.py:PyParser", "PyParser", "src/parsers/python.py", 10, "class PyParser:", True, True, 0.12],
                ["function:src/py.py:parse", "parse_file", "src/parsers/python.py", 50, "def parse_file(p):", False, True, 0.05],
            ],
        ]

        result = handle_summarize(mock_storage, "src/parsers/python.py")

        assert "src/parsers/python.py" in result
        assert "Classes" in result
        assert "Functions" in result
        assert "PyParser" in result
        assert "parse_file" in result

    def test_summarize_symbol_class(self, mock_storage: MagicMock) -> None:
        """Symbol name input returns class summary with callers count and tested flag."""
        mock_storage.execute_raw.side_effect = [
            [],  # file lookup — no match
            [["__init__", "def __init__"], ["parse", "def parse()"]],  # method children
        ]
        mock_storage.exact_name_search.return_value = []
        mock_storage.fts_search.return_value = [
            SearchResult(
                node_id="class:src/models.py:MyParser",
                score=1.0,
                node_name="MyParser",
                file_path="src/models.py",
                label="class",
            )
        ]
        mock_storage.get_node.return_value = GraphNode(
            id="class:src/models.py:MyParser",
            label=NodeLabel.CLASS,
            name="MyParser",
            file_path="src/models.py",
            start_line=10,
            tested=True,
            is_exported=True,
            centrality=0.12,
        )
        caller_node = GraphNode(
            id="function:src/cli.py:main",
            label=NodeLabel.FUNCTION,
            name="main",
            file_path="src/cli.py",
            start_line=1,
        )
        mock_storage.get_callers_with_confidence.return_value = [(caller_node, 1.0)] * 12
        mock_storage.get_callees_with_confidence.return_value = [(caller_node, 1.0)] * 5

        result = handle_summarize(mock_storage, "MyParser")

        assert "MyParser" in result
        assert "Class" in result
        assert "Callers: 12" in result
        assert "tested: yes" in result

    def test_summarize_not_found(self, mock_storage: MagicMock) -> None:
        """Returns not-found message when neither file nor symbol matches."""
        mock_storage.execute_raw.return_value = []
        mock_storage.exact_name_search.return_value = []
        mock_storage.fts_search.return_value = []

        result = handle_summarize(mock_storage, "NonExistent")

        assert "not found" in result.lower()

    def test_summarize_file_no_symbols(self, mock_storage: MagicMock) -> None:
        """File with no indexed symbols shows 0 symbols in result."""
        mock_storage.execute_raw.side_effect = [
            [["file:src/config.py", "config.py", "src/config.py", "python"]],  # file lookup
            [],  # children — empty
        ]

        result = handle_summarize(mock_storage, "src/config.py")

        assert "src/config.py" in result
        assert "0 symbols" in result


# ---------------------------------------------------------------------------
# axon_entry_points tests
# ---------------------------------------------------------------------------


class TestHandleEntryPoints:
    def test_returns_string(self, mock_storage: MagicMock) -> None:
        """handle_entry_points always returns a string."""
        mock_storage.execute_raw.return_value = []

        result = handle_entry_points(mock_storage)

        assert isinstance(result, str)

    def test_empty_results(self, mock_storage: MagicMock) -> None:
        """Returns a helpful message when no entry points are found."""
        mock_storage.execute_raw.return_value = []

        result = handle_entry_points(mock_storage)

        assert "No entry points found" in result or isinstance(result, str)

    def test_with_results(self, mock_storage: MagicMock) -> None:
        """Returns numbered list with file path when symbols are found."""
        mock_storage.execute_raw.side_effect = [
            [
                ["main", "src/cli.py", 10, "function", 0.42],
                ["handle_request", "src/server.py", 55, "function", 0.31],
            ],
        ]

        result = handle_entry_points(mock_storage)

        assert "main" in result
        assert "handle_request" in result
        assert "src/cli.py" in result

    def test_repo_not_found(self, mock_storage: MagicMock) -> None:
        """Returns not-found message when repo slug is unknown."""
        result = handle_entry_points(mock_storage, repo="nonexistent-repo-xyz")

        assert "not found" in result.lower() or "registry" in result.lower()

    def test_repo_none_uses_storage(self, mock_storage: MagicMock) -> None:
        """repo=None uses the passed storage backend directly."""
        mock_storage.execute_raw.return_value = []

        result = handle_entry_points(mock_storage, repo=None)

        assert isinstance(result, str)
        mock_storage.execute_raw.assert_called()

    def test_limit_respected(self, mock_storage: MagicMock) -> None:
        """Limit parameter is forwarded in the query."""
        mock_storage.execute_raw.return_value = []

        handle_entry_points(mock_storage, limit=5)

        call_args = mock_storage.execute_raw.call_args_list
        assert any("5" in str(c) for c in call_args)

    def test_centrality_shown(self, mock_storage: MagicMock) -> None:
        """Centrality score is included in output when non-zero."""
        mock_storage.execute_raw.side_effect = [
            [["run", "src/main.py", 1, "function", 0.75]],
        ]

        result = handle_entry_points(mock_storage)

        assert "centrality" in result


# ---------------------------------------------------------------------------
# axon_coverage_gaps tests
# ---------------------------------------------------------------------------


class TestHandleCoverageGaps:
    def test_returns_string(self, mock_storage: MagicMock) -> None:
        """handle_coverage_gaps always returns a string."""
        mock_storage.execute_raw.return_value = []

        result = handle_coverage_gaps(mock_storage)

        assert isinstance(result, str)

    def test_empty_results(self, mock_storage: MagicMock) -> None:
        """Returns a message when no coverage gaps are found."""
        mock_storage.execute_raw.return_value = []

        result = handle_coverage_gaps(mock_storage)

        assert "No coverage gaps found" in result or isinstance(result, str)

    def test_with_results(self, mock_storage: MagicMock) -> None:
        """Returns numbered list with centrality scores when gaps are found."""
        mock_storage.execute_raw.side_effect = [
            [
                ["process_payment", "src/payments.py", 42, "function", 0.88],
                ["validate_token", "src/auth.py", 10, "function", 0.55],
            ],
        ]

        result = handle_coverage_gaps(mock_storage)

        assert "process_payment" in result
        assert "validate_token" in result
        assert "Coverage gaps" in result

    def test_title_present(self, mock_storage: MagicMock) -> None:
        """Output includes the required title text."""
        mock_storage.execute_raw.side_effect = [
            [["my_func", "src/foo.py", 5, "function", 0.1]],
        ]

        result = handle_coverage_gaps(mock_storage)

        assert "Coverage gaps" in result

    def test_repo_not_found(self, mock_storage: MagicMock) -> None:
        """Returns not-found message when repo slug is unknown."""
        result = handle_coverage_gaps(mock_storage, repo="nonexistent-repo-xyz")

        assert "not found" in result.lower() or "registry" in result.lower()

    def test_repo_none_uses_storage(self, mock_storage: MagicMock) -> None:
        """repo=None uses the passed storage backend directly."""
        mock_storage.execute_raw.return_value = []

        result = handle_coverage_gaps(mock_storage, repo=None)

        assert isinstance(result, str)
        mock_storage.execute_raw.assert_called()

    def test_centrality_shown(self, mock_storage: MagicMock) -> None:
        """Centrality score is shown in output."""
        mock_storage.execute_raw.side_effect = [
            [["risky_fn", "src/risky.py", 99, "function", 0.99]],
        ]

        result = handle_coverage_gaps(mock_storage)

        assert "centrality" in result


# ---------------------------------------------------------------------------
# axon_path tests
# ---------------------------------------------------------------------------


class TestHandlePath:
    def test_returns_string(self, mock_storage: MagicMock) -> None:
        """handle_path always returns a string."""
        mock_storage.exact_name_search.return_value = []
        mock_storage.fts_search.return_value = []

        result = handle_path(mock_storage, "nonexistent_a", "nonexistent_b")

        assert isinstance(result, str)

    def test_from_symbol_not_found(self, mock_storage: MagicMock) -> None:
        """Returns not-found message when from_symbol does not exist."""
        mock_storage.exact_name_search.return_value = []
        mock_storage.fts_search.return_value = []

        result = handle_path(mock_storage, "ghost_fn", "other_fn")

        assert "ghost_fn" in result
        assert "not found" in result.lower()

    def test_to_symbol_not_found(self, mock_storage: MagicMock) -> None:
        """Returns not-found message when to_symbol does not exist."""
        from_sr = SearchResult(
            node_id="function:src/a.py:start",
            score=1.0,
            node_name="start",
            file_path="src/a.py",
            label="function",
        )
        # exact_name_search succeeds for from_symbol, fails for to_symbol
        mock_storage.exact_name_search.side_effect = [
            [from_sr],  # from_symbol found
            [],         # to_symbol not found → falls back to fts_search
        ]
        # fts_search only called once (fallback for ghost_target)
        mock_storage.fts_search.side_effect = [
            [],  # ghost_target not found in FTS either
        ]

        result = handle_path(mock_storage, "start", "ghost_target")

        assert "ghost_target" in result
        assert "not found" in result.lower()

    def test_no_path_between_symbols(self, mock_storage: MagicMock) -> None:
        """Returns no-path message when no CALLS chain connects the symbols."""
        from_sr = SearchResult(
            node_id="function:src/a.py:start",
            score=1.0,
            node_name="start",
            file_path="src/a.py",
            label="function",
        )
        to_sr = SearchResult(
            node_id="function:src/b.py:end",
            score=1.0,
            node_name="end",
            file_path="src/b.py",
            label="function",
        )
        mock_storage.fts_search.side_effect = [[from_sr], [to_sr]]
        mock_storage.exact_name_search.side_effect = [[from_sr], [to_sr]]
        # BFS returns no CALLS edges
        mock_storage.execute_raw.return_value = []

        result = handle_path(mock_storage, "start", "end")

        assert "No call path found" in result
        assert "start" in result
        assert "end" in result

    def test_path_found_direct_call(self, mock_storage: MagicMock) -> None:
        """Returns arrow-separated chain when a direct call path exists."""
        from_sr = SearchResult(
            node_id="function:src/a.py:caller",
            score=1.0,
            node_name="caller",
            file_path="src/a.py",
            label="function",
        )
        to_sr = SearchResult(
            node_id="function:src/b.py:callee",
            score=1.0,
            node_name="callee",
            file_path="src/b.py",
            label="function",
        )
        mock_storage.fts_search.side_effect = [[from_sr], [to_sr]]
        mock_storage.exact_name_search.side_effect = [[from_sr], [to_sr]]
        # BFS hop 1: caller -> callee (the target)
        mock_storage.execute_raw.return_value = [
            [
                "function:src/a.py:caller",
                "function:src/b.py:callee",
                "callee",
                "src/b.py",
            ]
        ]

        result = handle_path(mock_storage, "caller", "callee")

        assert "→" in result
        assert "caller" in result
        assert "callee" in result
        assert "1 hop" in result

    def test_same_symbol_path(self, mock_storage: MagicMock) -> None:
        """Returns zero-hop path when from and to are the same symbol."""
        sr = SearchResult(
            node_id="function:src/a.py:myfn",
            score=1.0,
            node_name="myfn",
            file_path="src/a.py",
            label="function",
        )
        mock_storage.fts_search.side_effect = [[sr], [sr]]
        mock_storage.exact_name_search.side_effect = [[sr], [sr]]

        result = handle_path(mock_storage, "myfn", "myfn")

        assert "0 hop" in result
        assert "myfn" in result

    def test_repo_not_found(self, mock_storage: MagicMock) -> None:
        """Returns not-found message when repo slug is unknown."""
        result = handle_path(mock_storage, "a", "b", repo="nonexistent-repo-xyz")

        assert "not found" in result.lower() or "registry" in result.lower()

    def test_repo_none_uses_storage(self, mock_storage: MagicMock) -> None:
        """repo=None uses the passed storage backend directly."""
        mock_storage.exact_name_search.return_value = []
        mock_storage.fts_search.return_value = []

        result = handle_path(mock_storage, "x", "y", repo=None)

        assert isinstance(result, str)
