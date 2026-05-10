#!/usr/bin/env bash
# REQ-AXO-261 — TDD for bench-end-to-end.sh awk math.
# Verifies summary computation on a synthetic probe CSV without
# requiring a real indexer run.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

PASS=0
FAIL=0
assert() {
    local desc="$1"
    local cond="$2"
    if eval "$cond"; then
        printf '  PASS  %s\n' "$desc"
        PASS=$(( PASS + 1 ))
    else
        printf '  FAIL  %s  (cond: %s)\n' "$desc" "$cond"
        FAIL=$(( FAIL + 1 ))
    fi
}

# Build a synthetic probe CSV with known ch/s values
SANDBOX="$(mktemp -d -t axon-bench-end-to-end-test-XXXXXX)"
trap 'rm -rf "$SANDBOX"' EXIT INT TERM

PROBE_CSV="$SANDBOX/dev-probe-test-X.csv"
cat > "$PROBE_CSV" <<'CSV'
t_seconds,files,chunks_total,chunks_per_sec,ready_queue,claim_mode,gpu_mb,zombies,provider
0,0,0,0.00,0,fast,0,0,gpu
5,3,30,6.00,2,fast,500,0,gpu
10,7,75,9.00,3,fast,520,0,gpu
15,12,150,15.00,4,fast,530,0,gpu
20,18,250,20.00,3,fast,540,0,gpu
25,25,360,22.00,2,fast,545,0,gpu
30,33,470,22.00,1,fast,550,0,gpu
CSV

cd "$ROOT_DIR"
OUTPUT="$(bash "$SCRIPT_DIR/bench-end-to-end.sh" --csv "$PROBE_CSV" --label "test" 2>&1)"
SUMMARY_FILE="$(ls -1t "$ROOT_DIR"/dev-bench-end-to-end-*.summary.csv 2>/dev/null | head -1)"

assert "T1: summary file produced" "[[ -f '$SUMMARY_FILE' ]]"
assert "T1: summary has header row" "head -1 '$SUMMARY_FILE' | grep -q 'label,bench,total_samples'"
assert "T1: summary has data row" "wc -l < '$SUMMARY_FILE' | awk '\$1 == 2 { exit 0 } { exit 1 }'"

# 6 non-zero samples (skipped t=0 row): 6, 9, 15, 20, 22, 22
# mean = (6+9+15+20+22+22)/6 = 94/6 = 15.67
assert "T2: mean_ch_per_s ≈ 15.67" "tail -1 '$SUMMARY_FILE' | awk -F',' '{printf \"%.2f\", \$6}' | grep -q '15.67'"
assert "T2: total_samples = 6" "tail -1 '$SUMMARY_FILE' | awk -F',' '{print \$3}' | grep -q '^6$'"
assert "T2: min = 6.00" "tail -1 '$SUMMARY_FILE' | awk -F',' '{print \$7}' | grep -q '^6.00$'"
assert "T2: max = 22.00" "tail -1 '$SUMMARY_FILE' | awk -F',' '{print \$8}' | grep -q '^22.00$'"
assert "T2: p50 = 15.00 (idx (6-1)/2 = 2 of sorted [6,9,15,20,22,22])" "tail -1 '$SUMMARY_FILE' | awk -F',' '{print \$9}' | grep -q '^15.00$'"
assert "T2: p95 = 22.00 (idx ((6-1)*95/100) = 4 of sorted)" "tail -1 '$SUMMARY_FILE' | awk -F',' '{print \$10}' | grep -q '^22.00$'"
assert "T2: total_chunks = 470" "tail -1 '$SUMMARY_FILE' | awk -F',' '{print \$5}' | grep -q '^470$'"
assert "T2: duration_s = 30" "tail -1 '$SUMMARY_FILE' | awk -F',' '{print \$4}' | grep -q '^30$'"
assert "T2: final_files = 33" "tail -1 '$SUMMARY_FILE' | awk -F',' '{print \$11}' | grep -q '^33$'"

# Cleanup the produced summary file (would pollute git status)
rm -f "$SUMMARY_FILE"

# Test 3: rejects empty / all-zero CSV
EMPTY_CSV="$SANDBOX/empty.csv"
cat > "$EMPTY_CSV" <<'CSV'
t_seconds,files,chunks_total,chunks_per_sec,ready_queue,claim_mode,gpu_mb,zombies,provider
0,0,0,0.00,0,fast,0,0,gpu
5,0,0,0.00,0,fast,0,0,gpu
CSV
EMPTY_OUT="$(bash "$SCRIPT_DIR/bench-end-to-end.sh" --csv "$EMPTY_CSV" --label "empty" 2>&1 || true)"
assert "T3: rejects all-zero CSV" "echo '$EMPTY_OUT' | grep -q 'zero non-zero'"

# Test 4: rejects missing CSV
MISSING_OUT="$(bash "$SCRIPT_DIR/bench-end-to-end.sh" --csv "/tmp/does-not-exist-axon-12345.csv" 2>&1 || true)"
assert "T4: rejects missing CSV" "echo '$MISSING_OUT' | grep -q 'CSV not found'"

# Test 5: rejects --run without --scope
NO_SCOPE_OUT="$(bash "$SCRIPT_DIR/bench-end-to-end.sh" --run 2>&1 || true)"
assert "T5: --run requires --scope" "echo '$NO_SCOPE_OUT' | grep -q 'requires --scope'"

# Test 6: help works
HELP_OUT="$(bash "$SCRIPT_DIR/bench-end-to-end.sh" --help 2>&1)"
assert "T6: --help contains REQ-AXO-261" "echo '$HELP_OUT' | grep -q 'REQ-AXO-261'"

echo ""
echo "=== Result: $PASS passed, $FAIL failed ==="
[[ "$FAIL" -eq 0 ]] || exit 1
exit 0
