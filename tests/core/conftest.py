"""Shared test fixtures for tests/core/."""
from __future__ import annotations

import shutil
from pathlib import Path
from unittest.mock import patch

import pytest

from axon.core.storage.astral_backend import AstralBackend


@pytest.fixture(autouse=True)
def isolated_axon_home(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """Redirect Path.home() to tmp_path for every test.

    Prevents log_event() calls inside run_pipeline() from writing to the
    real ~/.axon/events.jsonl during test runs.
    """
    monkeypatch.setattr(Path, "home", lambda: tmp_path)


@pytest.fixture(scope="session")
def astral_template(tmp_path_factory: pytest.TempPathFactory) -> Path:
    """Create a mock storage path for AstralBackend."""
    db_path = tmp_path_factory.mktemp("astral_template") / "db"
    db_path.parent.mkdir(parents=True, exist_ok=True)
    db_path.touch()
    return db_path


@pytest.fixture(scope="session")
def watcher_indexed_template(
    tmp_path_factory: pytest.TempPathFactory,
    astral_template: Path,
) -> Path:
    """Mock pre-indexed repository for watcher tests."""
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

    db_path = tmp_path_factory.mktemp("watcher_indexed_db") / "db"
    fake_home = tmp_path_factory.mktemp("watcher_template_home")
    
    with patch.object(Path, "home", return_value=fake_home), \
         patch("axon.core.storage.astral_backend.httpx.Client") as mock_client:
        # Mock communication for pipeline indexing
        mock_client.return_value.get.return_value.status_code = 200
        backend = AstralBackend()
        backend.initialize(db_path)
        run_pipeline(repo_dir, backend, embeddings=False)
        backend.close()

    return db_path
