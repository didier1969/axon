#!/usr/bin/env bash
# REQ-AXO-261 — Bench 4 of the 4-bench diagnostic framework.
#
# Wraps probe.sh (REQ-AXO-175 — full indexer end-to-end ch/s sampler)
# and computes a unified summary line that matches Bench 1/2/3 CSV
# shapes, so cross-stage diagnosis is straightforward.
#
# Two modes:
#   1. Run a fresh probe + summarize:
#      bench-end-to-end.sh --run --scope <path> [--duration N] [--label L]
#   2. Summarize an existing probe CSV:
#      bench-end-to-end.sh --csv <existing-probe-csv> [--label L]
#
# Output:
#   - dev-bench-end-to-end-<UTC>.summary.csv : one-line summary
#   - stdout: human-readable summary
#
# Summary columns (cross-bench unified format):
#   label,bench,total_samples,duration_s,total_chunks,
#   mean_ch_per_s,min_ch_per_s,max_ch_per_s,p50_ch_per_s,p95_ch_per_s,final_files

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

usage() {
    cat <<'USAGE'
bench-end-to-end.sh — REQ-AXO-261 (Bench 4 of the 4-bench framework)

Usage:
  bench-end-to-end.sh --run --scope <path> [--duration N] [--label L]
  bench-end-to-end.sh --csv <existing-probe-csv> [--label L]

Run mode:
  --run                Run a fresh probe via scripts/dev/probe.sh
  --scope PATH         Watch dir for the probe (forwarded to probe.sh)
  --duration N         Probe duration in seconds (default 90)
  --label L            Run label (default 'end-to-end')

Summarize-only mode:
  --csv PATH           Existing probe CSV to summarize
  --label L            Run label

REQ-AXO-271 slice 6 (2026-05-10): PG is the only supported backend.
The --postgres flag is accepted as a no-op for backwards compatibility
and will be deleted once existing CI invocations stop passing it.

Output:
  - dev-bench-end-to-end-<UTC>.summary.csv
  - One-line stdout summary matching Bench 1/2/3 format
USAGE
}

MODE=""
SCOPE=""
DURATION=90
LABEL="end-to-end"
EXISTING_CSV=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --run) MODE="run"; shift ;;
        --csv) MODE="csv"; EXISTING_CSV="$2"; shift 2 ;;
        --scope) SCOPE="$2"; shift 2 ;;
        --duration) DURATION="$2"; shift 2 ;;
        --label) LABEL="$2"; shift 2 ;;
        --postgres) shift ;;  # REQ-AXO-271 slice 6: accepted as no-op.
        -h|--help) usage; exit 0 ;;
        *) echo "Unknown arg: $1" >&2; usage; exit 1 ;;
    esac
done

if [[ -z "$MODE" ]]; then
    echo "ERROR: must specify --run or --csv" >&2
    usage
    exit 1
fi

# When --run, invoke probe.sh and capture its CSV path.
if [[ "$MODE" == "run" ]]; then
    if [[ -z "$SCOPE" ]]; then
        echo "ERROR: --run requires --scope" >&2
        exit 1
    fi
    cd "$ROOT_DIR"
    TS_PROBE="$(date -u +%Y%m%dT%H%M%SZ)"
    bash "$SCRIPT_DIR/probe.sh" \
        --scope "$SCOPE" \
        --duration "$DURATION" \
        --tag "${LABEL}-${TS_PROBE}"
    EXISTING_CSV="$ROOT_DIR/dev-probe-${LABEL}-${TS_PROBE}-${TS_PROBE}.csv"
    # probe.sh names file dev-probe-<TAG>-<TS>.csv where TAG already
    # carries our timestamp; locate the most recent matching file.
    EXISTING_CSV="$(ls -1t "$ROOT_DIR"/dev-probe-${LABEL}-*.csv 2>/dev/null | head -1)"
    if [[ -z "$EXISTING_CSV" || ! -f "$EXISTING_CSV" ]]; then
        echo "ERROR: probe.sh did not produce expected CSV under $ROOT_DIR" >&2
        exit 1
    fi
fi

if [[ ! -f "$EXISTING_CSV" ]]; then
    echo "ERROR: CSV not found: $EXISTING_CSV" >&2
    exit 1
fi

# Compute summary via awk. Probe CSV columns:
#   t_seconds,files,chunks_total,chunks_per_sec,ready_queue,claim_mode,gpu_mb,zombies,provider
#
# Strategy: skip header + zero-rate rows (warmup), aggregate the rest.
# mean = sum(ch_per_s) / count
# min/max from ch_per_s
# p50/p95 require sorted samples — pipe through sort then index.
SAMPLES_SORTED="$(awk -F',' 'NR>1 && $4 != "" && $4+0 > 0 {print $4}' "$EXISTING_CSV" | sort -n)"
SAMPLES_RAW="$(awk -F',' 'NR>1 && $4 != "" && $4+0 > 0 {print $4}' "$EXISTING_CSV")"
TOTAL_SAMPLES="$(echo "$SAMPLES_SORTED" | grep -c . || true)"
TOTAL_SAMPLES="${TOTAL_SAMPLES:-0}"

if [[ "$TOTAL_SAMPLES" -lt 1 ]]; then
    echo "ERROR: probe CSV has zero non-zero ch/s samples; check probe.sh ran long enough." >&2
    exit 1
fi

MEAN="$(echo "$SAMPLES_RAW" | awk '{sum+=$1; n++} END{ if (n>0) printf "%.2f", sum/n; else print "0.00" }')"
MIN_VAL="$(echo "$SAMPLES_SORTED" | head -1)"
MAX_VAL="$(echo "$SAMPLES_SORTED" | tail -1)"

P50_IDX=$(( (TOTAL_SAMPLES - 1) / 2 ))
P95_IDX=$(( ((TOTAL_SAMPLES - 1) * 95) / 100 ))
[[ "$P95_IDX" -lt "$P50_IDX" ]] && P95_IDX="$P50_IDX"

P50="$(echo "$SAMPLES_SORTED" | sed -n "$((P50_IDX+1))p")"
P95="$(echo "$SAMPLES_SORTED" | sed -n "$((P95_IDX+1))p")"
P50="${P50:-0.00}"
P95="${P95:-0.00}"

# Final aggregates from the last row of the CSV (if available)
LAST_LINE="$(tail -1 "$EXISTING_CSV")"
FINAL_FILES="$(echo "$LAST_LINE" | awk -F',' '{print $2}')"
TOTAL_CHUNKS="$(echo "$LAST_LINE" | awk -F',' '{print $3}')"
DURATION_S="$(echo "$LAST_LINE" | awk -F',' '{print $1}')"

TS_OUT="$(date -u +%Y%m%dT%H%M%SZ)"
SUMMARY_OUT="$ROOT_DIR/dev-bench-end-to-end-${TS_OUT}.summary.csv"
{
    echo "label,bench,total_samples,duration_s,total_chunks,mean_ch_per_s,min_ch_per_s,max_ch_per_s,p50_ch_per_s,p95_ch_per_s,final_files,probe_csv"
    echo "${LABEL},end-to-end,${TOTAL_SAMPLES},${DURATION_S},${TOTAL_CHUNKS},${MEAN},${MIN_VAL},${MAX_VAL},${P50},${P95},${FINAL_FILES},${EXISTING_CSV}"
} > "$SUMMARY_OUT"

echo "📊 axon-bench-end-to-end [${LABEL}]"
echo "   probe_csv       ${EXISTING_CSV}"
echo "   total_samples   ${TOTAL_SAMPLES}"
echo "   duration_s      ${DURATION_S}"
echo "   total_chunks    ${TOTAL_CHUNKS}"
echo "   mean_ch_per_s   ${MEAN}"
echo "   min / max       ${MIN_VAL} / ${MAX_VAL}"
echo "   p50 / p95       ${P50} / ${P95}"
echo "   final_files     ${FINAL_FILES}"
echo "   summary         ${SUMMARY_OUT}"
