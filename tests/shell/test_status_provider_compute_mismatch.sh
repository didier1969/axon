#!/usr/bin/env bash
# tests/shell/test_status_provider_compute_mismatch.sh — REQ-AXO-902363
#
# Fixture tests for the provider_compute_mismatch status.sh logic.
# Verifies that when a mismatch between GPU provider intent and CPU compute occurs:
# 1. OVERALL is DEGRADED (printed at top)
# 2. FAIL provider_compute_mismatch is printed
# 3. STATUS is DEGRADED and exit code is 1
# 4. A clean heartbeat yields OVERALL HEALTHY and exit code 0
#
# Run: bash tests/shell/test_status_provider_compute_mismatch.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

PASS=0
FAIL=0
pass() { printf '  PASS  %s\n' "$1"; PASS=$(( PASS + 1 )); }
fail() { printf '  FAIL  %s\n' "$1"; FAIL=$(( FAIL + 1 )); }

# Extract the inline python script from scripts/status.sh to test it cleanly
EXTRACTED_PY="$TMP/status_render.py"
sed -n '/<<'\''PY'\''/,/^PY$/p' "$ROOT_DIR/scripts/status.sh" | sed '1d;$d' > "$EXTRACTED_PY"

run_case() {
    local axonctl_json="$1"
    local heartbeat_json="$2"
    local run_dir="$TMP/run"
    rm -rf "$run_dir" && mkdir -p "$run_dir"
    printf '%s\n' "$heartbeat_json" > "$run_dir/runtime-heartbeat.json"

    OUT="$(
        AXONCTL_JSON="$axonctl_json" \
        AXON_RUN_ROOT="$run_dir" \
        python3 "$EXTRACTED_PY" "live" "indexer" 2>&1
    )" && RC=0 || RC=$?
}

printf 'status provider_compute_mismatch — REQ-AXO-902363\n'

BASE_AXONCTL='{"instance_kind":"live","role":"indexer","overall":"healthy","effective_alive":true,"liveness_source":"writer_guard","ports":[],"sockets":[],"writer_guards":[],"role_contract_violations":[]}'

# Test 1: Clean heartbeat with GPU -> HEALTHY
run_case "$BASE_AXONCTL" '{"provider_compute_mismatch":false,"embedder_provider":"cuda","embedder_compute":"GPU"}'
if [[ "$RC" -eq 0 && "$OUT" == *"OVERALL  HEALTHY"* && "$OUT" == *"STATUS  HEALTHY"* ]]; then
    pass "un heartbeat avec compute GPU et provider cuda reste HEALTHY"
else
    fail "heartbeat sain devrait etre HEALTHY (rc=$RC, out=$OUT)"
fi

# Test 2: Heartbeat with provider_compute_mismatch=true -> DEGRADED
run_case "$BASE_AXONCTL" '{"provider_compute_mismatch":true,"embedder_provider":"cuda","embedder_compute":"CPU"}'
if [[ "$RC" -eq 1 && "$OUT" == *"OVERALL  DEGRADED"* && "$OUT" == *"STATUS  DEGRADED"* && "$OUT" == *"FAIL    provider_compute_mismatch"* ]]; then
    pass "un provider_compute_mismatch explicite degrade OVERALL et STATUS a DEGRADED (rc=1)"
else
    fail "mismatch explicite non detecte (rc=$RC, out=$OUT)"
fi

# Test 3: Heartbeat with effective cuda and observed CPU -> detected as mismatch
run_case "$BASE_AXONCTL" '{"effective_embed_provider":"cuda","observed_compute":"CPU"}'
if [[ "$RC" -eq 1 && "$OUT" == *"OVERALL  DEGRADED"* && "$OUT" == *"STATUS  DEGRADED"* && "$OUT" == *"FAIL    provider_compute_mismatch"* ]]; then
    pass "un couple (effective=cuda, observed=CPU) sans flag booleen est deduit comme mismatch"
else
    fail "deduction mismatch par provider/compute echouee (rc=$RC, out=$OUT)"
fi

# Test 4: Heartbeat with cpu provider and CPU compute -> HEALTHY (no mismatch)
run_case "$BASE_AXONCTL" '{"effective_embed_provider":"cpu","observed_compute":"CPU"}'
if [[ "$RC" -eq 0 && "$OUT" == *"OVERALL  HEALTHY"* && "$OUT" == *"STATUS  HEALTHY"* ]]; then
    pass "un couple (effective=cpu, observed=CPU) est coherent et reste HEALTHY"
else
    fail "cpu sur CPU ne doit pas etre considere comme mismatch (rc=$RC, out=$OUT)"
fi

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]] || exit 1
