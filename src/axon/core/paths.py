"""Shared filesystem path constants for axon.

Import from here instead of recomputing paths inline.
"""
from __future__ import annotations

import hashlib
import json
from pathlib import Path


def central_db_path(slug: str) -> Path:
    """~/.axon/repos/{slug}/kuzu — centralised KuzuDB path (v0.6+)."""
    return Path.home() / ".axon" / "repos" / slug / "kuzu"


def daemon_sock_path() -> Path:
    """~/.axon/daemon.sock — Unix socket for axon daemon IPC."""
    return Path.home() / ".axon" / "daemon.sock"


def daemon_pid_path() -> Path:
    """~/.axon/daemon.pid — PID file for the running daemon."""
    return Path.home() / ".axon" / "daemon.pid"


def compute_repo_slug(repo_path: Path) -> str:
    """Compute the registry slug for a repository path.

    Returns repo_path.name unless that name is already claimed by a
    different repo in the global registry, in which case returns
    f"{repo_path.name}-{sha256(str(repo_path))[:8]}".
    """
    slug = repo_path.name
    candidate_meta = Path.home() / ".axon" / "repos" / slug / "meta.json"
    if candidate_meta.exists():
        try:
            existing = json.loads(candidate_meta.read_text(encoding="utf-8"))
            if existing.get("path") != str(repo_path):
                slug = f"{repo_path.name}-{hashlib.sha256(str(repo_path).encode()).hexdigest()[:8]}"
        except (json.JSONDecodeError, OSError):
            pass
    return slug
