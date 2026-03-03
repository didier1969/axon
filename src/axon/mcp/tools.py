"""MCP tool handler implementations for Axon.

Each function accepts a storage backend and the tool-specific arguments,
performs the appropriate query, and returns a human-readable string suitable
for inclusion in an MCP ``TextContent`` response.
"""

from __future__ import annotations

import json
import logging
import os
import re
from pathlib import Path
from typing import Any

from axon.core.analytics import log_event
from axon.core.paths import central_db_path
from axon.core.search.hybrid import hybrid_search
from axon.core.storage.base import StorageBackend

logger = logging.getLogger(__name__)

MAX_TRAVERSE_DEPTH = 10
_MAX_RELATIONS_DISPLAYED = 20


def _escape_cypher(value: str) -> str:
    """Escape a string for safe inclusion in a Cypher string literal."""
    return value.replace("\\", "\\\\").replace("'", "\\'")

def _repo_name_from_storage(storage: StorageBackend, explicit_repo: str | None) -> str:
    """Resolve the repo name for analytics logging.

    Uses the explicit repo param when provided; falls back to the storage
    backend's db_path to derive the name from the registry path structure
    (``~/.axon/repos/<name>/kuzu``).
    """
    if explicit_repo:
        return explicit_repo
    db_path = getattr(storage, "db_path", None)
    if db_path is not None:
        # ~/.axon/repos/<repo-name>/kuzu  →  parent.name
        return db_path.parent.name
    return ""
_EMBED_MODEL_NAME = "BAAI/bge-small-en-v1.5"


def _confidence_tag(confidence: float) -> str:
    """Return a visual confidence indicator for edge display."""
    if confidence >= 0.9:
        return ""
    if confidence >= 0.5:
        return " (~)"
    return " (?)"


def _resolve_symbol(storage: StorageBackend, symbol: str, limit: int = 1) -> list:
    """Resolve a symbol name to search results, preferring exact name matches."""
    if hasattr(storage, "exact_name_search"):
        results = storage.exact_name_search(symbol, limit=limit)
        if results:
            return results
    return storage.fts_search(symbol, limit=limit)

_SAFE_SLUG_RE = re.compile(r'^[a-zA-Z0-9._-]{1,200}$')


def _sanitize_repo_slug(repo: str) -> str | None:
    """Return repo if it is a safe single-component slug, else None."""
    if not _SAFE_SLUG_RE.match(repo):
        logger.warning("Invalid repo slug rejected: %r", repo)
        return None
    p = Path(repo)
    if len(p.parts) != 1 or ".." in p.parts:
        logger.warning("Invalid repo slug rejected: %r", repo)
        return None
    return repo


def _load_repo_storage(repo: str) -> StorageBackend | None:
    """Open a read-only KuzuBackend for a named repo from the global registry.

    Reads ``~/.axon/repos/{repo}/meta.json``. Opens the DB from the central
    location: ``~/.axon/repos/{repo}/kuzu`` (v0.6+) or falls back to
    ``{meta["path"]}/.axon/kuzu`` for repos not yet migrated.

    Returns ``None`` (and logs at DEBUG) on any error so callers can return
    a user-friendly message without crashing.
    """
    repo = _sanitize_repo_slug(repo)
    if repo is None:
        return None

    from axon.core.storage.kuzu_backend import KuzuBackend

    meta_path = Path.home() / ".axon" / "repos" / repo / "meta.json"
    try:
        data = json.loads(meta_path.read_text(encoding="utf-8"))
        # Central path (v0.6+): registry dir itself holds kuzu
        central_db = central_db_path(repo)
        if central_db.exists():
            db_path = central_db
        else:
            # Legacy fallback: kuzu was at {project}/.axon/kuzu
            db_path = Path(data["path"]) / ".axon" / "kuzu"
        backend = KuzuBackend()
        backend.initialize(db_path, read_only=True)
        return backend
    except (OSError, json.JSONDecodeError, KeyError, RuntimeError) as exc:
        logger.debug("Failed to load repo storage for '%s': %s", repo, exc)
        return None


def handle_list_repos(registry_dir: Path | None = None) -> str:
    """List indexed repositories by scanning for .axon directories.

    Scans the global registry directory (defaults to ``~/.axon/repos``) for
    project metadata files and returns a formatted summary.

    Args:
        registry_dir: Directory containing repo metadata. If ``None``,
            defaults to ``~/.axon/repos``.

    Returns:
        Formatted list of indexed repositories with stats, or a message
        indicating none were found.
    """
    use_cwd_fallback = registry_dir is None
    if registry_dir is None:
        registry_dir = Path.home() / ".axon" / "repos"

    repos: list[dict[str, Any]] = []

    if registry_dir.exists():
        for meta_file in registry_dir.glob("*/meta.json"):
            try:
                data = json.loads(meta_file.read_text())
                repos.append(data)
            except (json.JSONDecodeError, OSError):
                continue

    if not repos and use_cwd_fallback:
        # Fall back: scan current directory for .axon
        cwd_axon = Path.cwd() / ".axon" / "meta.json"
        if cwd_axon.exists():
            try:
                data = json.loads(cwd_axon.read_text())
                repos.append(data)
            except (json.JSONDecodeError, OSError):
                pass

    if not repos:
        return "No indexed repositories found. Run `axon index` on a project first."

    lines = [f"Indexed repositories ({len(repos)}):"]
    lines.append("")
    for i, repo in enumerate(repos, 1):
        name = repo.get("name", "unknown")
        path = repo.get("path", "")
        stats = repo.get("stats", {})
        files = stats.get("files", "?")
        symbols = stats.get("symbols", "?")
        relationships = stats.get("relationships", "?")
        lines.append(f"  {i}. {name}")
        lines.append(f"     Path: {path}")
        lines.append(f"     Files: {files}  Symbols: {symbols}  Relationships: {relationships}")
        lines.append("")

    return "\n".join(lines)

def _group_by_process(
    results: list,
    storage: StorageBackend,
) -> dict[str, list]:
    """Map search results to their parent execution processes.

    Delegates to ``storage.get_process_memberships()`` for a safe
    parameterized query, falling back to an empty dict if the backend
    does not support the method.
    """
    if not results:
        return {}

    node_ids = [r.node_id for r in results]

    try:
        node_to_process = storage.get_process_memberships(node_ids)
    except (AttributeError, TypeError):
        return {}

    groups: dict[str, list] = {}
    for r in results:
        pname = node_to_process.get(r.node_id)
        if pname:
            groups.setdefault(pname, []).append(r)

    return groups


def _result_tags(r: Any) -> str:
    """Build compact inline quality tags for a search result."""
    tags: list[str] = []
    if getattr(r, "is_exported", False):
        tags.append("[exported]")
    # Only flag untested for function/method labels
    if not getattr(r, "tested", True) and r.label in ("function", "method"):
        tags.append("[untested]")
    return " " + " ".join(tags) if tags else ""


def _format_query_results(results: list, groups: dict[str, list]) -> str:
    """Format search results with process grouping.

    Results belonging to a process appear under a labelled section.
    Remaining results appear in an "Other results" section.
    """
    grouped_ids: set[str] = {r.node_id for group in groups.values() for r in group}
    ungrouped = [r for r in results if r.node_id not in grouped_ids]

    lines: list[str] = []
    counter = 1

    for process_name, proc_results in groups.items():
        lines.append(f"=== {process_name} ===")
        for r in proc_results:
            label = r.label.title() if r.label else "Unknown"
            tags = _result_tags(r)
            lines.append(f"{counter}. {r.node_name} ({label}) -- {r.file_path}{tags}")
            if r.snippet:
                snippet = r.snippet[:200].replace("\n", " ").strip()
                lines.append(f"   {snippet}")
            counter += 1
        lines.append("")

    if ungrouped:
        if groups:
            lines.append("=== Other results ===")
        for r in ungrouped:
            label = r.label.title() if r.label else "Unknown"
            tags = _result_tags(r)
            lines.append(f"{counter}. {r.node_name} ({label}) -- {r.file_path}{tags}")
            if r.snippet:
                snippet = r.snippet[:200].replace("\n", " ").strip()
                lines.append(f"   {snippet}")
            counter += 1
        lines.append("")

    lines.append("Next: Use context() on a specific symbol for the full picture.")
    return "\n".join(lines)


# Synonym map for simple heuristic query expansion (v0.8 placeholder).
# LLM-based expansion deferred to v0.9.
_EXPANSION_SYNONYMS: dict[str, str] = {
    "find": "search locate",
    "create": "build generate make",
    "delete": "remove destroy",
    "update": "modify change edit",
    "read": "fetch load get",
    "write": "save store persist",
    "parse": "decode interpret",
    "validate": "check verify",
    "send": "emit dispatch publish",
    "receive": "consume listen handle",
}


def _expand_query(query: str) -> str:
    """Return a heuristic expansion of *query* using synonym substitution.

    Appends synonym terms for the first matching keyword found.  Returns
    the original query if no keyword matches.
    """
    words = query.lower().split()
    for word in words:
        base = word.rstrip("s")  # crude stemming: "deletes" → "delete"
        for keyword, synonyms in _EXPANSION_SYNONYMS.items():
            if word == keyword or base == keyword:
                return f"{query} {synonyms}"
    return query


def handle_query(
    storage: StorageBackend,
    query: str,
    limit: int = 20,
    language: str | None = None,
    repo: str | None = None,
) -> str:
    """Execute hybrid search and format results, grouped by execution process.

    Args:
        storage: The storage backend to search against (used when repo is None).
        query: Text search query.
        limit: Maximum number of results (default 20).
        language: Optional language filter (e.g. "python", "elixir"). Case-insensitive.
        repo: Optional repository slug to query instead of the current directory.

    Returns:
        Formatted search results grouped by process, with file, name, label,
        and snippet for each result.
    """
    _repo_storage = None
    if repo is not None:
        _repo_storage = _load_repo_storage(repo)
        if _repo_storage is None:
            return f"Repository '{repo}' not found in registry. Use axon_list_repos to see available repos."
        storage = _repo_storage

    if os.getenv("AXON_QUERY_EXPAND"):
        query = _expand_query(query)

    try:
        query_embedding: list[float] | None = None
        try:
            from axon.core.embeddings.embedder import _get_model

            model = _get_model(_EMBED_MODEL_NAME)
            query_embedding = list(next(iter(model.embed([query]))))
        except (RuntimeError, ValueError, OSError):
            logger.debug("Query embedding failed, falling back to FTS only", exc_info=True)

        results = hybrid_search(query, storage, query_embedding=query_embedding, limit=limit)

        if language:
            lang_lower = language.lower()
            results = [r for r in results if r.language and r.language.lower() == lang_lower]
            if not results:
                return f"No results found for '{query}' in language '{language}'."

        if not results:
            return f"No results found for '{query}'."

        groups = _group_by_process(results, storage)
        result = _format_query_results(results, groups)
    finally:
        if _repo_storage is not None:
            try:
                _repo_storage.close()
            except Exception:  # noqa: BLE001
                pass

    log_event("query", query=query[:200], results=len(results), language=language or "", repo=_repo_name_from_storage(storage, repo))
    return result

def _parse_file_symbol(symbol: str) -> tuple[str | None, str]:
    """Parse a 'file/path.py:symbol_name' string into (file_hint, symbol_name).

    Returns (None, symbol) if no colon is found or input looks like a Windows drive path.
    """
    if ":" not in symbol:
        return None, symbol
    # Ignore Windows-style drive letters like "C:\..."
    if len(symbol) >= 2 and symbol[1] == ":" and symbol[0].isalpha():
        return None, symbol
    file_hint, _, sym_name = symbol.rpartition(":")
    return file_hint, sym_name


def handle_context(storage: StorageBackend, symbol: str, repo: str | None = None) -> str:
    """Provide a 360-degree view of a symbol.

    Looks up the symbol by name via full-text search, then retrieves its
    callers, callees, and type references.

    Supports 'file/path.py:symbol_name' format to disambiguate symbols that
    share the same name across different files.  When a bare name matches
    multiple symbols in different files, a disambiguation list is returned.

    Args:
        storage: The storage backend.
        symbol: The symbol name to look up, optionally prefixed with a file
            path (e.g. ``"src/parsers/python.py:parse"``).

    Returns:
        Formatted view including callers, callees, type refs, and guidance.
    """
    _repo_storage = None
    if repo is not None:
        _repo_storage = _load_repo_storage(repo)
        if _repo_storage is None:
            return f"Repository '{repo}' not found in registry. Use axon_list_repos to see available repos."
        storage = _repo_storage

    try:
        file_hint, sym_name = _parse_file_symbol(symbol)

        if file_hint:
            # File-qualified lookup: find candidates by name, then filter by file path.
            candidates = storage.fts_search(sym_name, limit=20)
            matches = [r for r in candidates if r.file_path and r.file_path.endswith(file_hint)]
            if not matches:
                return f"Symbol '{sym_name}' not found in '{file_hint}'."
            results = matches[:1]
        else:
            # Unqualified lookup: detect ambiguity across files.
            candidates = _resolve_symbol(storage, sym_name, limit=5)
            if not candidates:
                return f"Symbol '{sym_name}' not found."
            # Check for distinct file paths among candidates.
            seen_files: list[str] = []
            for r in candidates:
                if r.file_path and r.file_path not in seen_files:
                    seen_files.append(r.file_path)
            if len(seen_files) > 1:
                lines = [
                    f"Multiple symbols named '{sym_name}' found. "
                    "Specify a file path to disambiguate:",
                    "",
                ]
                for i, r in enumerate(candidates, 1):
                    label = r.label.title() if r.label else "Unknown"
                    lines.append(f"  {i}. {r.node_name}  ({label}) — {r.file_path}")
                lines.append("")
                lines.append(
                    f'Retry with: axon_context(symbol="path/to/file.py:{sym_name}")'
                )
                return "\n".join(lines)
            results = candidates[:1]

        node = storage.get_node(results[0].node_id)
        if not node:
            return f"Symbol '{sym_name}' not found."

        label_display = node.label.value.title() if node.label else "Unknown"
        lines = [f"Symbol: {node.name} ({label_display})"]
        lines.append(f"File: {node.file_path}:{node.start_line}-{node.end_line}")

        if node.signature:
            lines.append(f"Signature: {node.signature}")

        # Attributes: tested, exported, centrality
        tested_str = "yes" if getattr(node, "tested", False) else "no"
        exported_str = "yes" if getattr(node, "is_exported", False) else "no"
        centrality = getattr(node, "centrality", 0.0) or 0.0
        attr_line = f"Attributes: tested={tested_str}  exported={exported_str}"
        if centrality > 0.0:
            attr_line += f"  centrality={centrality:.3f}"
        lines.append(attr_line)

        if node.is_dead:
            lines.append("Status: DEAD CODE (unreachable)")

        try:
            callers_raw = storage.get_callers_with_confidence(node.id)
        except (AttributeError, TypeError):
            callers_raw = [(c, 1.0) for c in storage.get_callers(node.id)]

        if callers_raw:
            total = len(callers_raw)
            shown = callers_raw[:_MAX_RELATIONS_DISPLAYED]
            lines.append(f"\nCallers ({total}):")
            for c, conf in shown:
                tag = _confidence_tag(conf)
                lines.append(f"  -> {c.name}  {c.file_path}:{c.start_line}{tag}")
            if total > _MAX_RELATIONS_DISPLAYED:
                lines.append(f"  ... and {total - _MAX_RELATIONS_DISPLAYED} more")

        try:
            callees_raw = storage.get_callees_with_confidence(node.id)
        except (AttributeError, TypeError):
            callees_raw = [(c, 1.0) for c in storage.get_callees(node.id)]

        if callees_raw:
            total = len(callees_raw)
            shown = callees_raw[:_MAX_RELATIONS_DISPLAYED]
            lines.append(f"\nCallees ({total}):")
            for c, conf in shown:
                tag = _confidence_tag(conf)
                lines.append(f"  -> {c.name}  {c.file_path}:{c.start_line}{tag}")
            if total > _MAX_RELATIONS_DISPLAYED:
                lines.append(f"  ... and {total - _MAX_RELATIONS_DISPLAYED} more")

        type_refs = storage.get_type_refs(node.id)
        if type_refs:
            lines.append(f"\nType references ({len(type_refs)}):")
            for t in type_refs:
                lines.append(f"  -> {t.name}  {t.file_path}")

        lines.append("")
        lines.append("Next: Use impact() if planning changes to this symbol.")
        result = "\n".join(lines)
    finally:
        if _repo_storage is not None:
            try:
                _repo_storage.close()
            except Exception:  # noqa: BLE001
                pass

    log_event("context", symbol=symbol[:200], repo=_repo_name_from_storage(storage, repo))
    return result

_DEPTH_LABELS: dict[int, str] = {
    1: "Direct callers (will break)",
    2: "Indirect (may break)",
}


def handle_impact(storage: StorageBackend, symbol: str, depth: int = 3, repo: str | None = None) -> str:
    """Analyse the blast radius of changing a symbol, grouped by hop depth.

    Uses BFS traversal through CALLS edges to find all affected symbols
    up to the specified depth, then groups results by distance.

    Args:
        storage: The storage backend (used when repo is None).
        symbol: The symbol name to analyse.
        depth: Maximum traversal depth (default 3).
        repo: Optional repository slug to query instead of the current directory.

    Returns:
        Formatted impact analysis with depth-grouped sections.
    """
    _repo_storage = None
    if repo is not None:
        _repo_storage = _load_repo_storage(repo)
        if _repo_storage is None:
            return f"Repository '{repo}' not found in registry. Use axon_list_repos to see available repos."
        storage = _repo_storage

    try:
        depth = max(1, min(depth, MAX_TRAVERSE_DEPTH))

        results = _resolve_symbol(storage, symbol)
        if not results:
            return f"Symbol '{symbol}' not found."

        start_node = storage.get_node(results[0].node_id)
        if not start_node:
            return f"Symbol '{symbol}' not found."

        affected_with_depth = storage.traverse_with_depth(
            start_node.id, depth, direction="callers"
        )
        if not affected_with_depth:
            return f"No upstream callers found for '{symbol}'."

        # Group by depth
        by_depth: dict[int, list] = {}
        for node, d in affected_with_depth:
            by_depth.setdefault(d, []).append(node)

        total = len(affected_with_depth)
        label_display = start_node.label.value.title()
        lines = [f"Impact analysis for: {start_node.name} ({label_display})"]
        lines.append(f"Depth: {depth} | Total: {total} symbols")

        # Build confidence lookup for depth-1 (direct callers) display
        conf_lookup: dict[str, float] = {}
        try:
            for node, conf in storage.get_callers_with_confidence(start_node.id):
                conf_lookup[node.id] = conf
        except (AttributeError, TypeError):
            pass

        counter = 1
        for d in sorted(by_depth.keys()):
            depth_label = _DEPTH_LABELS.get(d, "Transitive (review)")
            lines.append(f"\nDepth {d} — {depth_label}:")
            for node in by_depth[d]:
                label = node.label.value.title() if node.label else "Unknown"
                conf = conf_lookup.get(node.id)
                tag = f"  (confidence: {conf:.2f})" if conf is not None else ""
                lines.append(
                    f"  {counter}. {node.name} ({label}) -- "
                    f"{node.file_path}:{node.start_line}{tag}"
                )
                counter += 1

        lines.append("")
        lines.append("Tip: Review each affected symbol before making changes.")
        result = "\n".join(lines)
    finally:
        if _repo_storage is not None:
            try:
                _repo_storage.close()
            except Exception:  # noqa: BLE001
                pass

    log_event("impact", symbol=symbol[:200], repo=_repo_name_from_storage(storage, repo))
    return result

def handle_dead_code(storage: StorageBackend) -> str:
    """List all symbols marked as dead code.

    Delegates to :func:`~axon.mcp.resources.get_dead_code_list` for the
    shared query and formatting.

    Args:
        storage: The storage backend.

    Returns:
        Formatted list of dead code symbols grouped by file.
    """
    from axon.mcp.resources import get_dead_code_list

    return get_dead_code_list(storage)

_DIFF_FILE_PATTERN = re.compile(r"^diff --git a/(.+?) b/(.+?)$", re.MULTILINE)
_DIFF_HUNK_PATTERN = re.compile(r"^@@ -\d+(?:,\d+)? \+(\d+)(?:,(\d+))? @@", re.MULTILINE)

def handle_detect_changes(storage: StorageBackend, diff: str) -> str:
    """Map git diff output to affected symbols.

    Parses the diff to find changed files and line ranges, then queries
    the storage backend to identify which symbols those lines belong to.

    Args:
        storage: The storage backend.
        diff: Raw git diff output string.

    Returns:
        Formatted list of affected symbols per changed file.
    """
    if not diff.strip():
        return "Empty diff provided."

    changed_files: dict[str, list[tuple[int, int]]] = {}
    current_file: str | None = None

    for line in diff.split("\n"):
        file_match = _DIFF_FILE_PATTERN.match(line)
        if file_match:
            current_file = file_match.group(2)
            if current_file not in changed_files:
                changed_files[current_file] = []
            continue

        hunk_match = _DIFF_HUNK_PATTERN.match(line)
        if hunk_match and current_file is not None:
            start = int(hunk_match.group(1))
            count = int(hunk_match.group(2) or "1")
            changed_files[current_file].append((start, start + count - 1))

    if not changed_files:
        return "Could not parse any changed files from the diff."

    lines = [f"Changed files: {len(changed_files)}"]
    lines.append("")
    total_affected = 0

    all_file_paths = list(changed_files.keys())
    # Single parameterised query — no Cypher injection, no N+1
    try:
        all_rows = storage.execute_raw(
            "MATCH (n) WHERE n.file_path IN $fps AND n.start_line > 0 "
            "RETURN n.id, n.name, n.file_path, n.start_line, n.end_line",
            parameters={"fps": all_file_paths},
        ) or []
    except (RuntimeError, ValueError) as exc:
        logger.warning("Failed to query symbols for batch: %s", exc, exc_info=True)
        all_rows = []

    # Group results by file_path
    results_by_file: dict[str, list] = {}
    for row in all_rows:
        fp = row[2] or ""
        results_by_file.setdefault(fp, []).append(row)

    for file_path, ranges in changed_files.items():
        affected_symbols = []
        rows = results_by_file.get(file_path, [])
        for row in rows:
            node_id = row[0] or ""
            name = row[1] or ""
            start_line = row[3] or 0
            end_line = row[4] or 0
            label_prefix = node_id.split(":", 1)[0] if node_id else ""
            for start, end in ranges:
                if start_line <= end and end_line >= start:
                    affected_symbols.append(
                        (name, label_prefix.title(), start_line, end_line)
                    )
                    break
        lines.append(f"  {file_path}:")
        if affected_symbols:
            for sym_name, label, s_line, e_line in affected_symbols:
                lines.append(f"    - {sym_name} ({label}) lines {s_line}-{e_line}")
                total_affected += 1
        else:
            lines.append("    (no indexed symbols in changed lines)")
        lines.append("")

    lines.append(f"Total affected symbols: {total_affected}")
    lines.append("")
    lines.append("Next: Use impact() on affected symbols to see downstream effects.")
    return "\n".join(lines)

def _get_repo_root_from_storage(storage: StorageBackend) -> Path | None:
    """Derive the repo root path from the storage backend's db_path via meta.json."""
    db_path = getattr(storage, "db_path", None)
    if db_path is None:
        return None
    meta_path = Path(db_path).parent / "meta.json"
    try:
        data = json.loads(meta_path.read_text(encoding="utf-8"))
        repo_path = data.get("path")
        if repo_path:
            return Path(repo_path)
    except (OSError, json.JSONDecodeError):
        pass
    return None


def handle_read_symbol(
    storage: StorageBackend,
    symbol: str,
    file: str | None = None,
    repo: str | None = None,
) -> str:
    """Return the exact source of a symbol using byte offsets (O(1) file read).

    Looks up start_byte/end_byte from the graph and slices the source file
    directly.  Falls back to the stored content field when byte offsets are
    unavailable (e.g. legacy DBs or regex-based parsers not yet migrated).
    """
    _storage = storage
    if repo:
        repo_storage = _load_repo_storage(repo)
        if repo_storage is None:
            return f"Repository not found: {repo}"
        _storage = repo_storage

    if file:
        rows = _storage.execute_raw(
            "MATCH (n) WHERE n.name = $name AND n.file_path CONTAINS $file "
            "RETURN n.name, n.file_path, n.start_line, n.start_byte, n.end_byte, n.content",
            parameters={"name": symbol, "file": file},
        )
    else:
        rows = _storage.execute_raw(
            "MATCH (n) WHERE n.name = $name "
            "RETURN n.name, n.file_path, n.start_line, n.start_byte, n.end_byte, n.content",
            parameters={"name": symbol},
        )

    if not rows:
        return f"Symbol not found: {symbol}"

    if len(rows) > 1:
        lines = [f"Multiple symbols named '{symbol}' — specify `file` to disambiguate:"]
        for row in rows[:10]:
            lines.append(f"  - {row[1]} (line {row[2]})")
        if len(rows) > 10:
            lines.append(f"  ... and {len(rows) - 10} more")
        return "\n".join(lines)

    row = rows[0]
    sym_name, file_path, start_line, start_byte, end_byte, stored_content = row

    if start_byte and end_byte and start_byte < end_byte:
        repo_root = _get_repo_root_from_storage(_storage)
        if repo_root is not None:
            abs_path = repo_root / file_path
            try:
                raw = abs_path.read_bytes()[start_byte:end_byte]
                source = raw.decode("utf-8", errors="replace")
                return f"# {sym_name} — {file_path}:{start_line}\n\n{source}"
            except OSError:
                pass

    note = "(byte offsets unavailable, using stored content)"
    content_text = stored_content or "(no content stored)"
    return f"# {sym_name} — {file_path}:{start_line}\n{note}\n\n{content_text}"


def handle_find_similar(
    storage: StorageBackend,
    symbol: str,
    limit: int = 10,
    repo: str | None = None,
) -> str:
    """Find symbols semantically similar to *symbol* using stored embeddings.

    Args:
        storage: The storage backend.
        symbol: Name of the symbol to find similar symbols for.
        limit: Maximum number of similar symbols to return (default 10).
        repo: Optional repository slug to query instead of the current directory.

    Returns:
        Formatted list of semantically similar symbols with similarity scores.
    """
    _repo_storage = None
    if repo is not None:
        _repo_storage = _load_repo_storage(repo)
        if _repo_storage is None:
            return f"Repository '{repo}' not found in registry. Use axon_list_repos to see available repos."
        storage = _repo_storage

    try:
        # Resolve symbol to a node
        results = _resolve_symbol(storage, symbol, limit=5)
        if not results:
            return f"Symbol '{symbol}' not found."

        node_id = results[0].node_id
        node_name = results[0].node_name
        node_file = results[0].file_path

        # Get the stored embedding for the node
        embedding: list[float] | None = None
        if hasattr(storage, "get_embedding"):
            embedding = storage.get_embedding(node_id)

        if not embedding:
            return (
                f"No embedding found for '{symbol}'. "
                "Run axon analyze first to generate embeddings."
            )

        # Find similar symbols (fetch limit+1 to exclude self)
        similar = storage.vector_search(embedding, limit + 1)

        # Filter out the queried symbol itself
        similar = [r for r in similar if r.node_id != node_id][:limit]

        if not similar:
            return f"No similar symbols found for '{symbol}'."

        lines = [f"Similar to: {node_name} ({node_file})"]
        lines.append("─" * 45)
        for i, r in enumerate(similar, 1):
            label = r.label.title() if r.label else "Unknown"
            sim_pct = int(r.score * 100)
            lines.append(f"{i}. {r.node_name} ({label}) — {r.file_path}  [sim: {sim_pct}%]")
            if r.snippet:
                snippet = r.snippet[:120].replace("\n", " ").strip()
                lines.append(f"   {snippet}")

        result = "\n".join(lines)
    finally:
        if _repo_storage is not None:
            try:
                _repo_storage.close()
            except Exception:  # noqa: BLE001
                pass

    log_event("find_similar", symbol=symbol[:200], repo=_repo_name_from_storage(storage, repo))
    return result


def handle_find_usages(
    storage: StorageBackend,
    symbol: str,
    limit: int = 50,
    repo: str | None = None,
) -> str:
    """Find all call-sites and import-sites of a symbol across the repo.

    Args:
        storage: The storage backend.
        symbol: Name of the symbol to find usages for.
        limit: Maximum number of usages to return (default 50).
        repo: Optional repository slug to query instead of the current directory.

    Returns:
        Formatted list of CALLS and IMPORTS sites, or a not-found message.
    """
    _repo_storage = None
    if repo is not None:
        _repo_storage = _load_repo_storage(repo)
        if _repo_storage is None:
            return f"Repository '{repo}' not found in registry. Use axon_list_repos to see available repos."
        storage = _repo_storage

    try:
        results = _resolve_symbol(storage, symbol, limit=1)
        if not results:
            return f"Symbol '{symbol}' not found."

        node_id = results[0].node_id
        node_name = results[0].node_name
        node_file = results[0].file_path

        calls_rows = storage.execute_raw(
            "MATCH (caller)-[r:CodeRelation]->(callee) "
            "WHERE callee.id = $nid AND r.rel_type = 'calls' "
            "RETURN caller.name, caller.file_path, caller.start_line "
            f"LIMIT {limit}",
            parameters={"nid": node_id},
        ) or []

        imports_rows = storage.execute_raw(
            "MATCH (importer)-[r:CodeRelation]->(imported) "
            "WHERE imported.file_path = $fp AND r.rel_type = 'imports' "
            "RETURN importer.name, importer.file_path, importer.start_line "
            f"LIMIT {limit}",
            parameters={"fp": node_file},
        ) or []

        # Deduplicate importers by file_path
        seen_files: set[str] = set()
        unique_imports: list = []
        for row in imports_rows:
            fp = row[1] if len(row) > 1 else row[0]
            if fp not in seen_files:
                seen_files.add(fp)
                unique_imports.append(row)

        if not calls_rows and not unique_imports:
            return f"No usages found for '{symbol}'."

        lines = [f"{len(calls_rows)} call sites for '{node_name}' ({node_file}):", ""]

        if calls_rows:
            lines.append(f"CALLS ({len(calls_rows)}):")
            for i, row in enumerate(calls_rows, 1):
                name = row[0] if row else "?"
                fp = row[1] if len(row) > 1 else "?"
                line = row[2] if len(row) > 2 else None
                loc = f"{fp}:L{line}" if line is not None else fp
                lines.append(f"  {i}. {name}  {loc}")
            lines.append("")

        if unique_imports:
            lines.append(f"IMPORTS ({len(unique_imports)}):")
            for i, row in enumerate(unique_imports, 1):
                fp = row[1] if len(row) > 1 else row[0]
                lines.append(f"  {i}. {fp}")
            lines.append("")

        lines.append(f"Next: Use axon_impact('{node_name}') for transitive blast radius.")
        result = "\n".join(lines)
    finally:
        if _repo_storage is not None:
            try:
                _repo_storage.close()
            except Exception:  # noqa: BLE001
                pass

    log_event("find_usages", symbol=symbol[:200], repo=_repo_name_from_storage(storage, repo))
    return result


_WRITE_KEYWORDS = re.compile(
    r"\b(DELETE|DROP|CREATE|SET|REMOVE|MERGE|DETACH|INSTALL|LOAD|COPY|CALL"
    r"|RENAME|ALTER|IMPORT|TRUNCATE)\b",
    re.IGNORECASE,
)

def handle_cypher(storage: StorageBackend, query: str) -> str:
    """Execute a raw Cypher query and return formatted results.

    Only read-only queries are allowed.  Queries containing write keywords
    (DELETE, DROP, CREATE, SET, etc.) are rejected.

    Args:
        storage: The storage backend.
        query: The Cypher query string.

    Returns:
        Formatted query results, or an error message if execution fails.
    """
    if _WRITE_KEYWORDS.search(query):
        return (
            "Query rejected: only read-only queries (MATCH/RETURN) are allowed. "
            "Write operations (DELETE, DROP, CREATE, SET, MERGE) are not permitted."
        )

    try:
        rows = storage.execute_raw(query)
    except (RuntimeError, ValueError) as exc:
        return f"Cypher query failed: {exc}"

    if not rows:
        return "Query returned no results."

    lines = [f"Results ({len(rows)} rows):"]
    lines.append("")
    for i, row in enumerate(rows, 1):
        formatted_values = [str(v) for v in row]
        lines.append(f"  {i}. {' | '.join(formatted_values)}")

    return "\n".join(lines)
