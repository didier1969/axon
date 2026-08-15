#!/usr/bin/env bash
# tests/shell/test_host_check_dxgvmb.sh — REQ-AXO-902334
#
# Falsify the `dxgvmb` probe of scripts/check-host-before-suite.sh in BOTH
# directions.
#
# Why this file exists
# --------------------
# The first version of that probe took ONE `ps` sample and, on any D-state thread
# blocked in `dxgvmb_send_sync_msg`, printed "GPU channel ALREADY jammed" and
# prescribed `wsl --shutdown` — a remedy that closes every Windows session the
# operator has open. Measured on a healthy host on 2026-08-15: 1 sample in 5
# showed such a thread, with a DIFFERENT tid each time. It was an indexer doing
# its job. WSL2 exposes a single synchronous GPU channel, so a transient D-state
# is the NORMAL state under load, not a fault.
#
# A guard that cries wolf is a guard people learn to skip — the check-host script
# argues exactly that in its own comments about the live-serving line, three
# blocks above where it then did it. So the probe now requires the SAME tid across
# every sample, plus an independent corroboration, and this file proves both
# branches are reachable. Neither assertion can pass by accident: each pins a
# distinct string that only its own branch emits.
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CHECK="$SCRIPT_DIR/scripts/check-host-before-suite.sh"
fails=0

pass() { printf '  ✅ %s\n' "$1"; }
fail() { printf '  ❌ %s\n' "$1"; fails=$((fails + 1)); }

printf '\n== test_host_check_dxgvmb (REQ-AXO-902334) ==\n\n'

[[ -r "$CHECK" ]] || { printf '  ❌ %s unreadable\n' "$CHECK"; exit 1; }

# --- 1. NO D-thread at all: the probe must stay silent about jamming ----------
out="$(DXGVMB_PROBE_CMD='true' DXGVMB_SAMPLES=3 DXGVMB_INTERVAL=0 bash "$CHECK" 2>&1)"
if grep -q 'tid stable across all: no' <<<"$out"; then
    pass "no D-thread → reports 'stable: no'"
else
    fail "no D-thread → expected 'tid stable across all: no', got: $(grep dxgvmb <<<"$out")"
fi
if grep -q 'JAMMED' <<<"$out"; then
    fail "no D-thread → must NOT claim JAMMED"
else
    pass "no D-thread → does not claim JAMMED"
fi

# --- 2. TRANSIENT D-threads (a different tid each sample) = normal traffic ----
# This is the exact shape that produced the false positive.
stub_transient='echo $((RANDOM + 100000))'
out="$(DXGVMB_PROBE_CMD="$stub_transient" DXGVMB_SAMPLES=5 DXGVMB_INTERVAL=0 bash "$CHECK" 2>&1)"
if grep -q 'present in 5/5 samples' <<<"$out"; then
    pass "transient → the sample count is REPORTED, not hidden behind a verdict"
else
    fail "transient → expected 'present in 5/5 samples', got: $(grep dxgvmb <<<"$out")"
fi
if grep -q 'tid stable across all: no' <<<"$out"; then
    pass "transient → changing tid is NOT read as a wedge"
else
    fail "transient → a changing tid was treated as stable"
fi
if grep -q 'wsl --shutdown' <<<"$out"; then
    fail "transient → must NOT prescribe wsl --shutdown (the original defect)"
else
    pass "transient → does not prescribe wsl --shutdown"
fi

# --- 3. NEGATIVE CONTROL: a stable tid MUST still be able to fire -------------
# Without this, every assertion above could be satisfied by a probe that simply
# never reports anything — a guard that cannot go red is not a guard.
out="$(DXGVMB_PROBE_CMD='echo 424242' DXGVMB_SAMPLES=4 DXGVMB_INTERVAL=0 bash "$CHECK" 2>&1)"
if grep -q 'tid stable across all: YES' <<<"$out"; then
    pass "stable tid → detected as stable (the jam branch is REACHABLE)"
else
    fail "stable tid → not detected; the jam branch is dead code"
fi

# --- 4. Stable tid + a LIVE runtime = long GPU call, not a wedge --------------
# Corroboration is what keeps the expensive remedy honest. When the brain still
# answers /readyz, a persistent tid is a long call; naming `wsl --shutdown` there
# is what taught everyone to ignore this script.
if curl -fsS -m 3 'http://127.0.0.1:44129/readyz' >/dev/null 2>&1; then
    if grep -q 'still answers' <<<"$out"; then
        pass "stable tid + live runtime → reported as a long call, no shutdown prescribed"
    else
        fail "stable tid + live runtime → expected the 'still answers' branch"
    fi
else
    printf '  ⏭  live brain not answering — corroboration branch not exercised\n'
fi

printf '\n'
if [[ "$fails" -eq 0 ]]; then
    printf '  ✅ all assertions passed\n\n'
    exit 0
fi
printf '  ❌ %s assertion(s) failed\n\n' "$fails"
exit 1
