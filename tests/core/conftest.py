"""Shared test fixtures for tests/core/."""
from __future__ import annotations

import shutil
from pathlib import Path

import pytest

from axon.core.storage.kuzu_backend import KuzuBackend


@pytest.fixture(autouse=True)
def isolated_axon_home(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """Redirect Path.home() to tmp_path for every test.

    Prevents log_event() calls inside run_pipeline() from writing to the
    real ~/.axon/events.jsonl during test runs.
    """
    monkeypatch.setattr(Path, "home", lambda: tmp_path)


@pytest.fixture(scope="session")
def kuzu_template(tmp_path_factory: pytest.TempPathFactory) -> Path:
    """Create a schema-initialized empty KuzuDB once per session.

    Function-scoped storage fixtures copy this template instead of
    calling KuzuBackend.initialize() from scratch, saving 4-5s per test.
    The schema is already present in the copy so create_schema() IF NOT
    EXISTS calls become no-ops.
    """
    db_path = tmp_path_factory.mktemp("kuzu_template") / "db"
    backend = KuzuBackend()
    backend.initialize(db_path)
    backend.close()
    return db_path


@pytest.fixture(scope="session")
def watcher_indexed_template(
    tmp_path_factory: pytest.TempPathFactory,
    kuzu_template: Path,
) -> Path:
    """KuzuDB pre-indexed with the standard watcher test repo (src/app.py + src/utils.py).

    Amortizes run_pipeline() cost across all TestReindexFiles and
    TestWatcherReindexFiles tests. Each test copies this template instead of
    calling run_pipeline() from scratch, saving 5-7s per test.

    Path.home() is patched during indexing to prevent events.jsonl pollution.
    """
    from unittest.mock import patch

    from axon.core.ingestion.pipeline import run_pipeline

    # Create repo matching test_watcher.py's tmp_repo fixture exactly.
    repo_dir = tmp_path_factory.mktemp("watcher_template_repo")
    src = repo_dir / "src"
    src.mkdir()
    (src / "app.py").write_text(
        "def hello():\n    return 'hello'\n", encoding="utf-8"
    )
    (src / "utils.py").write_text(
        "def helper():\n    pass\n", encoding="utf-8"
    )

    # Copy empty schema template, index, close.
    db_path = tmp_path_factory.mktemp("watcher_indexed_db") / "db"
    shutil.copy2(str(kuzu_template), str(db_path))

    fake_home = tmp_path_factory.mktemp("watcher_template_home")
    with patch.object(Path, "home", return_value=fake_home):
        backend = KuzuBackend()
        backend.initialize(db_path)
        run_pipeline(repo_dir, backend, embeddings=False)
        backend.close()

    return db_path
