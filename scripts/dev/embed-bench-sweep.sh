#!/usr/bin/env bash
# REQ-AXO-176 — L1 throughput parameter sweep.
#
# Loops `embed-bench.sh` over a parameter grid, accumulating CSV rows
# into a single output file for offline analysis. The first call for
# each unique TensorRT shape (driven by AXON_EMBED_MICRO_BATCH_MAX_ITEMS)
# pays the engine compile cost (~3-5 min), subsequent calls with the
# same shape hit the engine cache.
#
# Goal: identify (micro_batch, max_total_tokens) tuple that maximizes
# sustained chunks/s for a 1M+ chunk corpus, where compile cost is
# fully amortized.
#
# Usage:
#   scripts/dev/embed-bench-sweep.sh [--n N] [--out FILE]
#                                    [--micro-batches "32,64,128,256"]
#                                    [--max-tokens "8192,16384,32768"]
#
# Defaults: n=512, sweep 4×3 = 12 cells, output dev-bench-sweep-<UTC>.csv
# Estimated time: ~30-60 min for full grid (mostly TensorRT first-compiles)

set -euo pipefail

N=512
OUT=""
MICRO_BATCHES="32,64,128,256"
MAX_TOKENS="16384"   # default to single value to keep first sweep small
LABEL_PREFIX="sweep"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --n) N="$2"; shift 2 ;;
        --out) OUT="$2"; shift 2 ;;
        --micro-batches) MICRO_BATCHES="$2"; shift 2 ;;
        --max-tokens) MAX_TOKENS="$2"; shift 2 ;;
        --prefix) LABEL_PREFIX="$2"; shift 2 ;;
        -h|--help)
            sed -n '2,18p' "$0"
            exit 0
            ;;
        *) echo "unknown arg: $1" >&2; exit 2 ;;
    esac
done

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

if [[ -z "$OUT" ]]; then
    OUT="dev-bench-sweep-$(date -u +%Y%m%dT%H%M%SZ).csv"
fi

# Header — match embedder-bench --csv stderr line + add the swept params
echo "label,micro_batch,max_tokens,n,dim,load_ms,total_embed_ms,tokenize_ms,host_prepare_ms,input_copy_ms,inference_ms,output_extract_ms,chunks_per_sec" > "$OUT"

CELLS=0
IFS=',' read -ra MB_ARR <<< "$MICRO_BATCHES"
IFS=',' read -ra TOK_ARR <<< "$MAX_TOKENS"
TOTAL=$(( ${#MB_ARR[@]} * ${#TOK_ARR[@]} ))

for mb in "${MB_ARR[@]}"; do
    for tok in "${TOK_ARR[@]}"; do
        CELLS=$((CELLS + 1))
        LABEL="${LABEL_PREFIX}-mb${mb}-tok${tok}"
        echo "" >&2
        echo "═══ Cell ${CELLS}/${TOTAL}: ${LABEL} (n=${N}) ═══" >&2
        START="$(date +%s)"

        # Bigger token caps must accommodate the micro-batch capacity
        # (max_total_tokens >= micro_batch * max_seq_len). 512-tok max
        # sequence × N micro_batch = 512N. Clamp tok to >= 512*mb.
        EFF_TOK="$tok"
        MIN_TOK=$(( mb * 512 ))
        if (( EFF_TOK < MIN_TOK )); then
            EFF_TOK="$MIN_TOK"
        fi

        # Run the cell. Capture both stderr (for header echo) and stdout
        # (for the CSV row). embed-bench.sh writes only the data row to
        # stdout under --csv mode.
        ROW="$(AXON_EMBED_MICRO_BATCH_MAX_ITEMS="$mb" \
            AXON_EMBED_MICRO_BATCH_MAX_TOTAL_TOKENS="$EFF_TOK" \
            AXON_EMBED_BATCH_MAX_TOTAL_TOKENS="$EFF_TOK" \
            scripts/dev/embed-bench.sh --n "$N" --csv --label "$LABEL" 2>/dev/null \
            | tail -1 || echo "")"

        if [[ -z "$ROW" ]]; then
            echo "[FAIL] cell ${LABEL} produced no row" >&2
            continue
        fi

        # Prepend the swept params (mb, tok) before the embed-bench row.
        # embed-bench row format: label,n,dim,load_ms,total_embed_ms,tokenize_ms,...
        # We add: label,mb,tok,n,dim,...
        IFS=',' read -ra COLS <<< "$ROW"
        LABEL_COL="${COLS[0]}"
        REST="$(printf '%s,' "${COLS[@]:1}" | sed 's/,$//')"
        echo "${LABEL_COL},${mb},${EFF_TOK},${REST}" >> "$OUT"

        ELAPSED=$(( $(date +%s) - START ))
        echo "  → ${ELAPSED}s elapsed; row: ${LABEL_COL},mb=${mb},tok=${EFF_TOK} → ${COLS[*]: -1}" >&2
    done
done

echo "" >&2
echo "═══ Sweep complete: ${CELLS}/${TOTAL} cells → $OUT ═══" >&2

# Print sorted summary by chunks_per_sec (last column)
echo "" >&2
echo "Top 5 by chunks_per_sec:" >&2
tail -n +2 "$OUT" | sort -t, -k13 -gr | head -5 >&2 || true
