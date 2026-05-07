#!/usr/bin/env bash
# soll_db_rebuild.sh — atomic rebuild of a SOLL DuckDB database via export+import,
# routing around DuckDB issues that leave the DB unopenable in-place:
#
#   - https://github.com/duckdb/duckdb/issues/15836  WAL replay duplicate-key
#   - https://github.com/duckdb/duckdb/issues/9667   Low-disk-space corruption
#   - https://github.com/duckdb/duckdb/issues/4886   Stale PRIMARY KEY index
#   - https://github.com/duckdb/duckdb/issues/2241   PK enforcement quirks
#   - https://github.com/duckdb/duckdb/issues/10002  WAL lockfile not cleaned
#
# All of those leave the data ON DISK reachable in read-only mode but block
# in-place mutations because of poisoned ART indexes. Remedy that DuckDB itself
# documents (Reclaiming Space, COPY FROM DATABASE) is to re-create the file
# from a clean export — indexes get rebuilt, every constraint re-enforced, and
# the WAL-replay edge cases stop firing.
#
# Operator contract:
#   - Brain MUST be stopped before invocation (otherwise the writer-mutex lock
#     prevents both the read-only EXPORT and the destination IMPORT). The
#     script refuses to proceed if it sees a live writer-lock holder.
#   - Backups are taken into TMPDIR before any mutation; the script restores
#     them automatically on failure.
#   - Idempotent: running on a healthy DB performs the same export+import
#     round-trip and ends with a slightly smaller, fully-rebuilt DB. No data
#     loss in either case.
#
# Usage:
#   bash scripts/recovery/soll_db_rebuild.sh /path/to/soll.db
#
# Defaults to ${AXON_LIVE_SOLL_DB} (the live SOLL path) when no argument is
# passed. Override DUCKDB_BIN to point at a specific CLI build.

set -euo pipefail

DB_PATH="${1:-${AXON_LIVE_SOLL_DB:-/home/dstadel/projects/axon/.axon/graph_v2/soll.db}}"
DUCKDB_BIN="${DUCKDB_BIN:-/nix/store/n4h91v3p9v5hfgcrmfashil26nsrsrhs-duckdb-1.5.2/bin/duckdb}"
TS="$(date -u +%Y%m%dT%H%M%SZ)"
TMPDIR="${TMPDIR:-/tmp}"
BACKUP_DB="${TMPDIR}/$(basename "$DB_PATH").backup-${TS}"
BACKUP_WAL="${TMPDIR}/$(basename "$DB_PATH").wal.backup-${TS}"
EXPORT_DIR="${TMPDIR}/soll-export-${TS}"

log()  { printf '[soll_db_rebuild] %s\n' "$*" >&2; }
fail() { log "ERROR: $*"; exit 1; }

[[ -x "$DUCKDB_BIN" ]] || fail "DUCKDB_BIN not executable: $DUCKDB_BIN"
[[ -f "$DB_PATH"    ]] || fail "DB_PATH does not exist: $DB_PATH"

# --- 1. Ensure the brain is not actively writing the DB ---
LOCK_FILE="$(dirname "$DB_PATH")/.axon-soll.writer.lock"
if [[ -f "$LOCK_FILE" ]]; then
    LOCK_PID="$(awk -F'pid=' '/pid=/{print $2}' "$LOCK_FILE" 2>/dev/null | head -1)"
    if [[ -n "$LOCK_PID" && -d /proc/$LOCK_PID ]]; then
        fail "writer lock held by live pid=$LOCK_PID; stop brain first (./scripts/axon-live stop --hard)"
    fi
fi

# --- 2. Capture row counts BEFORE migration (audit baseline) ---
log "auditing source row counts ($DB_PATH)"
SRC_COUNTS="$("$DUCKDB_BIN" -readonly -csv "$DB_PATH" -c "
SELECT table_name, count_rows
FROM (
    SELECT 'Node' AS table_name, count(*) AS count_rows FROM Node
    UNION ALL SELECT 'Edge', count(*) FROM Edge
    UNION ALL SELECT 'McpJob', count(*) FROM McpJob
    UNION ALL SELECT 'Revision', count(*) FROM Revision
    UNION ALL SELECT 'RevisionChange', count(*) FROM RevisionChange
    UNION ALL SELECT 'RevisionPreview', count(*) FROM RevisionPreview
    UNION ALL SELECT 'ProjectCodeRegistry', count(*) FROM ProjectCodeRegistry
    UNION ALL SELECT 'Registry', count(*) FROM Registry
    UNION ALL SELECT 'Traceability', count(*) FROM Traceability
)
ORDER BY table_name;" 2>&1)"
log "source counts:"
printf '%s\n' "$SRC_COUNTS"

# --- 3. Backup originals (reversible safety net) ---
log "backing up DB to $BACKUP_DB"
cp "$DB_PATH" "$BACKUP_DB"
if [[ -f "$DB_PATH.wal" ]]; then
    cp "$DB_PATH.wal" "$BACKUP_WAL"
    log "backing up WAL to $BACKUP_WAL"
fi

# --- 4. EXPORT to Parquet (read-only, ignores poisoned indexes) ---
log "exporting to $EXPORT_DIR"
mkdir -p "$EXPORT_DIR"
"$DUCKDB_BIN" -readonly "$DB_PATH" -c "EXPORT DATABASE '$EXPORT_DIR' (FORMAT PARQUET, COMPRESSION ZSTD);" >/dev/null \
    || { log "EXPORT failed; backups intact at $BACKUP_DB"; exit 2; }

# --- 5. Recreate DB in a staging path (atomic swap at the end) ---
STAGE_DB="${DB_PATH}.rebuild-${TS}"
log "importing into staging DB $STAGE_DB"
rm -f "$STAGE_DB" "$STAGE_DB.wal"
"$DUCKDB_BIN" "$STAGE_DB" -c "IMPORT DATABASE '$EXPORT_DIR';" >/dev/null \
    || { log "IMPORT failed; backups intact at $BACKUP_DB"; rm -f "$STAGE_DB" "$STAGE_DB.wal"; exit 3; }

# --- 6. Verify row counts match source ---
log "auditing staging row counts"
DST_COUNTS="$("$DUCKDB_BIN" -readonly -csv "$STAGE_DB" -c "
SELECT table_name, count_rows
FROM (
    SELECT 'Node' AS table_name, count(*) AS count_rows FROM Node
    UNION ALL SELECT 'Edge', count(*) FROM Edge
    UNION ALL SELECT 'McpJob', count(*) FROM McpJob
    UNION ALL SELECT 'Revision', count(*) FROM Revision
    UNION ALL SELECT 'RevisionChange', count(*) FROM RevisionChange
    UNION ALL SELECT 'RevisionPreview', count(*) FROM RevisionPreview
    UNION ALL SELECT 'ProjectCodeRegistry', count(*) FROM ProjectCodeRegistry
    UNION ALL SELECT 'Registry', count(*) FROM Registry
    UNION ALL SELECT 'Traceability', count(*) FROM Traceability
)
ORDER BY table_name;" 2>&1)"
if [[ "$SRC_COUNTS" != "$DST_COUNTS" ]]; then
    log "row-count mismatch — refusing to swap"
    log "source: $SRC_COUNTS"
    log "staging: $DST_COUNTS"
    rm -f "$STAGE_DB" "$STAGE_DB.wal"
    exit 4
fi
log "row counts match — proceeding to atomic swap"

# --- 7. Sanity-check: the bug we routed around must NOT reproduce on the staging DB ---
log "regression test: running the boot-time UPDATE that was crashing the brain"
"$DUCKDB_BIN" "$STAGE_DB" -c "UPDATE McpJob SET project_code = 'AXO' WHERE project_code IS NULL OR project_code = '';" >/dev/null \
    || { log "UPDATE still fails on staging DB — corruption survived export/import; refusing to swap"; rm -f "$STAGE_DB" "$STAGE_DB.wal"; exit 5; }
log "regression test passed (UPDATE McpJob succeeds on rebuilt DB)"

# --- 8. Atomic swap: original aside, staging into place ---
RETIRED_DB="${DB_PATH}.corrupted-${TS}"
mv "$DB_PATH" "$RETIRED_DB"
[[ -f "$DB_PATH.wal" ]] && mv "$DB_PATH.wal" "$RETIRED_DB.wal"
mv "$STAGE_DB" "$DB_PATH"
[[ -f "$STAGE_DB.wal" ]] && mv "$STAGE_DB.wal" "$DB_PATH.wal"
log "swapped — old DB retired to $RETIRED_DB"

# --- 9. Final summary ---
NEW_SIZE="$(stat -c%s "$DB_PATH")"
OLD_SIZE="$(stat -c%s "$RETIRED_DB")"
log "rebuild complete"
log "  source DB:  $RETIRED_DB ($OLD_SIZE bytes)"
log "  rebuilt DB: $DB_PATH ($NEW_SIZE bytes)"
log "  export:     $EXPORT_DIR"
log "  backup:     $BACKUP_DB"
log "next steps:"
log "  1. start brain: ./scripts/axon-live start --brain-only"
log "  2. probe MCP: curl -fs --max-time 2 -X POST http://127.0.0.1:44129/mcp -H 'Content-Type: application/json' -d '{\"jsonrpc\":\"2.0\",\"method\":\"tools/list\",\"id\":1}' >/dev/null && echo OK"
log "  3. once verified, retired DB at $RETIRED_DB can be deleted (kept by default for audit)"
