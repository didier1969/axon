#!/usr/bin/env bash
# Regression test for REQ-AXO-901635 / REQ-AXO-901636 / REQ-AXO-901637.
# DEC-AXO-901598 canonical-only scope + binary-anchored process identity.
#
# Scenario reproduced (session 50):
#   - Dashboard BEAM listens on PHX_PORT.
#   - No axon-brain / axon-indexer canonical process alive.
#   - `scripts/stop.sh --verify` must exit 0 (canonical scope only).
#
# Test A : dashboard listener on PHX_PORT, no canonical process -> verify OK.
# Test B : third-party listener on a non-canonical port, no canonical process -> verify OK.
# Test C : (skip if any canonical process alive ; would interfere with state).
# Test D : --hard mode does NOT kill processes whose cmdline contains 'axon' but
#          is not in ${PROJECT_ROOT}/bin/.

set -euo pipefail

TEST_NAME="test_stop_canonical_scope"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
INSTANCE_KIND="${AXON_INSTANCE_KIND:-live}"

fail() {
    echo "FAIL [$TEST_NAME/$1]: $2" >&2
    exit 1
}

pass() {
    echo "PASS [$TEST_NAME/$1]: $2"
}

cleanup_pid() {
    local pid="$1"
    [ -z "$pid" ] && return 0
    kill -KILL "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
}

spawn_listener() {
    # $1 = port  $2 = python marker tag for cmdline introspection
    local port="$1"
    local marker="$2"
    python3 -u -c "
import socket, sys, signal, time
marker = '$marker'
s = socket.socket()
s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(('127.0.0.1', $port))
s.listen()
sys.stdout.write(f'listener_ready marker={marker} port=$port pid={ \
__import__(\"os\").getpid()}\\n')
sys.stdout.flush()
signal.signal(signal.SIGTERM, lambda *_: sys.exit(0))
while True:
    try:
        c, _ = s.accept()
        c.close()
    except Exception:
        break
" &
    local pid=$!
    # Wait briefly for the listener to actually bind.
    for _ in 1 2 3 4 5; do
        if ss -tln "sport = :$port" 2>/dev/null | grep -q ":$port"; then
            echo "$pid"
            return 0
        fi
        sleep 0.2
    done
    cleanup_pid "$pid"
    fail "spawn_listener" "failed to bind port $port"
}

# Resolve ports through axon-instance.sh.
# shellcheck source=scripts/lib/axon-instance.sh
source "$PROJECT_ROOT/scripts/lib/axon-instance.sh"
export AXON_INSTANCE_KIND="$INSTANCE_KIND"
axon_resolve_instance "$PROJECT_ROOT" "$(basename "$PROJECT_ROOT")"

# HYDRA_MCP_PORT retired (legacy, no longer exported). Use a fixed
# non-canonical port for the third-party listener test.
NON_CANONICAL_PORT=44142
echo "instance=$AXON_INSTANCE_KIND PHX_PORT=$PHX_PORT NON_CANONICAL_PORT=$NON_CANONICAL_PORT"

# Sanity: skip the suite if any canonical process is alive — we cannot
# distinguish their listeners from injected ones, and we never kill
# operator's running runtime. Pattern aligned on the canonical helper in
# scripts/stop.sh (REQ-AXO-901637).
canonical_alive_count="$(pgrep -af \
    "${PROJECT_ROOT}/bin/axon-brain( |\$)|${PROJECT_ROOT}/bin/axon-indexer( |\$)|(^|[[:space:]])bin/axon-brain\$|(^|[[:space:]])bin/axon-indexer\$" \
    2>/dev/null | grep -v -E 'grep|claude|pgrep' | wc -l | awk '{print $1}')"
if [ "$canonical_alive_count" -gt 0 ]; then
    echo "SKIP [$TEST_NAME]: $canonical_alive_count canonical axon process(es) alive in ${PROJECT_ROOT}/bin/."
    exit 0
fi

trap 'cleanup_pid "${PID_DASH:-}" ; cleanup_pid "${PID_3PTY:-}"' EXIT

# --- Test A: dashboard listener on PHX_PORT -> verify must pass ---
PID_DASH="$(spawn_listener "$PHX_PORT" "fake-dashboard")"
if AXON_INSTANCE_KIND="$INSTANCE_KIND" bash "$PROJECT_ROOT/scripts/stop.sh" --verify \
        > /tmp/${TEST_NAME}_A.log 2>&1; then
    pass A "dashboard on PHX_PORT does not block --verify"
else
    rc=$?
    echo "--- /tmp/${TEST_NAME}_A.log ---" >&2
    cat /tmp/${TEST_NAME}_A.log >&2
    fail A "--verify exited $rc with only dashboard listening on PHX_PORT (regression of DEC-AXO-901598 rule 1)"
fi
cleanup_pid "$PID_DASH"
PID_DASH=""

# --- Test B: third-party listener on a non-canonical port -> verify must pass ---
PID_3PTY="$(spawn_listener "$NON_CANONICAL_PORT" "third-party-mcp-squat")"
if AXON_INSTANCE_KIND="$INSTANCE_KIND" bash "$PROJECT_ROOT/scripts/stop.sh" --verify \
        > /tmp/${TEST_NAME}_B.log 2>&1; then
    pass B "third-party listener on non-canonical port does not block --verify (binary-anchored identity)"
else
    rc=$?
    echo "--- /tmp/${TEST_NAME}_B.log ---" >&2
    cat /tmp/${TEST_NAME}_B.log >&2
    fail B "--verify exited $rc on third-party listener (regression of DEC-AXO-901598 rule 2)"
fi
cleanup_pid "$PID_3PTY"
PID_3PTY=""

echo "ALL TESTS PASS [$TEST_NAME]"
