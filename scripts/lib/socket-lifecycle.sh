#!/usr/bin/env bash
# REQ-AXO-093 socket lifecycle helpers — shared by scripts/start.sh and
# scripts/stop.sh and exercised by scripts/test_axon_socket_lifecycle.sh.
#
# axon_socket_responds: real AF_UNIX liveness probe (bare `[[ -S file ]]`
# misreads orphan socket files left from crashes as a live data plane).
#
# axon_cleanup_role_state / axon_cleanup_legacy_instance_paths: unlink the
# AF_UNIX sockets, pid file, and runtime.env that axonctl stop kills the
# processes for but does not always remove. Leftover sockets cause the
# next start to misread "data plane already up" and silently skip launch.

axon_socket_responds() {
    local sock_path="$1"
    [[ -S "$sock_path" ]] || return 1
    python3 - "$sock_path" <<'PYEOF' 2>/dev/null
import socket, sys
s = socket.socket(socket.AF_UNIX)
s.settimeout(0.5)
try:
    s.connect(sys.argv[1])
    s.close()
except Exception:
    sys.exit(1)
PYEOF
}

axon_cleanup_role_state() {
    local instance_kind="$1"
    local role="$2"
    local run_root_base="$3"
    rm -f "/tmp/axon-${instance_kind}-${role}-telemetry.sock" \
          "/tmp/axon-${instance_kind}-${role}-mcp.sock" \
          "$run_root_base/run-${role}/axon-${role}.pid" \
          "$run_root_base/run-${role}/runtime.env"
}

axon_cleanup_legacy_instance_paths() {
    local instance_kind="$1"
    rm -f "/tmp/axon-${instance_kind}-telemetry.sock" \
          "/tmp/axon-${instance_kind}-mcp.sock"
}
