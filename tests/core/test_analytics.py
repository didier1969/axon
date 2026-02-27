"""Tests for the usage analytics event logger."""

from __future__ import annotations

import json
from pathlib import Path
from unittest.mock import patch

import pytest

from axon.core.analytics import log_event


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _read_events(events_path: Path) -> list[dict]:
    """Parse all JSON lines from an events file."""
    return [json.loads(line) for line in events_path.read_text(encoding="utf-8").splitlines() if line.strip()]


# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------


def test_log_event_creates_file(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """log_event creates events.jsonl when it does not exist."""
    monkeypatch.setattr(Path, "home", lambda: tmp_path)

    log_event("query", query="find auth", results=5)

    events_path = tmp_path / ".axon" / "events.jsonl"
    assert events_path.exists()
    events = _read_events(events_path)
    assert len(events) == 1


def test_log_event_appends(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """Multiple log_event calls produce one line per call."""
    monkeypatch.setattr(Path, "home", lambda: tmp_path)

    log_event("query", query="first")
    log_event("index", repo="axon", files=42)

    events_path = tmp_path / ".axon" / "events.jsonl"
    events = _read_events(events_path)
    assert len(events) == 2


def test_log_event_fields(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """Event records include ts, type, and all extra kwargs."""
    monkeypatch.setattr(Path, "home", lambda: tmp_path)

    log_event("query", query="test query", results=3, language="python", repo="my-repo")

    events_path = tmp_path / ".axon" / "events.jsonl"
    event = _read_events(events_path)[0]

    assert event["type"] == "query"
    assert event["query"] == "test query"
    assert event["results"] == 3
    assert event["language"] == "python"
    assert event["repo"] == "my-repo"
    assert "ts" in event


def test_log_event_ts_is_iso(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """Timestamp field is a valid ISO 8601 string."""
    from datetime import datetime

    monkeypatch.setattr(Path, "home", lambda: tmp_path)
    log_event("index", repo="proj")

    events_path = tmp_path / ".axon" / "events.jsonl"
    event = _read_events(events_path)[0]

    # Should parse without error
    datetime.fromisoformat(event["ts"])


def test_log_event_never_raises(monkeypatch: pytest.MonkeyPatch) -> None:
    """log_event does not raise even if the file cannot be written."""
    # Patch open to raise OSError
    monkeypatch.setattr("builtins.open", lambda *a, **kw: (_ for _ in ()).throw(OSError("disk full")))

    # Must not raise
    log_event("query", query="will fail silently")


def test_log_event_creates_parent_dirs(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """log_event creates ~/.axon/ directory if missing."""
    monkeypatch.setattr(Path, "home", lambda: tmp_path)

    axon_dir = tmp_path / ".axon"
    assert not axon_dir.exists()

    log_event("query", query="hello")

    assert axon_dir.exists()
    assert (axon_dir / "events.jsonl").exists()
