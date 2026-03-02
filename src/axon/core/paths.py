"""Shared filesystem path constants for axon.

Import from here instead of recomputing paths inline.
"""
from __future__ import annotations

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
