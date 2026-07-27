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

assert_jobs() {
    local desc="$1" expected="$2" avail="$3" swap="$4" cores="$5" per_job="${6:-2}" got
    got="$(axon_compute_cargo_jobs "$avail" "$swap" "$cores" "$per_job")"
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
assert_jobs 'the real host (24 GB free, swap 98%, 16 cores) → 6, not 16' \
    6 24 98 16

# Memory is the binding constraint, not cores: plenty of cores but little RAM must NOT
# spawn one rustc per core.
assert_jobs 'RAM-starved host is bounded by RAM, not by cores' \
    2 4 0 16

# …and the converse: never exceed the core count even with abundant RAM. This policy may
# only ever LOWER parallelism; raising it above cargo's own default is not its job.
assert_jobs 'abundant RAM is still capped at the core count' \
    8 128 0 8

# Swap pressure halves the budget. Heavy swap use means the kernel is ALREADY evicting;
# the next allocation storm is what invokes the OOM killer.
assert_jobs 'swap ≥ 50% halves the budget' \
    6 24 50 16
assert_jobs 'swap just under the threshold does not halve' \
    12 24 49 16

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
