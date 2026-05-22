#!/usr/bin/env bash
# VAL-AXO-038 probe driver: defends VAL-AXO-037 76.9 ch/s claim with N fresh runs.
# Each run: stop dev → wipe IST → start with full Parquet+TensorRT env → sample 90s → stop.
# Output: dev-probe-val038-runN-<UTC>.csv per run + summary line.
#
# Usage:
#   scripts/dev/probe_val38.sh <tag> [duration_sec=90] [interval_sec=10]
# Env override:
#   AXON_DIAG_SKIP_CHUNK_CONTENT=true   # for cheap-diagnostic run

set -euo pipefail

TAG="${1:?tag required, e.g. val38-run1 or val38-diag}"
DURATION="${2:-90}"
INTERVAL="${3:-10}"

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

# Stop dev cleanly (idempotent)
./scripts/axon-dev stop --hard >/dev/null 2>&1 || true
sleep 2

# Wipe IST (fresh DB per run; mirrors VAL-AXO-037 conditions)
rm -rf .axon-dev/graph_v2

TS="$(date -u +%Y%m%dT%H%M%SZ)"
OUT="dev-probe-${TAG}-${TS}.csv"
HB_PATH=".axon-dev/run-indexer/runtime-heartbeat.json"

echo "🚀 [$TAG] Starting dev (tensorrt) at $TS"
env \
  AXON_ASYNC_WRITER_ENABLED="${AXON_ASYNC_WRITER_ENABLED:-false}" \
  AXON_DIAG_SKIP_CHUNK_CONTENT="${AXON_DIAG_SKIP_CHUNK_CONTENT:-false}" \
  AXON_GRAPH_EMBEDDINGS_ENABLED=false \
  AXON_BACKGROUND_BUDGET_CLASS=aggressive \
  AXON_RESOURCE_PRIORITY=critical \
  AXON_WATCHER_POLICY=full \
  AXON_OPT_MAX_VRAM_USED_MB=7500 \
  AXON_CUDA_MEMORY_SOFT_LIMIT_MB=7500 \
  AXON_CUDA_MEMORY_LIMIT_MB=4096 \
  AXON_GPU_PRIMARY_WORKER_MAX_USED_MB=6500 \
  AXON_WATCH_DIR="$ROOT" \
  AXON_PROJECTS_ROOT="$ROOT" \
  ./scripts/axon-dev start --indexer-full --tensorrt \
    > "/tmp/probe-${TAG}-start.log" 2>&1

# Wait up to 60s for heartbeat
for _ in $(seq 1 60); do
    [[ -f "$HB_PATH" ]] && break
    sleep 1
done
if [[ ! -f "$HB_PATH" ]]; then
    echo "❌ [$TAG] heartbeat never appeared at $HB_PATH" >&2
    tail -30 "/tmp/probe-${TAG}-start.log" >&2 || true
    ./scripts/axon-dev stop --hard >/dev/null 2>&1 || true
    exit 1
fi

echo "ts,vector_chunks_total,inflight,queued,gp_queued,delta_chunks,delta_seconds,chunks_per_sec" > "$OUT"

START_EPOCH="$(date +%s)"
PREV_TOTAL=0
PREV_T=0
while :; do
    NOW="$(date +%s)"
    T=$((NOW - START_EPOCH))
    [[ "$T" -ge "$DURATION" ]] && break

    READ="$(python3 -c '
import json, sys
try:
    d = json.load(open(sys.argv[1]))
    rtt = d.get("runtime_telemetry", {})
    fvq = rtt.get("file_vectorization_queue", {})
    gp  = rtt.get("graph_projection", {}) or rtt.get("graph_projection_queue", {})
    print(",".join([
        str(rtt.get("vector_chunks_embedded_total", 0)),
        str(fvq.get("inflight", 0)),
        str(fvq.get("queued", 0)),
        str(gp.get("queued", 0)),
    ]))
except Exception as e:
    print("0,0,0,0")
' "$HB_PATH" 2>/dev/null || echo "0,0,0,0")"

    IFS=',' read -r CHUNKS INFLIGHT QUEUED GPQ <<< "$READ"
    DELTA_C=$((CHUNKS - PREV_TOTAL))
    DELTA_S=$((T - PREV_T))
    if [[ "$DELTA_S" -gt 0 ]]; then
        RATE="$(python3 -c "print(f'{$DELTA_C / $DELTA_S:.2f}')")"
    else
        RATE="0.00"
    fi
    ISO="$(date -u -Iseconds | sed 's/+00:00/Z/')"
    echo "$ISO,$CHUNKS,$INFLIGHT,$QUEUED,$GPQ,$DELTA_C,$DELTA_S,$RATE" >> "$OUT"
    PREV_TOTAL="$CHUNKS"
    PREV_T="$T"
    sleep "$INTERVAL"
done

# Final mean: total chunks / duration (last sample)
LAST_TOTAL="$(tail -1 "$OUT" | awk -F',' '{print $2}')"
MEAN_RATE="$(python3 -c "print(f'{$LAST_TOTAL / $DURATION:.2f}')")"

# Capture provider effectiveness for sanity
PROVIDER="$(python3 -c '
import json, sys
try:
    d = json.load(open(sys.argv[1]))
    print(d.get("embedder_provider", {}).get("effective", "unknown"))
except Exception:
    print("err")
' "$HB_PATH" 2>/dev/null || echo "err")"

./scripts/axon-dev stop --hard >/dev/null 2>&1 || true

echo "📊 [$TAG] csv=$OUT total_chunks=$LAST_TOTAL mean_chs=$MEAN_RATE provider=$PROVIDER duration=${DURATION}s"
