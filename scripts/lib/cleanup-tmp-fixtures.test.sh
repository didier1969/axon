#!/usr/bin/env bash
# REQ-AXO-255 / DEC-AXO-076 — TDD test for axon_cleanup_tmp_fixtures.
# Run: bash scripts/lib/cleanup-tmp-fixtures.test.sh
# Exit code 0 on pass, 1 on any failed assertion.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=cleanup-tmp-fixtures.sh
source "$SCRIPT_DIR/cleanup-tmp-fixtures.sh"

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

setup_sandbox() {
    SANDBOX="$(mktemp -d -t axon-cleanup-test-XXXXXX)"
    AXON_CLEANUP_LOG="$SANDBOX/axon-cleanup.log"
    export AXON_CLEANUP_LOG

    # Old fixtures (mtime 2h ago) — should be swept.
    local old_ts
    old_ts="$(date -d '2 hours ago' '+%Y%m%d%H%M.%S')"
    mkdir -p "$SANDBOX/axon_test_db_old"
    echo "old" > "$SANDBOX/axon_test_db_old/data"
    touch -t "$old_ts" "$SANDBOX/axon_test_db_old"

    mkdir -p "$SANDBOX/.tmpAbCdEf"  # 6-char tempfile pattern
    touch -t "$old_ts" "$SANDBOX/.tmpAbCdEf"

    mkdir -p "$SANDBOX/axon-legacy-ist-1234-9876543210"
    touch -t "$old_ts" "$SANDBOX/axon-legacy-ist-1234-9876543210"

    mkdir -p "$SANDBOX/axon-embedding-soft-reset-555-111"
    touch -t "$old_ts" "$SANDBOX/axon-embedding-soft-reset-555-111"

    mkdir -p "$SANDBOX/hydra_db_test"
    touch -t "$old_ts" "$SANDBOX/hydra_db_test"

    touch -t "$old_ts" "$SANDBOX/soll.db.backup-20260507T053303Z"

    # Recent fixtures (just created) — must NOT be swept (concurrent test guard).
    mkdir -p "$SANDBOX/axon_test_db_new"
    echo "new" > "$SANDBOX/axon_test_db_new/data"

    # Out-of-allowlist entries — must NEVER be swept.
    mkdir -p "$SANDBOX/.X11-unix"
    touch -t "$old_ts" "$SANDBOX/.X11-unix"

    mkdir -p "$SANDBOX/onnxruntime-cache"
    touch -t "$old_ts" "$SANDBOX/onnxruntime-cache"

    touch -t "$old_ts" "$SANDBOX/important_user_file.txt"

    # 7-char tmp suffix (NOT the 6-char tempfile pattern) — must NOT match.
    mkdir -p "$SANDBOX/.tmpAbCdEfG"
    touch -t "$old_ts" "$SANDBOX/.tmpAbCdEfG"
}

teardown_sandbox() {
    rm -rf "$SANDBOX" 2>/dev/null || true
}

trap 'teardown_sandbox; exit' EXIT INT TERM

echo "=== axon_cleanup_tmp_fixtures TDD ==="

# Test 1: dry-run reports matches but deletes nothing.
setup_sandbox
out="$(axon_cleanup_tmp_fixtures --dir="$SANDBOX" --age-hours=1 --dry-run --quiet 2>&1 || true)"
assert "T1: dry-run preserves old fixtures" "[[ -d '$SANDBOX/axon_test_db_old' ]]"
assert "T1: dry-run preserves .tmpAbCdEf" "[[ -d '$SANDBOX/.tmpAbCdEf' ]]"
assert "T1: dry-run log mentions DRY" "grep -q 'DRY' '$AXON_CLEANUP_LOG'"
teardown_sandbox

# Test 2: actual cleanup deletes old fixtures.
setup_sandbox
axon_cleanup_tmp_fixtures --dir="$SANDBOX" --age-hours=1 --quiet
assert "T2: axon_test_db_old deleted" "[[ ! -e '$SANDBOX/axon_test_db_old' ]]"
assert "T2: .tmpAbCdEf deleted" "[[ ! -e '$SANDBOX/.tmpAbCdEf' ]]"
assert "T2: axon-legacy-ist-* deleted" "[[ ! -e '$SANDBOX/axon-legacy-ist-1234-9876543210' ]]"
assert "T2: axon-embedding-soft-reset-* deleted" "[[ ! -e '$SANDBOX/axon-embedding-soft-reset-555-111' ]]"
assert "T2: hydra_db_test deleted" "[[ ! -e '$SANDBOX/hydra_db_test' ]]"
assert "T2: soll.db.backup-* deleted" "[[ ! -e '$SANDBOX/soll.db.backup-20260507T053303Z' ]]"
teardown_sandbox

# Test 3: recent fixtures preserved (concurrent test guard).
setup_sandbox
axon_cleanup_tmp_fixtures --dir="$SANDBOX" --age-hours=1 --quiet
assert "T3: axon_test_db_new (recent) preserved" "[[ -d '$SANDBOX/axon_test_db_new' ]]"
teardown_sandbox

# Test 4: out-of-allowlist entries NEVER touched (safety).
setup_sandbox
axon_cleanup_tmp_fixtures --dir="$SANDBOX" --age-hours=1 --quiet
assert "T4: .X11-unix preserved" "[[ -d '$SANDBOX/.X11-unix' ]]"
assert "T4: onnxruntime-cache preserved" "[[ -d '$SANDBOX/onnxruntime-cache' ]]"
assert "T4: important_user_file.txt preserved" "[[ -f '$SANDBOX/important_user_file.txt' ]]"
assert "T4: 7-char .tmp prefix preserved (only 6-char pattern matches)" "[[ -d '$SANDBOX/.tmpAbCdEfG' ]]"
teardown_sandbox

# Test 5: age safety floor — passing 0 still uses 1.
setup_sandbox
axon_cleanup_tmp_fixtures --dir="$SANDBOX" --age-hours=0 --quiet
assert "T5: age=0 floored to 1 — recent fixture preserved" "[[ -d '$SANDBOX/axon_test_db_new' ]]"
teardown_sandbox

# Test 6: never blocks on missing dir.
out="$(axon_cleanup_tmp_fixtures_safe --dir=/tmp/nonexistent-axon-cleanup-12345 --quiet 2>&1)"
assert "T6: missing dir does not error" "[[ \$? -eq 0 ]]"

# Test 7: log file is written.
setup_sandbox
axon_cleanup_tmp_fixtures --dir="$SANDBOX" --age-hours=1 --quiet
assert "T7: log file populated" "[[ -s '$AXON_CLEANUP_LOG' ]]"
assert "T7: log contains summary line" "grep -q 'summary deleted=' '$AXON_CLEANUP_LOG'"
teardown_sandbox

# Test 8: stdout summary format when not --quiet.
setup_sandbox
out="$(axon_cleanup_tmp_fixtures --dir="$SANDBOX" --age-hours=1 2>/dev/null)"
assert "T8: stdout has summary prefix" "[[ '$out' == axon-cleanup:* ]]"
assert "T8: stdout has deleted= field" "[[ '$out' == *deleted=* ]]"
assert "T8: stdout has freed= field" "[[ '$out' == *freed=* ]]"
teardown_sandbox

echo ""
echo "=== Result: $PASS passed, $FAIL failed ==="
if (( FAIL > 0 )); then
    exit 1
fi
exit 0
