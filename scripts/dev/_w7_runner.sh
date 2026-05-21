#!/usr/bin/env bash
# Wave 7 runner — Wave 5 envs (PG + async + bulk + parquet) PLUS forces
# AXON_GRAPH_WORKERS=1 to bypass the autoconfig that produces graph_workers=0
# under the default VRAM budget. Used to empirically validate REQ-AXO-269 v1
# (graph projection lane short-circuit drain) — see VAL-AXO-061 for context.
#
# Idempotent: stops dev cleanly first; bench-end-to-end.sh handles fresh start.
#
# Usage: scripts/dev/_w7_runner.sh <scope-path> <duration-secs> <label>

set -euo pipefail

# REQ-AXO-901640 — derive defaults from $ROOT, not the maintainer's machine.
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

SCOPE="${1:-$ROOT/src}"
DURATION="${2:-90}"
LABEL="${3:-pg-graph-workers-1-w7}"

# Pick up persistent live runtime config (DB_BACKEND + DATABASE_URL + SOLL_SEED + AGE_ONLY).
set -a
# shellcheck disable=SC1091
source "$ROOT/.axon/runtime-config.live.env"
set +a

# probe.sh --postgres requires AXON_LIVE_DATABASE_URL or AXON_DEV_DATABASE_URL exported.
export AXON_DEV_DATABASE_URL="${AXON_DEV_DATABASE_URL:-${AXON_LIVE_DATABASE_URL}}"

# Wave 5 baseline overrides
export AXON_ASYNC_WRITER_ENABLED=true
export AXON_BULK_WRITER_ENABLED=true
export AXON_PARQUET_EMBEDDING_STORE_ENABLED=true
export AXON_INDEXER_PG_OPT_IN=1

# Wave 7 specific: force the graph projection worker to run despite the
# default tight VRAM budget. The autoconfig at scripts/start.sh would
# otherwise leave graph_workers=0 and our REQ-269 v1 short-circuit
# would never execute. Setting AXON_GRAPH_WORKERS=1 + a bigger soft VRAM
# limit lets a single graph worker spawn alongside the vector worker.
export AXON_GRAPH_WORKERS=1
export AXON_OPT_MAX_VRAM_USED_MB="${AXON_OPT_MAX_VRAM_USED_MB:-7500}"
export AXON_GPU_PRIMARY_WORKER_MAX_USED_MB="${AXON_GPU_PRIMARY_WORKER_MAX_USED_MB:-6500}"
export AXON_CUDA_MEMORY_SOFT_LIMIT_MB="${AXON_CUDA_MEMORY_SOFT_LIMIT_MB:-7500}"

echo "==[ Wave 7 env ]=="
echo "  AXON_DB_BACKEND                 = ${AXON_DB_BACKEND:-<unset>}"
echo "  AXON_INDEXER_PG_OPT_IN          = ${AXON_INDEXER_PG_OPT_IN}"
echo "  AXON_ASYNC_WRITER_ENABLED       = ${AXON_ASYNC_WRITER_ENABLED}"
echo "  AXON_BULK_WRITER_ENABLED        = ${AXON_BULK_WRITER_ENABLED}"
echo "  AXON_PARQUET_EMBEDDING_STORE_ENABLED = ${AXON_PARQUET_EMBEDDING_STORE_ENABLED}"
echo "  AXON_AGE_ONLY_RELATIONS         = ${AXON_AGE_ONLY_RELATIONS:-<unset>}"
echo "  AXON_GRAPH_WORKERS              = ${AXON_GRAPH_WORKERS}  ← force >0"
echo "  AXON_OPT_MAX_VRAM_USED_MB       = ${AXON_OPT_MAX_VRAM_USED_MB}"
echo "  AXON_GPU_PRIMARY_WORKER_MAX_USED_MB = ${AXON_GPU_PRIMARY_WORKER_MAX_USED_MB}"
echo "  scope=$SCOPE duration=${DURATION}s label=$LABEL"
echo "===================="

exec bash scripts/dev/bench-end-to-end.sh --run --scope "$SCOPE" \
  --duration "$DURATION" --postgres --label "$LABEL"
