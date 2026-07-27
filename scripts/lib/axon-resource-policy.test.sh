#!/usr/bin/env bash
# REQ-AXO-902267 — unit tests for the memory-bounded cargo job policy.
# Pure-function tests (no /proc read, no build, no runtime side effect).
# Run: bash scripts/lib/axon-resource-policy.test.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=axon-resource-policy.sh
source "$SCRIPT_DIR/axon-resource-policy.sh"

PASS=0
FAIL=0

# When no per-job budget is given, call the function WITHOUT that argument so ITS OWN
# default is what gets exercised. An earlier version of this helper substituted its own
# default (2) instead, which meant the tests could never have caught a drift in the
# function's default — the very thing the measured-budget test below exists to pin.
assert_jobs() {
    local desc="$1" expected="$2" avail="$3" swap="$4" cores="$5" per_job="${6:-}" got
    if [[ -n "$per_job" ]]; then
        got="$(axon_compute_cargo_jobs "$avail" "$swap" "$cores" "$per_job")"
    else
        got="$(axon_compute_cargo_jobs "$avail" "$swap" "$cores")"
    fi
    if [[ "$got" == "$expected" ]]; then
        printf '  PASS  %s\n' "$desc"; PASS=$(( PASS + 1 ))
    else
        printf '  FAIL  %s  (expected %s, got %s)\n' "$desc" "$expected" "$got"; FAIL=$(( FAIL + 1 ))
    fi
}

printf 'axon_compute_cargo_jobs — REQ-AXO-902267\n'

# The host as measured when this was written: 24 GB free, swap at 98 %, 16 cores.
# Unbounded cargo would run 16 rustc processes on a machine already evicting to swap —
# the shape that produced two global OOM kills (Chrome died twice) during a promote.
# 24/3 = 8, halved for swap ≥ 50 % → 4.
assert_jobs 'the real host (24 GB free, swap 98%, 16 cores) → 4, not 16' \
    4 24 98 16

# Memory is the binding constraint, not cores: plenty of cores but little RAM must NOT
# spawn one rustc per core.
assert_jobs 'RAM-starved host is bounded by RAM, not by cores' \
    1 4 0 16

# …and the converse: never exceed the core count even with abundant RAM. This policy may
# only ever LOWER parallelism; raising it above cargo's own default is not its job.
assert_jobs 'abundant RAM is still capped at the core count' \
    8 128 0 8

# Swap pressure halves the budget. Heavy swap use means the kernel is ALREADY evicting;
# the next allocation storm is what invokes the OOM killer.
assert_jobs 'swap ≥ 50% halves the budget' \
    4 24 50 16
assert_jobs 'swap just under the threshold does not halve' \
    8 24 49 16

# The default budget must stay at the MEASURED peak (2.13 GB rounded up), never drift back
# to the estimate it replaced. 24/3 = 8 with no swap pressure.
assert_jobs 'default per-job budget is the measured 3 GB, not the old estimate of 2' \
    8 24 0 16

# Never return 0 or a negative: `cargo -j 0` is an error, and a build that cannot start is
# a worse failure than a slow one.
assert_jobs 'no free memory still yields a usable job count' \
    1 0 0 16
assert_jobs 'tiny memory + heavy swap still yields at least 1' \
    1 1 99 16

# A single-core host must not be handed more than it has.
assert_jobs 'single-core host gets exactly 1' \
    1 64 0 1

# The per-job budget is a knob: a host known to build cheaply can lower it.
assert_jobs 'a smaller per-job budget allows more jobs' \
    12 24 0 16 2
assert_jobs 'a larger per-job budget allows fewer' \
    6 24 0 16 4

# A degenerate per-job budget must not divide by zero.
assert_jobs 'per-job budget of 0 is coerced to 1, never a division by zero' \
    16 24 0 16 0

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
