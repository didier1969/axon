// REQ-AXO-901870 — brain_reader_only_refresh_opens_late_and_republished_ist_replica
// removed: it exercised the legacy split-brain DuckDB-file reader replica
// (split_brain_mode hardcoded false; the mechanism is dead under PG MVCC).
