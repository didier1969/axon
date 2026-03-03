"""Tests for cross-repo dependency edge creation."""
from __future__ import annotations

import json
from pathlib import Path

import pytest

from axon.core.graph.graph import KnowledgeGraph
from axon.core.graph.model import GraphNode, NodeLabel, RelType, generate_id
from axon.core.ingestion.cross_repo import (
    _parse_go_mod,
    _parse_package_json,
    _parse_pyproject_toml,
    process_cross_repo_deps,
)


def test_parse_pyproject_toml_extracts_deps(tmp_path: Path) -> None:
    (tmp_path / "pyproject.toml").write_text(
        "[project]\ndependencies = [\"requests>=2.28\", \"numpy\", \"click\"]\n",
        encoding="utf-8",
    )
    result = _parse_pyproject_toml(tmp_path)
    assert "requests" in result
    assert "numpy" in result
    assert "click" in result


def test_parse_pyproject_toml_missing_file(tmp_path: Path) -> None:
    assert _parse_pyproject_toml(tmp_path) == []


def test_parse_package_json_extracts_deps(tmp_path: Path) -> None:
    (tmp_path / "package.json").write_text(
        json.dumps({
            "dependencies": {"lodash": "^4.0"},
            "devDependencies": {"jest": "^29"},
        }),
        encoding="utf-8",
    )
    result = _parse_package_json(tmp_path)
    assert "lodash" in result
    assert "jest" in result


def test_parse_go_mod_extracts_deps(tmp_path: Path) -> None:
    (tmp_path / "go.mod").write_text(
        "module github.com/my/repo\n"
        "\n"
        "require (\n"
        "\tgithub.com/gin-gonic/gin v1.9.0\n"
        "\tgithub.com/stretchr/testify v1.8.0\n"
        ")\n",
        encoding="utf-8",
    )
    result = _parse_go_mod(tmp_path)
    assert "gin" in result
    assert "testify" in result


def test_process_cross_repo_deps_registered_match(tmp_path: Path) -> None:
    # Set up registry with "requests" slug
    registry = tmp_path / "registry"
    (registry / "requests").mkdir(parents=True)

    # Create pyproject.toml with requests as dependency
    repo_path = tmp_path / "myrepo"
    repo_path.mkdir()
    (repo_path / "pyproject.toml").write_text(
        "[project]\ndependencies = [\"requests>=2.28\"]\n",
        encoding="utf-8",
    )

    # Build a minimal KnowledgeGraph with one File node
    graph = KnowledgeGraph()
    file_id = generate_id(NodeLabel.FILE, "src/__init__.py")
    graph.add_node(GraphNode(
        id=file_id,
        label=NodeLabel.FILE,
        name="__init__.py",
        file_path="src/__init__.py",
    ))

    count = process_cross_repo_deps(graph, repo_path, registry_root=registry)

    assert count == 1
    rels = list(graph.iter_relationships())
    assert any(r.type == RelType.DEPENDS_ON for r in rels)


def test_process_cross_repo_deps_no_match(tmp_path: Path) -> None:
    # Empty registry — no matching slugs
    registry = tmp_path / "registry"
    registry.mkdir()

    repo_path = tmp_path / "myrepo"
    repo_path.mkdir()
    (repo_path / "pyproject.toml").write_text(
        "[project]\ndependencies = [\"numpy\"]\n",
        encoding="utf-8",
    )

    graph = KnowledgeGraph()
    file_id = generate_id(NodeLabel.FILE, "src/main.py")
    graph.add_node(GraphNode(
        id=file_id,
        label=NodeLabel.FILE,
        name="main.py",
        file_path="src/main.py",
    ))

    count = process_cross_repo_deps(graph, repo_path, registry_root=registry)

    assert count == 0
    rels = list(graph.iter_relationships())
    assert not any(r.type == RelType.DEPENDS_ON for r in rels)


def test_process_cross_repo_deps_no_manifest(tmp_path: Path) -> None:
    # Repo with no dependency files
    repo_path = tmp_path / "emptyrepo"
    repo_path.mkdir()

    registry = tmp_path / "registry"
    registry.mkdir()

    graph = KnowledgeGraph()
    count = process_cross_repo_deps(graph, repo_path, registry_root=registry)

    assert count == 0
