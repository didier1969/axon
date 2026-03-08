"""Storage backends for the Axon knowledge graph."""

from axon.core.storage.base import StorageBackend, SearchResult, NodeEmbedding
from axon.core.storage.astral_backend import AstralBackend

def get_storage_backend(mode: str = "astral", **kwargs) -> StorageBackend:
    """Factory for storage backends. v1.0 defaults to Astral (HydraDB)."""
    return AstralBackend(**kwargs)

__all__ = ["StorageBackend", "SearchResult", "NodeEmbedding", "AstralBackend", "get_storage_backend"]
