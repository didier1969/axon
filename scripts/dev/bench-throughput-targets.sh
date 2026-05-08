#!/usr/bin/env bash
# REQ-AXO-221 / REQ-AXO-222 — Throughput target benchmark with env-var
# opt-ins isolated and cumulative.
#
# ⚠️ Scope vs existing campaign infra:
# - `scripts/benchmark-vector-token-matrix.sh` + `scripts/benchmark_vector_campaign.py`
#   already exist and run an exhaustive matrix sweeping `AXON_EMBED_MICRO_BATCH_MAX_TOTAL_TOKENS`,
#   `AXON_EMBED_MICRO_BATCH_MAX_ITEMS`, `AXON_MAX_EMBED_BATCH_BYTES`,
#   `AXON_OPT_MAX_VRAM_USED_MB` etc. Use that for parameter EXPLORATION.
# - This script is purpose-built for the REQ-AXO-221 acceptance test:
#   prove that the 4 priority env-vars from the expert report
#   collectively reach the operator target (120 ch/s) without code
#   refactor. It overlaps with the campaign on `AXON_MAX_EMBED_BATCH_BYTES`
#   (1/5 vars) but adds CHUNK_BATCH_SIZE, CUDA_ALLOW_TF32,
#   VECTOR_PERSIST_QUEUE_BOUND, PARQUET_EMBEDDING_STORE — none of which
#   the campaign sweeps. Use this for the SPECIFIC acceptance test.
#
# Builds on `probe.sh` to sweep the 4 priority env-vars from the
# 2026-05-08 expert report (`docs/working-notes/2026-05-08-expert-report-embedding-performance.md`):
#   - AXON_CHUNK_BATCH_SIZE        (16 → 64, P2)
#   - AXON_MAX_EMBED_BATCH_BYTES   (4 MB → 16 MB, P2)
#   - AXON_CUDA_ALLOW_TF32         (off → on, P3)
#   - AXON_VECTOR_PERSIST_QUEUE_BOUND (4 → 64, P4)
#   - AXON_PARQUET_EMBEDDING_STORE_ENABLED (P4)
#
# Acceptance: identifies the smallest config delta that crosses the
# operator throughput target (default 120 chunks/s end-to-end). Fails
# if no config reaches the target after the cumulative run.
#
# Usage:
#   scripts/dev/bench-throughput-targets.sh \
#       [--scope PATH] \
#       [--duration SEC] \
#       [--target CH_PER_SEC] \
#       [--out FILE]
#
# Cycle target: ~10-15 min total (7 cells × 90s + indexer warmup).

set -euo pipefail

SCOPE=""
DURATION=90
TARGET_CHPS=120
OUT=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --scope) SCOPE="$2"; shift 2 ;;
        --duration) DURATION="$2"; shift 2 ;;
        --target) TARGET_CHPS="$2"; shift 2 ;;
        --out) OUT="$2"; shift 2 ;;
        -h|--help)
            grep '^#' "$0" | sed 's/^# \{0,1\}//' | head -28
            exit 0
            ;;
        *) echo "❌ unknown arg: $1" >&2; exit 2 ;;
    esac
done

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

if [[ -z "$SCOPE" ]]; then
    SCOPE="$ROOT"
fi
if [[ -z "$OUT" ]]; then
    OUT="dev-bench-throughput-$(date -u +%Y%m%dT%H%M%SZ).csv"
fi

PROBE="scripts/dev/probe.sh"
if [[ ! -x "$PROBE" ]]; then
    echo "❌ missing $PROBE" >&2; exit 2
fi

echo "config,chunk_batch,max_bytes,tf32,persist_queue,parquet,chunks_per_sec,target_met" > "$OUT"

run_cell() {
    local label="$1"
    local cb="$2"; local mb="$3"; local tf="$4"; local pq="$5"; local pq_store="$6"

    echo "=== Cell: $label ==="
    echo "  CHUNK_BATCH=$cb MAX_BYTES=$mb TF32=$tf PERSIST_Q=$pq PARQUET=$pq_store"

    # Reset + apply env
    unset AXON_CHUNK_BATCH_SIZE AXON_MAX_EMBED_BATCH_BYTES \
          AXON_CUDA_ALLOW_TF32 AXON_VECTOR_PERSIST_QUEUE_BOUND \
          AXON_PARQUET_EMBEDDING_STORE_ENABLED
    [[ "$cb" != "default" ]] && export AXON_CHUNK_BATCH_SIZE="$cb"
    [[ "$mb" != "default" ]] && export AXON_MAX_EMBED_BATCH_BYTES="$mb"
    [[ "$tf" == "1" ]] && export AXON_CUDA_ALLOW_TF32=1
    [[ "$pq" != "default" ]] && export AXON_VECTOR_PERSIST_QUEUE_BOUND="$pq"
    [[ "$pq_store" == "1" ]] && export AXON_PARQUET_EMBEDDING_STORE_ENABLED=true

    "$PROBE" --scope "$SCOPE" --duration "$DURATION" --fresh \
             --tag "throughput-$label" 2>&1 | tail -8

    # Extract steady-state chunks_per_sec from probe CSV.
    # CSV columns: ts, vector_chunks_total, inflight, queued, gp_queued,
    #              delta_chunks, delta_seconds, chunks_per_sec
    # Strategy: skip warmup zeros, average non-zero chunks_per_sec
    # across all post-warmup samples for noise reduction.
    # CSV column 4 is chunks_per_sec; column 8 is zombies (always 0).
    # An earlier commit (ddc2a97) regressed this from 4 to 8 — fixed back.
    local csv
    csv=$(ls -t dev-probe-throughput-${label}-*.csv 2>/dev/null | head -1)
    local chps="0"
    if [[ -n "$csv" ]] && [[ -f "$csv" ]]; then
        chps=$(awk -F',' '
            NR>1 && $4!="" && $4>0 { sum += $4; n += 1 }
            END { if (n > 0) printf "%.2f", sum / n; else print "0" }
        ' "$csv")
    fi
    chps="${chps:-0}"

    local met="no"
    if (( $(echo "$chps >= $TARGET_CHPS" | bc -l 2>/dev/null || echo 0) )); then
        met="yes"
    fi

    echo "$label,$cb,$mb,$tf,$pq,$pq_store,$chps,$met" >> "$OUT"
    echo "  → chunks/sec: $chps (target $TARGET_CHPS: $met)"
    echo
}

# --- Test grid (each row is additive on top of the previous one) ---

# Baseline: defaults
run_cell baseline           default       default          0  default  0
# P2.a: chunk batch 64
run_cell p2a-batch64        64            default          0  default  0
# P2.b: + max bytes 16 MB
run_cell p2b-bytes16mb      64            $((16*1024*1024)) 0  default  0
# P3: + TF32
run_cell p3-tf32            64            $((16*1024*1024)) 1  default  0
# P4.a: + persist queue 64
run_cell p4a-persistq64     64            $((16*1024*1024)) 1  64       0
# P4.b: + parquet store
run_cell p4b-parquet        64            $((16*1024*1024)) 1  64       1
# Cumulative target check
run_cell cumulative-target  64            $((16*1024*1024)) 1  64       1

echo "=== Summary ==="
column -t -s, "$OUT"
echo
echo "CSV: $OUT"

# Exit code: 0 if cumulative cell met target, 1 otherwise.
last_met=$(tail -1 "$OUT" | awk -F',' '{print $NF}')
if [[ "$last_met" == "yes" ]]; then
    echo "✅ Target $TARGET_CHPS ch/s reached on cumulative config"
    exit 0
else
    echo "❌ Target $TARGET_CHPS ch/s NOT reached. Next steps:"
    echo "   - REQ-AXO-223 pipeline 3-stages refactor (×8-12 expected)"
    echo "   - REQ-AXO-224 NVIDIA MPS (×1.5 if 2 workers fit 8GB)"
    echo "   - REQ-AXO-225 INT8 quantization (×1.5-2)"
    exit 1
fi
