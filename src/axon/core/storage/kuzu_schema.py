"""Schema creation and FTS index setup for KuzuDB.

Contains module-level functions that receive a ``kuzu.Connection`` and
create/manage the database schema.  These are called by
:class:`~axon.core.storage.kuzu_backend.KuzuBackend` during
``initialize()``.
"""

from __future__ import annotations

import logging

import kuzu

from axon.core.storage.kuzu_constants import (
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

    # Force drop of CodeRelation to ensure schema update
    try:
        conn.execute("DROP TABLE CodeRelation")
    except RuntimeError:
        pass

    logger.info("Creating node tables: %s", _NODE_TABLE_NAMES)
    for table in _NODE_TABLE_NAMES:
        stmt = f"CREATE NODE TABLE IF NOT EXISTS {table}({_NODE_PROPERTIES})"
        try:
            conn.execute(stmt)
        except RuntimeError as e:
            logger.error("Failed to create node table %s: %s", table, e)
            raise

    conn.execute(
        f"CREATE NODE TABLE IF NOT EXISTS Embedding({_EMBEDDING_PROPERTIES})"
    )

    # Simplified: One REL TABLE for all relations instead of a complex GROUP.
    # This avoids the "Table does not exist" errors for internal group tables.
    from_to_pairs: list[str] = []
    for src in _NODE_TABLE_NAMES:
        for dst in _NODE_TABLE_NAMES:
            from_to_pairs.append(f"FROM {src} TO {dst}")

    pairs_clause = ", ".join(from_to_pairs)
    rel_stmt = (
        f"CREATE REL TABLE CodeRelation("
        f"{pairs_clause}, {_REL_PROPERTIES})"
    )
    try:
        conn.execute(rel_stmt)
    except RuntimeError as e:
        logger.error("Critical: Failed to create CodeRelation table: %s", e)
        raise

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
