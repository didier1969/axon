"""Tests for the LRU backend cache."""
from unittest.mock import MagicMock, patch

from axon.core.paths import central_db_path
from axon.daemon.lru_cache import LRUBackendCache


def _make_mock_backend():
    """Return a MagicMock with initialize() and close() methods."""
    b = MagicMock()
    b.close = MagicMock()
    return b


class TestLRUBackendCache:
    def test_get_returns_none_for_unknown_slug(self, tmp_path, monkeypatch):
        """get_or_load returns None when no central DB exists."""
        monkeypatch.setattr("pathlib.Path.home", lambda: tmp_path)
        cache = LRUBackendCache(maxsize=3)
        result = cache.get_or_load("nonexistent")
        assert result is None

    def test_loads_backend_when_db_exists(self, tmp_path, monkeypatch):
        """get_or_load returns a backend when central DB path exists."""
        monkeypatch.setattr("pathlib.Path.home", lambda: tmp_path)
        db_path = central_db_path("myrepo")
        db_path.mkdir(parents=True)

        mock_backend = _make_mock_backend()
        with patch("axon.core.storage.astral_backend.AstralBackend", return_value=mock_backend):
            cache = LRUBackendCache(maxsize=3)
            result = cache.get_or_load("myrepo")

        assert result is mock_backend
        mock_backend.initialize.assert_called_once()

    def test_second_get_returns_cached_without_reload(self, tmp_path, monkeypatch):
        """Second call to get_or_load for same slug returns cached backend."""
        monkeypatch.setattr("pathlib.Path.home", lambda: tmp_path)
        db_path = central_db_path("myrepo")
        db_path.mkdir(parents=True)

        mock_backend = _make_mock_backend()
        with patch("axon.core.storage.astral_backend.AstralBackend", return_value=mock_backend):
            cache = LRUBackendCache(maxsize=3)
            r1 = cache.get_or_load("myrepo")
            r2 = cache.get_or_load("myrepo")

        assert r1 is r2
        assert mock_backend.initialize.call_count == 1

    def test_lru_eviction_calls_close(self, tmp_path, monkeypatch):
        """When cache is full, LRU backend is evicted and close() is called."""
        monkeypatch.setattr("pathlib.Path.home", lambda: tmp_path)

        backends = {}
        for slug in ["a", "b", "c", "d"]:
            central_db_path(slug).mkdir(parents=True)
            backends[slug] = _make_mock_backend()

        cache = LRUBackendCache(maxsize=3)

        with patch("axon.core.storage.astral_backend.AstralBackend") as MockKuzu:
            MockKuzu.side_effect = [backends["a"], backends["b"], backends["c"], backends["d"]]
            cache.get_or_load("a")
            cache.get_or_load("b")
            cache.get_or_load("c")
            # Cache full: a is LRU. Loading d should evict a.
            cache.get_or_load("d")

        backends["a"].close.assert_called_once()
        backends["b"].close.assert_not_called()
        backends["c"].close.assert_not_called()

    def test_get_reorders_to_mru(self, tmp_path, monkeypatch):
        """Accessing a cached backend moves it to MRU position."""
        monkeypatch.setattr("pathlib.Path.home", lambda: tmp_path)

        for slug in ["a", "b", "c", "d"]:
            central_db_path(slug).mkdir(parents=True)

        mock_backends = {s: _make_mock_backend() for s in ["a", "b", "c", "d"]}
        cache = LRUBackendCache(maxsize=3)

        with patch("axon.core.storage.astral_backend.AstralBackend") as MockKuzu:
            MockKuzu.side_effect = list(mock_backends.values())
            cache.get_or_load("a")
            cache.get_or_load("b")
            cache.get_or_load("c")
            # Access 'a' to make it MRU (b becomes LRU)
            cache.get_or_load("a")
            # Add 'd' — should evict 'b' (LRU), not 'a'
            cache.get_or_load("d")

        mock_backends["b"].close.assert_called_once()
        mock_backends["a"].close.assert_not_called()

    def test_close_all_closes_all_backends(self, tmp_path, monkeypatch):
        """close_all() closes every cached backend."""
        monkeypatch.setattr("pathlib.Path.home", lambda: tmp_path)

        for slug in ["x", "y"]:
            central_db_path(slug).mkdir(parents=True)

        bx, by = _make_mock_backend(), _make_mock_backend()
        cache = LRUBackendCache(maxsize=5)
        with patch("axon.core.storage.astral_backend.AstralBackend") as MockKuzu:
            MockKuzu.side_effect = [bx, by]
            cache.get_or_load("x")
            cache.get_or_load("y")

        cache.close_all()
        bx.close.assert_called_once()
        by.close.assert_called_once()
        assert cache.status()["count"] == 0

    def test_status_reflects_cache(self, tmp_path, monkeypatch):
        """status() returns correct cached slug list and count."""
        monkeypatch.setattr("pathlib.Path.home", lambda: tmp_path)
        for slug in ["p", "q"]:
            central_db_path(slug).mkdir(parents=True)

        bp, bq = _make_mock_backend(), _make_mock_backend()
        cache = LRUBackendCache(maxsize=5)
        with patch("axon.core.storage.astral_backend.AstralBackend") as MockKuzu:
            MockKuzu.side_effect = [bp, bq]
            cache.get_or_load("p")
            cache.get_or_load("q")

        s = cache.status()
        assert s["count"] == 2
        assert s["maxsize"] == 5
        assert "p" in s["cached"]
        assert "q" in s["cached"]
