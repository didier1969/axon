#!/usr/bin/env bash
# Session 48 — EXPLOIT A3 sweep (Goldratt Step 2). Sweeps
# AXON_A3_BATCH_SIZE × AXON_A3_BATCH_TIMEOUT_MS and captures
# sustained ch/s + drum identification for each cell.
#
# Output: docs/working-notes/bench-data/2026-05-19-session-48-a3-sweep.csv
#
# Discardable wrapper — not part of the canonical bench surface.
# Kept in scripts/dev/ for reproducibility ; remove after session 48
# results are committed.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

OUT="docs/working-notes/bench-data/2026-05-19-session-48-a3-sweep.csv"
mkdir -p "$(dirname "$OUT")"

# CSV header is written by the bench itself ; the wrapper just stitches
# rows across cells. We capture (a3_batch_size, a3_batch_timeout_ms) as
# a leading composite label by overwriting `label` in each line.
header_written=0

# Cells to sweep. Skip combinations that are obviously bad (large batch +
# small timeout = same as small batch since flush happens before fill).
cells=(
    "32 10"      # baseline (defaults)
    "32 50"
    "64 10"
    "64 50"
    "64 200"
    "128 50"
    "128 200"
    "256 100"
    "256 200"
)

for cell in "${cells[@]}"; do
    read -r BATCH TIMEOUT <<< "$cell"
    label="a3=${BATCH}/${TIMEOUT}ms"
    echo "▶ Sweep cell: $label" >&2

    AXON_A3_BATCH_SIZE="$BATCH" \
    AXON_A3_BATCH_TIMEOUT_MS="$TIMEOUT" \
    AXON_B2_BATCH_SIZE=128 \
    AXON_B2_BATCH_TIMEOUT_MS=200 \
    AXON_A1_WORKERS=4 \
    AXON_A2_WORKERS=8 \
    AXON_A3_WORKERS=2 \
    AXON_B1_WORKERS=4 \
    AXON_B2_WORKERS=1 \
    AXON_B3_WORKERS=2 \
        scripts/dev/bench-v2.sh \
            --source ./src \
            --max-files 1500 \
            --duration-secs 45 \
            --warmup-secs 10 \
            --gpu \
            --csv \
        > /tmp/sweep-cell.csv 2>/tmp/sweep-cell.stderr || {
            echo "❌ Cell $label failed — see /tmp/sweep-cell.stderr"
            tail -10 /tmp/sweep-cell.stderr >&2
            continue
        }

    if (( header_written == 0 )); then
        head -n 1 /tmp/sweep-cell.csv > "$OUT"
        header_written=1
    fi
    # Replace the 'v2-bench' label with the cell identifier (use | as
    # sed delimiter since the label contains / characters).
    tail -n 1 /tmp/sweep-cell.csv | sed "s|^v2-bench|${label}|" >> "$OUT"
done

echo "Sweep complete : $OUT"
echo
echo "=== Summary ==="
python3 - <<PYEOF
import csv
with open("$OUT") as f:
    r = csv.DictReader(f)
    rows = list(r)
print(f"{'cell':<18} {'sus_ch/s':>10} {'drum':>5} {'drum_ratio':>10} {'b2_util':>8} {'a3_util':>8}")
print("-" * 70)
for row in rows:
    cell = row["label"]
    sus = float(row["sustained_chunks_per_sec"])
    drum = row["drum_identified"]
    drum_r = float(row["drum_work_ratio"])
    b2 = float(row["b2_work_ratio"])
    a3 = float(row["a3_work_ratio"])
    print(f"{cell:<18} {sus:>10.2f} {drum:>5} {drum_r:>10.2%} {b2:>8.2%} {a3:>8.2%}")
PYEOF
