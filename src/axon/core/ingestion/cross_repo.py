"""Phase: Multi-repo DEPENDS_ON edge creation.

Parses pyproject.toml, package.json, and go.mod in the repo root
to find external dependencies. For each dependency name that matches
a registered Axon repo slug (in ~/.axon/repos/), creates a DEPENDS_ON
relationship from the local repo's root File node to a placeholder File
node for the dependency repo.

DEPENDS_ON edges are stored with rel_type='depends_on' in the existing
CodeRelation_File_File table — no schema change required.
"""

from __future__ import annotations

import logging
import re
from pathlib import Path

from axon.core.graph.graph import KnowledgeGraph
from axon.core.graph.model import (
    GraphNode,
    GraphRelationship,
    NodeLabel,
    RelType,
    generate_id,
)

logger = logging.getLogger(__name__)


def _extract_package_name(dep_spec: str) -> str:
    """Extract package name from a dependency specifier like 'requests>=2.0'."""
    return re.split(r"[>=<!\[;@ ]", dep_spec)[0].strip()


def _parse_pyproject_toml(repo_path: Path) -> list[str]:
    """Parse [project.dependencies] from pyproject.toml."""
    toml_path = repo_path / "pyproject.toml"
    if not toml_path.exists():
        return []
    try:
        try:
            import tomllib
        except ImportError:
            try:
                import tomli as tomllib  # type: ignore[no-reuse-def]
            except ImportError:
                return []
        with toml_path.open("rb") as f:
            data = tomllib.load(f)
        deps = data.get("project", {}).get("dependencies", [])
        return [_extract_package_name(d) for d in deps if isinstance(d, str)]
    except Exception:  # noqa: BLE001
        return []


def _parse_package_json(repo_path: Path) -> list[str]:
    """Parse dependencies + devDependencies from package.json."""
    import json as _json
    pkg_path = repo_path / "package.json"
    if not pkg_path.exists():
        return []
    try:
        data = _json.loads(pkg_path.read_text(encoding="utf-8"))
        deps = list(data.get("dependencies", {}).keys())
        dev_deps = list(data.get("devDependencies", {}).keys())
        return deps + dev_deps
    except Exception:  # noqa: BLE001
        return []


def _parse_go_mod(repo_path: Path) -> list[str]:
    """Parse require blocks from go.mod."""
    go_mod = repo_path / "go.mod"
    if not go_mod.exists():
        return []
    try:
        names = []
        in_require = False
        for line in go_mod.read_text(encoding="utf-8").splitlines():
            stripped = line.strip()
            if stripped.startswith("require ("):
                in_require = True
                continue
            if in_require and stripped == ")":
                in_require = False
                continue
            if stripped.startswith("//"):
                continue
            if in_require:
                # Inside require block: "github.com/gin-gonic/gin v1.9.0"
                parts = stripped.split()
                if parts:
                    names.append(parts[0].split("/")[-1])
            elif stripped.startswith("require "):
                # Single-line: "require github.com/gin-gonic/gin v1.9.0"
                parts = stripped.split()
                if len(parts) >= 2:
                    names.append(parts[1].split("/")[-1])
        return names
    except Exception:  # noqa: BLE001
        return []


def _get_registered_slugs(registry_root: Path | None = None) -> set[str]:
    """Return the set of repo slugs registered in ~/.axon/repos/."""
    if registry_root is None:
        registry_root = Path.home() / ".axon" / "repos"
    if not registry_root.exists():
        return set()
    return {d.name for d in registry_root.iterdir() if d.is_dir()}


def process_cross_repo_deps(
    graph: KnowledgeGraph,
    repo_path: Path,
    registry_root: Path | None = None,
) -> int:
    """Detect external repo dependencies and add DEPENDS_ON edges.

    Parses pyproject.toml, package.json, and go.mod in *repo_path* to find
    external dependencies. For each dependency name that matches a registered
    Axon repo slug, creates a DEPENDS_ON relationship from the local repo's
    root File node to a placeholder File node representing the dep repo.

    Args:
        graph: The knowledge graph to augment with DEPENDS_ON edges.
        repo_path: Root of the repo being indexed.
        registry_root: Override for ~/.axon/repos/ (used in tests).

    Returns:
        Number of DEPENDS_ON edges created.
    """
    dep_names: list[str] = []
    dep_names.extend(_parse_pyproject_toml(repo_path))
    dep_names.extend(_parse_package_json(repo_path))
    dep_names.extend(_parse_go_mod(repo_path))

    if not dep_names:
        return 0

    registered = _get_registered_slugs(registry_root)
    matches = [name for name in dep_names if name in registered]

    if not matches:
        return 0

    # Find the local repo's root File node (exact repo_path match first)
    local_root_id = None
    for node in graph.get_nodes_by_label(NodeLabel.FILE):
        if node.file_path in ("", ".", str(repo_path)):
            local_root_id = node.id
            break
    # Fallback: use any File node
    if local_root_id is None:
        for node in graph.get_nodes_by_label(NodeLabel.FILE):
            local_root_id = node.id
            break

    if local_root_id is None:
        logger.debug("No File node found for repo root — skipping DEPENDS_ON edges")
        return 0

    count = 0
    for dep_name in matches:
        dep_root_id = generate_id(NodeLabel.FILE, f"dep:{dep_name}", "")
        if not graph.get_node(dep_root_id):
            dep_node = GraphNode(
                id=dep_root_id,
                label=NodeLabel.FILE,
                name=dep_name,
                file_path=f"dep:{dep_name}",
            )
            graph.add_node(dep_node)

        rel_id = f"depends_on:{local_root_id}->{dep_root_id}"
        graph.add_relationship(GraphRelationship(
            id=rel_id,
            type=RelType.DEPENDS_ON,
            source=local_root_id,
            target=dep_root_id,
        ))
        count += 1
        logger.info("DEPENDS_ON edge: %s → %s", repo_path.name, dep_name)

    return count
