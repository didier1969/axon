#!/usr/bin/env bash
# REQ-AXO-901968 — unit tests for the cross-project control-plane guard.
# Pure-function tests (no process kill, no runtime side effect).
# Run: bash scripts/lib/axon-lifecycle-guard.test.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=axon-lifecycle-guard.sh
source "$SCRIPT_DIR/axon-lifecycle-guard.sh"

PASS=0
FAIL=0
assert_authorized() {
    local desc="$1" pwd_="$2" root="$3" override="${4:-0}"
    if axon_lifecycle_authorized "$pwd_" "$root" "$override"; then
        printf '  PASS  %s\n' "$desc"; PASS=$(( PASS + 1 ))
    else
        printf '  FAIL  %s  (expected authorized)\n' "$desc"; FAIL=$(( FAIL + 1 ))
    fi
}
assert_refused() {
    local desc="$1" pwd_="$2" root="$3" override="${4:-0}"
    if axon_lifecycle_authorized "$pwd_" "$root" "$override"; then
        printf '  FAIL  %s  (expected refused)\n' "$desc"; FAIL=$(( FAIL + 1 ))
    else
        printf '  PASS  %s\n' "$desc"; PASS=$(( PASS + 1 ))
    fi
}

ROOT="/home/dstadel/projects/axon"

assert_authorized "cwd == repo root"                 "$ROOT"            "$ROOT"
assert_authorized "cwd is repo subdirectory"         "$ROOT/scripts"    "$ROOT"
assert_authorized "cwd deep in repo"                 "$ROOT/src/a/b"    "$ROOT"
assert_refused    "foreign project cwd"              "/home/dstadel/projects/fiscaly" "$ROOT"
assert_refused    "sibling with shared prefix"       "/home/dstadel/projects/axon-other" "$ROOT"
assert_refused    "parent of repo"                   "/home/dstadel/projects" "$ROOT"
assert_refused    "empty cwd"                         ""                "$ROOT"
assert_refused    "empty repo root"                   "$ROOT"           ""
assert_authorized "override bypasses foreign cwd"    "/tmp/elsewhere"   "$ROOT" "1"

echo "----"
echo "PASS=$PASS FAIL=$FAIL"
[[ "$FAIL" -eq 0 ]]
