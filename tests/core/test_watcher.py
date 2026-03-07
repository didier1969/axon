"""Tests for the watch mode module (watcher.py)."""

from __future__ import annotations

import inspect
import shutil
from pathlib import Path

import pytest

from axon.core.ingestion.pipeline import reindex_files
from axon.core.ingestion.walker import FileEntry, read_file
from axon.core.ingestion.watcher import _reindex_files, watch_repo
from axon.core.storage.kuzu_backend import KuzuBackend

# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------


@pytest.fixture()
def tmp_repo(tmp_path: Path) -> Path:
    """Create a small Python repository for watcher tests."""
    src = tmp_path / "src"
    src.mkdir()

    (src / "app.py").write_text(
        "def hello():\n"
        "    return 'hello'\n",
        encoding="utf-8",
    )

    (src / "utils.py").write_text(
        "def helper():\n"
        "    pass\n",
        encoding="utf-8",
    )

    return tmp_path


@pytest.fixture()
def storage(tmp_path: Path, watcher_indexed_template: Path) -> KuzuBackend:
    """Provide a KuzuBackend pre-indexed with src/app.py + src/utils.py.

    Copies the session-level pre-indexed template instead of calling
    run_pipeline() from scratch, saving 9-12s per test.
    """
    db_path = tmp_path / "test_db"
    shutil.copy2(str(watcher_indexed_template), str(db_path))
    backend = KuzuBackend()
    backend.initialize(db_path)  # schema already present: IF NOT EXISTS no-ops
    yield backend
    backend.close()


# ---------------------------------------------------------------------------
# Tests: _read_file_entry
# ---------------------------------------------------------------------------


class TestReadFileEntry:
    """_read_file_entry reads a file and returns a FileEntry."""

    def test_reads_python_file(self, tmp_repo: Path) -> None:
        entry = read_file(tmp_repo, tmp_repo / "src" / "app.py")

        assert entry is not None
        assert entry.path == "src/app.py"
        assert entry.language == "python"
        assert "hello" in entry.content

    def test_returns_entry_for_any_text_file(self, tmp_repo: Path) -> None:
        data_file = tmp_repo / "data.csv"
        data_file.write_text("a,b,c", encoding="utf-8")

        entry = read_file(tmp_repo, data_file)

        assert entry is not None
        assert entry.language in ("csv", "text")

    def test_returns_none_for_missing(self, tmp_repo: Path) -> None:
        entry = read_file(tmp_repo, tmp_repo / "nonexistent.py")

        assert entry is None

    def test_returns_none_for_empty(self, tmp_repo: Path) -> None:
        empty = tmp_repo / "empty.py"
        empty.write_text("", encoding="utf-8")

        entry = read_file(tmp_repo, empty)

        assert entry is None


# ---------------------------------------------------------------------------
# Tests: reindex_files (pipeline function)
# ---------------------------------------------------------------------------


class TestReindexFiles:
    """reindex_files() correctly removes old nodes and adds new ones."""

    def test_reindex_updates_content(
        self, tmp_repo: Path, storage: KuzuBackend
    ) -> None:
        # Verify initial node exists.
        node = storage.get_node("function:src/app.py:hello")
        assert node is not None
        assert "hello" in node.content

        # Modify the file.
        (tmp_repo / "src" / "app.py").write_text(
            "def hello():\n"
            "    return 'goodbye'\n",
            encoding="utf-8",
        )

        # Re-read and reindex.
        entry = FileEntry(
            path="src/app.py",
            content=(tmp_repo / "src" / "app.py").read_text(),
            language="python",
        )
        reindex_files([entry], tmp_repo, storage)

        # Verify updated node.
        node = storage.get_node("function:src/app.py:hello")
        assert node is not None
        assert "goodbye" in node.content

    def test_reindex_handles_new_symbols(
        self, tmp_repo: Path, storage: KuzuBackend
    ) -> None:
        # Add a new function to the file.
        (tmp_repo / "src" / "app.py").write_text(
            "def hello():\n"
            "    return 'hello'\n"
            "\n"
            "def world():\n"
            "    return 'world'\n",
            encoding="utf-8",
        )

        entry = FileEntry(
            path="src/app.py",
            content=(tmp_repo / "src" / "app.py").read_text(),
            language="python",
        )
        reindex_files([entry], tmp_repo, storage)

        # Both symbols should exist.
        assert storage.get_node("function:src/app.py:hello") is not None
        assert storage.get_node("function:src/app.py:world") is not None

    def test_reindex_removes_deleted_symbols(
        self, tmp_repo: Path, storage: KuzuBackend
    ) -> None:
        assert storage.get_node("function:src/app.py:hello") is not None

        # Remove the function.
        (tmp_repo / "src" / "app.py").write_text(
            "# empty file\nX = 1\n",
            encoding="utf-8",
        )

        entry = FileEntry(
            path="src/app.py",
            content=(tmp_repo / "src" / "app.py").read_text(),
            language="python",
        )
        reindex_files([entry], tmp_repo, storage)

        # Old symbol should be gone.
        assert storage.get_node("function:src/app.py:hello") is None


# ---------------------------------------------------------------------------
# Tests: _reindex_files (watcher helper)
# ---------------------------------------------------------------------------


class TestWatcherReindexFiles:
    """_reindex_files filters and processes changed paths."""

    def test_reindexes_changed_files(
        self, tmp_repo: Path, storage: KuzuBackend
    ) -> None:
        # Modify a file.
        app_path = tmp_repo / "src" / "app.py"
        app_path.write_text(
            "def hello():\n    return 'updated'\n",
            encoding="utf-8",
        )

        count = _reindex_files([app_path], tmp_repo, storage)

        assert count == 1
        node = storage.get_node("function:src/app.py:hello")
        assert node is not None
        assert "updated" in node.content

    def test_skips_ignored_files(
        self, tmp_repo: Path, storage: KuzuBackend
    ) -> None:
        # Create a file in an ignored directory.
        cache_dir = tmp_repo / "__pycache__"
        cache_dir.mkdir()
        cached = cache_dir / "module.cpython-311.pyc"
        cached.write_bytes(b"\x00")

        count = _reindex_files([cached], tmp_repo, storage)

        assert count == 0

    def test_no_longer_skips_unknown_files(
        self, tmp_repo: Path, storage: KuzuBackend
    ) -> None:
        data_file = tmp_repo / "data.csv"
        data_file.write_text("a,b,c", encoding="utf-8")

        count = _reindex_files([data_file], tmp_repo, storage)

        assert count == 1

    def test_handles_deleted_files(
        self, tmp_repo: Path, storage: KuzuBackend
    ) -> None:
        # File exists in storage but is now deleted from disk.
        deleted_path = tmp_repo / "src" / "app.py"
        assert storage.get_node("file:src/app.py:") is not None

        deleted_path.unlink()

        count = _reindex_files([deleted_path], tmp_repo, storage)

        # Returns 0 because file no longer exists (was handled as deletion).
        assert count == 0

    def test_handles_multiple_files(
        self, tmp_repo: Path, storage: KuzuBackend
    ) -> None:
        # Modify both files.
        (tmp_repo / "src" / "app.py").write_text(
            "def hello():\n    return 'v2'\n",
            encoding="utf-8",
        )
        (tmp_repo / "src" / "utils.py").write_text(
            "def helper():\n    return 42\n",
            encoding="utf-8",
        )

        count = _reindex_files(
            [tmp_repo / "src" / "app.py", tmp_repo / "src" / "utils.py"],
            tmp_repo,
            storage,
        )

        assert count == 2

    def test_paul_files_are_now_reindexed(
        self, tmp_repo: Path, storage: KuzuBackend
    ) -> None:
        paul_dir = tmp_repo / ".paul"
        paul_dir.mkdir()
        state_file = paul_dir / "STATE.md"
        state_file.write_text("# state\n", encoding="utf-8")

        count = _reindex_files([state_file], tmp_repo, storage)

        assert count == 1


class TestWatchRepoSignature:
    """watch_repo() signature checks."""

    def test_debounce_ms_param_accepted(self) -> None:
        sig = inspect.signature(watch_repo)
        assert "debounce_ms" in sig.parameters
        assert sig.parameters["debounce_ms"].default == 500


class TestWatchRepoQueue:
    """watch_repo() processes all batches via internal asyncio.Queue."""

    def test_processes_multiple_batches(
        self, tmp_repo: Path, storage: KuzuBackend
    ) -> None:
        """Both batches from distinct awatch events are processed."""
        import asyncio
        from unittest.mock import MagicMock, patch

        call_batches: list[list[Path]] = []
        _real_reindex = _reindex_files

        def tracking_reindex(paths: list[Path], *args, **kwargs) -> int:
            call_batches.append(list(paths))
            return _real_reindex(paths, *args, **kwargs)

        async def fake_awatch(*args, **kwargs):
            yield {(MagicMock(), str(tmp_repo / "src" / "app.py"))}
            yield {(MagicMock(), str(tmp_repo / "src" / "utils.py"))}

        with patch("axon.core.ingestion.watcher._reindex_files", tracking_reindex):
            with patch("watchfiles.awatch", fake_awatch):
                asyncio.run(watch_repo(tmp_repo, storage))

        # Both batches must be processed in separate _reindex_files calls
        assert len(call_batches) == 2
        paths_seen = {str(p) for batch in call_batches for p in batch}
        assert str(tmp_repo / "src" / "app.py") in paths_seen
        assert str(tmp_repo / "src" / "utils.py") in paths_seen

    def test_empty_changeset_not_queued(
        self, tmp_repo: Path, storage: KuzuBackend
    ) -> None:
        """An empty set of changes is not put into the queue."""
        import asyncio
        from unittest.mock import patch

        call_count: list[int] = []

        def counting_reindex(paths: list[Path], *args, **kwargs) -> int:
            call_count.append(len(paths))
            return 0

        async def fake_awatch(*args, **kwargs):
            # Empty set — all paths deduped away or watchfiles fires with no paths.
            # The producer's `if changed_paths:` guard must prevent queuing.
            yield set()

        with patch("axon.core.ingestion.watcher._reindex_files", counting_reindex):
            with patch("watchfiles.awatch", fake_awatch):
                asyncio.run(watch_repo(tmp_repo, storage))

        # Empty batch never enters the queue — _reindex_files never called.
        assert call_count == []


# ---------------------------------------------------------------------------
# Queue bounded by _WATCH_QUEUE_MAXSIZE
# ---------------------------------------------------------------------------


class TestWatchQueueBounded:
    """watch_repo uses a bounded queue of size _WATCH_QUEUE_MAXSIZE."""

    def test_watch_queue_maxsize_constant(self):
        from axon.core.ingestion.watcher import _WATCH_QUEUE_MAXSIZE

        assert _WATCH_QUEUE_MAXSIZE == 100

    def test_queue_created_with_maxsize(self):
        import asyncio

        from axon.core.ingestion.watcher import _WATCH_QUEUE_MAXSIZE

        q: asyncio.Queue = asyncio.Queue(maxsize=_WATCH_QUEUE_MAXSIZE)
        assert q.maxsize == 100
