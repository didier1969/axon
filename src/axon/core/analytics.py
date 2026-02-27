"""Usage analytics — fire-and-forget event logging for MCP queries and index runs.

Events are appended as JSON lines to ``~/.axon/events.jsonl``.  All errors
are silently swallowed so that a logging failure never affects callers.
"""

from __future__ import annotations

import json
import logging
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

logger = logging.getLogger(__name__)


def log_event(type: str, **kwargs: Any) -> None:  # noqa: A002
    """Append a JSON event line to ``~/.axon/events.jsonl``.

    Parameters
    ----------
    type:
        Event category — ``"query"``, ``"context"``, ``"impact"``, or ``"index"``.
    **kwargs:
        Arbitrary key/value fields included in the event record.

    Never raises.  Any I/O or serialisation failure is logged at DEBUG level.
    """
    try:
        event: dict[str, Any] = {
            "ts": datetime.now(tz=timezone.utc).isoformat(),
            "type": type,
            **kwargs,
        }
        events_path = Path.home() / ".axon" / "events.jsonl"
        events_path.parent.mkdir(parents=True, exist_ok=True)
        with events_path.open("a", encoding="utf-8") as fh:
            fh.write(json.dumps(event) + "\n")
    except Exception as exc:  # noqa: BLE001
        logger.debug("Failed to log analytics event: %s", exc)
