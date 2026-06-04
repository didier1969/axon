#!/usr/bin/env bash
# Wave 5 runner — sources canonical live runtime config and adds writer-path
# overrides (async writer + bulk writer + parquet store) before invoking the
# end-to-end bench harness against the AXO source tree.
#
# Idempotent: stops dev cleanly first; bench-end-to-end.sh handles fresh start.
#
# Usage: scripts/dev/_w5_runner.sh <scope-path> <duration-secs> <label>

set -euo pipefail

# REQ-AXO-901640 — derive defaults from $ROOT, not the maintainer's machine.
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

SCOPE="${1:-$ROOT/src}"
DURATION="${2:-90}"
LABEL="${3:-pg-async-bulk-w5}"

# Pick up persistent live runtime config (DB_BACKEND + DATABASE_URL + SOLL_SEED + AGE_ONLY).
set -a
# shellcheck disable=SC1091
source "$ROOT/.axon/runtime-config.live.env"
set +a

# probe.sh --postgres requires AXON_LIVE_DATABASE_URL or AXON_DEV_DATABASE_URL exported.
export AXON_DEV_DATABASE_URL="${AXON_DEV_DATABASE_URL:-${AXON_LIVE_DATABASE_URL}}"

# Wave 5 overrides — write-path optimisations for AC4 of REQ-AXO-252.
export AXON_BULK_WRITER_ENABLED=true
export AXON_PARQUET_EMBEDDING_STORE_ENABLED=true
# REQ-AXO-205 / MIL-AXO-015 indexer-gate: bypass the PG opt-in smoke gate
# (writer hot path PG-clean, B.2 AGE dual-write live for relations).
export AXON_INDEXER_PG_OPT_IN=1

echo "==[ Wave 5 env ]=="
echo "  AXON_INDEXER_PG_OPT_IN          = ${AXON_INDEXER_PG_OPT_IN}"
echo "  AXON_BULK_WRITER_ENABLED        = ${AXON_BULK_WRITER_ENABLED}"
echo "  AXON_PARQUET_EMBEDDING_STORE_ENABLED = ${AXON_PARQUET_EMBEDDING_STORE_ENABLED}"
echo "  AXON_DEV_DATABASE_URL           = (set)"
echo "  scope=$SCOPE duration=${DURATION}s label=$LABEL"
echo "===================="

exec bash scripts/dev/bench-end-to-end.sh --run --scope "$SCOPE" \
  --duration "$DURATION" --postgres --label "$LABEL"
