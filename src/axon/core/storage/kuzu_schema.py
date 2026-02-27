"""Schema creation and FTS index setup for KuzuDB.

Contains module-level functions that receive a ``kuzu.Connection`` and
create/manage the database schema.  These are called by
:class:`~axon.core.storage.kuzu_backend.KuzuBackend` during
``initialize()``.
"""

from __future__ import annotations

import logging

import kuzu

from axon.core.storage.kuzu_backend import (
    _EMBEDDING_PROPERTIES,
    _NODE_PROPERTIES,
    _NODE_TABLE_NAMES,
    _REL_PROPERTIES,
)

logger = logging.getLogger(__name__)


def create_schema(conn: kuzu.Connection) -> None:
    """Create node/rel/embedding tables and the FTS extension."""
    try:
        conn.execute("INSTALL fts")
        conn.execute("LOAD EXTENSION fts")
    except RuntimeError:
        logger.debug("FTS extension load skipped (may already be loaded)", exc_info=True)

    for table in _NODE_TABLE_NAMES:
        stmt = f"CREATE NODE TABLE IF NOT EXISTS {table}({_NODE_PROPERTIES})"
        conn.execute(stmt)

    conn.execute(
        f"CREATE NODE TABLE IF NOT EXISTS Embedding({_EMBEDDING_PROPERTIES})"
    )

    # Build the REL TABLE GROUP covering all table-to-table combinations.
    from_to_pairs: list[str] = []
    for src in _NODE_TABLE_NAMES:
        for dst in _NODE_TABLE_NAMES:
            from_to_pairs.append(f"FROM {src} TO {dst}")

    pairs_clause = ", ".join(from_to_pairs)
    rel_stmt = (
        f"CREATE REL TABLE GROUP IF NOT EXISTS CodeRelation("
        f"{pairs_clause}, {_REL_PROPERTIES})"
    )
    try:
        conn.execute(rel_stmt)
    except RuntimeError:
        logger.debug("REL TABLE GROUP creation skipped", exc_info=True)

    create_fts_indexes(conn)


def create_fts_indexes(conn: kuzu.Connection) -> None:
    """Create FTS indexes for every node table (idempotent)."""
    for table in _NODE_TABLE_NAMES:
        idx_name = f"{table.lower()}_fts"
        try:
            conn.execute(
                f"CALL CREATE_FTS_INDEX('{table}', '{idx_name}', "
                f"['name', 'content', 'signature'])"
            )
        except RuntimeError:
            # Index may already exist — that's fine.
            pass
