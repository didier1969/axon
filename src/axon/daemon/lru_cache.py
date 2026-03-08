"""Thread-safe LRU cache for AstralBackend instances.

Evicts least-recently-used backend when at capacity, calling close() on evicted entries.
"""
from __future__ import annotations

import logging
from collections import OrderedDict
from threading import Lock
from typing import TYPE_CHECKING

from axon.core.paths import central_db_path

if TYPE_CHECKING:
    from axon.core.storage.astral_backend import AstralBackend

logger = logging.getLogger(__name__)


class LRUBackendCache:
    """Thread-safe LRU cache for up to `maxsize` AstralBackend instances."""

    def __init__(self, maxsize: int = 5) -> None:
        self._cache: OrderedDict = OrderedDict()  # slug → AstralBackend
        self._maxsize = maxsize
        self._lock = Lock()

    def get_or_load(self, slug: str) -> "AstralBackend | None":
        """Return cached backend for slug, loading from central path if needed.

        Returns None if no central DB exists for the slug.
        Uses double-checked locking: load I/O happens outside the lock.
        """
        from axon.core.storage.astral_backend import AstralBackend

        # Fast path: already cached
        with self._lock:
            if slug in self._cache:
                self._cache.move_to_end(slug)
                return self._cache[slug]

        # Slow path: load from disk (outside lock to avoid blocking other requests)
        db_path = central_db_path(slug)
        if not db_path.exists():
            logger.debug("No central DB for slug '%s' at %s", slug, db_path)
            return None

        backend = AstralBackend()
        try:
            backend.initialize(db_path, read_only=True)
        except RuntimeError as exc:
            logger.warning("Failed to open DB for '%s': %s", slug, exc)
            return None

        with self._lock:
            if slug in self._cache:
                # Another thread loaded it while we were loading — discard duplicate
                backend.close()
                self._cache.move_to_end(slug)
                return self._cache[slug]
            # Evict LRU entries until under capacity
            while len(self._cache) >= self._maxsize:
                evicted_slug, evicted = self._cache.popitem(last=False)
                logger.info("LRU evict '%s' from backend cache", evicted_slug)
                evicted.close()
            self._cache[slug] = backend
            logger.info(
                "Loaded backend for '%s' (cache: %d/%d)", slug, len(self._cache), self._maxsize
            )
            return backend

    def status(self) -> dict:
        """Return a dict with cached slug list and capacity."""
        with self._lock:
            return {
                "cached": list(self._cache.keys()),
                "count": len(self._cache),
                "maxsize": self._maxsize,
            }

    def evict(self, slug: str) -> bool:
        """Close and remove a single slug from the cache. Returns True if it was cached."""
        with self._lock:
            if slug not in self._cache:
                return False
            backend = self._cache.pop(slug)
        try:
            backend.close()
        except Exception:  # noqa: BLE001
            pass
        logger.info("Evicted '%s' from backend cache (analyze requested)", slug)
        return True

    def close_all(self) -> None:
        """Close all cached backends and clear the cache. Call on daemon shutdown."""
        with self._lock:
            for backend in self._cache.values():
                try:
                    backend.close()
                except Exception:  # noqa: BLE001
                    pass
            self._cache.clear()
