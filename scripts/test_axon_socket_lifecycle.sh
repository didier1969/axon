#!/usr/bin/env bash
# REQ-AXO-117 — start/stop socket lifecycle integration test.
#
# Exercises the helpers in scripts/lib/socket-lifecycle.sh that REQ-AXO-093
# fixes depend on:
#   1. axon_socket_responds() correctly distinguishes orphan socket files
#      from live AF_UNIX listeners (5-case probe matrix from the original
#      smoke test in commit 4107623).
#   2. axon_cleanup_role_state() and axon_cleanup_legacy_instance_paths()
#      remove every artifact that the orphan-socket guard depends on:
#      role-specific telemetry/MCP sockets, role pid file, runtime.env,
#      and the legacy non-role-specific socket paths.
#   3. Both helpers are idempotent — running cleanup twice in sequence
#      catches the orphan-block pattern where the second start would
#      otherwise misread leftover state as a live data plane.
#
# Exit codes:
#   0 : all assertions passed.
#   non-zero : first failure, with a diagnostic listing the offending state.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/lib/socket-lifecycle.sh
source "$ROOT_DIR/scripts/lib/socket-lifecycle.sh"

TEST_INSTANCE="testlifecycle"
TMP_PREFIX="/tmp/axon-${TEST_INSTANCE}"
SCRATCH_DIR=""
LISTENER_PID=""

cleanup_scratch() {
    if [[ -n "$LISTENER_PID" ]] && kill -0 "$LISTENER_PID" 2>/dev/null; then
        kill "$LISTENER_PID" 2>/dev/null || true
        wait "$LISTENER_PID" 2>/dev/null || true
    fi
    rm -f "$TMP_PREFIX"-*.sock
    rm -f "$TMP_PREFIX-telemetry.sock" "$TMP_PREFIX-mcp.sock"
    [[ -n "$SCRATCH_DIR" && -d "$SCRATCH_DIR" ]] && rm -rf "$SCRATCH_DIR"
}
trap cleanup_scratch EXIT

fail() {
    local label="$1"
    echo "FAIL: $label" >&2
    if compgen -G "$TMP_PREFIX*" >/dev/null; then
        echo "  leftover /tmp/axon-${TEST_INSTANCE}* paths:" >&2
        ls -la "$TMP_PREFIX"* >&2 || true
    fi
    if [[ -n "$SCRATCH_DIR" && -d "$SCRATCH_DIR" ]]; then
        echo "  leftover scratch state under $SCRATCH_DIR:" >&2
        find "$SCRATCH_DIR" -type f >&2 || true
    fi
    exit 1
}

assert_responds_yes() {
    local sock="$1"
    local label="$2"
    if ! axon_socket_responds "$sock"; then
        fail "$label (axon_socket_responds expected 0 for $sock)"
    fi
}

assert_responds_no() {
    local sock="$1"
    local label="$2"
    if axon_socket_responds "$sock"; then
        fail "$label (axon_socket_responds expected non-zero for $sock)"
    fi
}

assert_path_absent() {
    local path="$1"
    local label="$2"
    if [[ -e "$path" ]]; then
        fail "$label (expected $path absent, still present)"
    fi
}

# Pre-condition: previous run must not leak state.
rm -f "$TMP_PREFIX"-*.sock "$TMP_PREFIX-telemetry.sock" "$TMP_PREFIX-mcp.sock"

# --- Probe matrix: 5 cases ---------------------------------------------------

# Case 1: nonexistent path → 1.
assert_responds_no "$TMP_PREFIX-nonexistent.sock" "case1 nonexistent"

# Case 2: regular file masquerading as socket → 1.
regular_file="$TMP_PREFIX-regular-file.sock"
: > "$regular_file"
assert_responds_no "$regular_file" "case2 regular file"
rm -f "$regular_file"

# Case 3: bound socket file with no listener (orphan) → 1.
orphan_sock="$TMP_PREFIX-orphan.sock"
python3 - "$orphan_sock" <<'PYEOF'
import os, socket, sys
path = sys.argv[1]
try:
    os.unlink(path)
except FileNotFoundError:
    pass
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.bind(path)
# Socket is bound but never listen()ed and is closed immediately.
# The bound inode persists until unlinked.
s.close()
PYEOF
assert_responds_no "$orphan_sock" "case3 bound-no-listener"
rm -f "$orphan_sock"

# Case 4: live listener → 0.
live_sock="$TMP_PREFIX-live.sock"
python3 - "$live_sock" <<'PYEOF' &
import os, socket, signal, sys, time
path = sys.argv[1]
try:
    os.unlink(path)
except FileNotFoundError:
    pass
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.bind(path)
s.listen(1)
def _term(signum, frame):
    # Exit immediately on SIGTERM without unlinking — leaves orphan
    # path for case 5.
    os._exit(0)
signal.signal(signal.SIGTERM, _term)
time.sleep(30)
PYEOF
LISTENER_PID=$!
# Wait briefly for the listener to bind.
for _ in 1 2 3 4 5 6 7 8 9 10; do
    [[ -S "$live_sock" ]] && break
    sleep 0.1
done
assert_responds_yes "$live_sock" "case4 live listener"

# Case 5: killed listener leaves the socket file as orphan → 1.
kill "$LISTENER_PID" 2>/dev/null || true
wait "$LISTENER_PID" 2>/dev/null || true
LISTENER_PID=""
# Brief pause so the kernel finishes tearing down the listener fd.
sleep 0.2
[[ -S "$live_sock" ]] || fail "case5 expected orphan socket file to remain"
assert_responds_no "$live_sock" "case5 killed listener leaves orphan"
rm -f "$live_sock"

# --- Cleanup helper: synthetic role state -----------------------------------

SCRATCH_DIR="$(mktemp -d -t axon-socket-test.XXXXXX)"

prepare_role_state() {
    local role="$1"
    local run_root_base="$2"
    local role_dir="$run_root_base/run-${role}"
    mkdir -p "$role_dir"
    : > "$TMP_PREFIX-${role}-telemetry.sock"
    : > "$TMP_PREFIX-${role}-mcp.sock"
    : > "$role_dir/axon-${role}.pid"
    : > "$role_dir/runtime.env"
}

assert_role_state_absent() {
    local role="$1"
    local run_root_base="$2"
    local label_prefix="$3"
    assert_path_absent "$TMP_PREFIX-${role}-telemetry.sock" \
        "$label_prefix telemetry socket"
    assert_path_absent "$TMP_PREFIX-${role}-mcp.sock" \
        "$label_prefix mcp socket"
    assert_path_absent "$run_root_base/run-${role}/axon-${role}.pid" \
        "$label_prefix pid file"
    assert_path_absent "$run_root_base/run-${role}/runtime.env" \
        "$label_prefix runtime.env"
}

# Cycle 1: prepare brain + indexer state, run cleanup, assert removal.
prepare_role_state brain "$SCRATCH_DIR"
prepare_role_state indexer "$SCRATCH_DIR"
: > "$TMP_PREFIX-telemetry.sock"
: > "$TMP_PREFIX-mcp.sock"

axon_cleanup_role_state "$TEST_INSTANCE" brain "$SCRATCH_DIR"
axon_cleanup_role_state "$TEST_INSTANCE" indexer "$SCRATCH_DIR"
axon_cleanup_legacy_instance_paths "$TEST_INSTANCE"

assert_role_state_absent brain "$SCRATCH_DIR" "cycle1 brain"
assert_role_state_absent indexer "$SCRATCH_DIR" "cycle1 indexer"
assert_path_absent "$TMP_PREFIX-telemetry.sock" "cycle1 legacy telemetry"
assert_path_absent "$TMP_PREFIX-mcp.sock" "cycle1 legacy mcp"

# Cycle 2: re-prepare and re-run — catches the orphan-block pattern where
# leftover state would fool the next start into thinking the data plane
# is up.
prepare_role_state brain "$SCRATCH_DIR"
prepare_role_state indexer "$SCRATCH_DIR"
: > "$TMP_PREFIX-telemetry.sock"
: > "$TMP_PREFIX-mcp.sock"

axon_cleanup_role_state "$TEST_INSTANCE" brain "$SCRATCH_DIR"
axon_cleanup_role_state "$TEST_INSTANCE" indexer "$SCRATCH_DIR"
axon_cleanup_legacy_instance_paths "$TEST_INSTANCE"

assert_role_state_absent brain "$SCRATCH_DIR" "cycle2 brain"
assert_role_state_absent indexer "$SCRATCH_DIR" "cycle2 indexer"
assert_path_absent "$TMP_PREFIX-telemetry.sock" "cycle2 legacy telemetry"
assert_path_absent "$TMP_PREFIX-mcp.sock" "cycle2 legacy mcp"

# Cycle 3: cleanup is idempotent on already-clean state.
axon_cleanup_role_state "$TEST_INSTANCE" brain "$SCRATCH_DIR"
axon_cleanup_legacy_instance_paths "$TEST_INSTANCE"
assert_role_state_absent brain "$SCRATCH_DIR" "cycle3 idempotent brain"

echo "PASS: axon socket lifecycle (REQ-AXO-117)"
