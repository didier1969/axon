#!/bin/bash
set -euo pipefail

# Axon v2 - Daily Start Script
# Canonical daily workflow entrypoint for running Axon in TMUX.

PROJECT_ROOT="/home/dstadel/projects/axon"
DEFAULT_PROJECTS_ROOT="/home/dstadel/projects"
cd "$PROJECT_ROOT"
WATCH_ROOT="${AXON_WATCH_DIR:-$DEFAULT_PROJECTS_ROOT}"
PROJECTS_ROOT="${AXON_PROJECTS_ROOT:-$WATCH_ROOT}"
REPO_SLUG="${AXON_REPO_SLUG:-$(basename "$PROJECT_ROOT")}"

if ! command -v tmux >/dev/null 2>&1; then
    echo "❌ tmux is required to start Axon via scripts/start-v2.sh"
    exit 1
fi

echo "📦 Validating Devenv environment..."
devenv shell -- bash -lc './scripts/validate-devenv.sh'

echo "📦 Pre-warming Elixir environment (Hex/Rebar)..."
devenv shell -- bash -lc "cd '$PROJECT_ROOT/src/dashboard' && mix local.hex --force >/dev/null && mix local.rebar --force >/dev/null"

if [ ! -x "bin/axon-core" ]; then
    echo "❌ Missing bin/axon-core"
    echo "   Run ./scripts/setup_v2.sh first."
    exit 1
fi

if tmux has-session -t axon 2>/dev/null; then
    DELETED_EXE_PIDS=$(for pid in $(pgrep -f "$PROJECT_ROOT/bin/axon-core" || true); do
        exe=$(readlink -f "/proc/$pid/exe" 2>/dev/null || true)
        if [[ "$exe" == *"(deleted)"* ]]; then
            echo "$pid"
        fi
    done)

    if [ -n "${DELETED_EXE_PIDS:-}" ]; then
        echo "⚠️ Found Axon processes still running on deleted executables: $DELETED_EXE_PIDS"
        echo "   Resetting stale runtime state before restart..."
        bash "$PROJECT_ROOT/scripts/stop-v2.sh"
    fi

    if nc -z localhost 44129 2>/dev/null || [ -S "/tmp/axon-telemetry.sock" ]; then
        echo "ℹ️ Axon is already running in TMUX session 'axon'."
        echo "   Attach with: tmux attach -t axon"
        exit 0
    fi

    echo "⚠️ Found stale TMUX session 'axon' without a healthy data plane. Resetting local runtime state..."
    tmux kill-session -t axon 2>/dev/null || true
fi

# 1. Verify nix-daemon is running (WSL2 specific mitigation)
if ! nix store info >/dev/null 2>&1; then
    echo "⚠️ Nix daemon is not responding. Attempting to start it..."
    if command -v systemctl >/dev/null && systemctl is-system-running >/dev/null 2>&1; then
        sudo systemctl start nix-daemon
    else
        sudo bash -c "/nix/var/nix/profiles/default/bin/nix-daemon --daemon &"
        sleep 2
    fi
fi

# 2. Synchronize binaries (handle 'Text file busy' via install)
CARGO_TARGET_ROOT="${CARGO_TARGET_DIR:-$PROJECT_ROOT/.axon/cargo-target}"
LEGACY_RELEASE_BIN="$PROJECT_ROOT/src/axon-core/target/release/axon-core"
DEVENV_RELEASE_BIN="$CARGO_TARGET_ROOT/release/axon-core"

rebuild_core_release() {
    echo "🔧 Rebuilding axon-core release inside Devenv..."
    if ! devenv shell -- bash -lc "cd '$PROJECT_ROOT/src/axon-core' && cargo build --release"; then
        echo "❌ Automatic Devenv rebuild failed."
        return 1
    fi
    return 0
}

verify_sql_gateway() {
    local response
    response="$(curl -sS -X POST http://127.0.0.1:$HYDRA_HTTP_PORT/sql \
        -H 'content-type: application/json' \
        -d '{"query":"SHOW TABLES"}' 2>/dev/null || true)"

    if [[ -z "$response" ]]; then
        echo "❌ SQL Gateway did not answer the schema probe."
        return 1
    fi

    for table in File Symbol RuntimeMetadata; do
        if [[ "$response" != *"\"$table\""* ]]; then
            echo "❌ SQL Gateway is up but missing required table '$table'."
            echo "   Response: $response"
            return 1
        fi
    done

    return 0
}

if [ -f "$LEGACY_RELEASE_BIN" ] && [ ! -f "$DEVENV_RELEASE_BIN" ]; then
    echo "⚠️ Found a release build outside Devenv at $LEGACY_RELEASE_BIN"
    echo "   Axon starts from $DEVENV_RELEASE_BIN. Attempting automatic rebuild..."
    rebuild_core_release || exit 1
fi

if [ -f "$LEGACY_RELEASE_BIN" ] && [ "$LEGACY_RELEASE_BIN" -nt "$DEVENV_RELEASE_BIN" ]; then
    echo "⚠️ Detected a newer release binary outside Devenv:"
    echo "   $LEGACY_RELEASE_BIN"
    echo "   Attempting to refresh the authoritative Devenv build..."
    rebuild_core_release || exit 1
fi

if [ ! -f "$DEVENV_RELEASE_BIN" ]; then
    echo "⚠️ Missing Devenv release binary at $DEVENV_RELEASE_BIN"
    rebuild_core_release || exit 1
fi

if [ -f "$DEVENV_RELEASE_BIN" ]; then
    echo "🔄 Updating bin/axon-core safely..."
    install -m 755 "$DEVENV_RELEASE_BIN" bin/axon-core
fi

if [ -f "$CARGO_TARGET_ROOT/release/axon-mcp-tunnel" ]; then
    echo "🔄 Updating bin/axon-mcp-tunnel safely..."
    install -m 755 "$CARGO_TARGET_ROOT/release/axon-mcp-tunnel" bin/axon-mcp-tunnel
fi

echo "🚀 Starting Axon in TMUX session 'axon'..."
echo "📂 Watch root: $WATCH_ROOT"
echo "🗂️ Projects root: $PROJECTS_ROOT"

# Configuration
export PHX_PORT=44127
export HYDRA_TCP_PORT=44128
export HYDRA_HTTP_PORT=44129
export HYDRA_ODATA_PORT=44130
export HYDRA_HTTP2_PORT=44131
export HYDRA_MCP_PORT=44132

# Clean only the sockets used by the active runtime path
rm -f /tmp/axon-telemetry.sock /tmp/axon-mcp.sock
rm -f "$PROJECT_ROOT/.axon/graph_v2/"*.wal "$PROJECT_ROOT/.axon/graph_v2/"*.lock 2>/dev/null || true

# Create TMUX session
tmux new-session -d -s axon -n "core" 

# Start Data Plane
# We use 'devenv shell' to ensure the runtime matches the pinned project toolchain.
# NEXUS v10.8: We force fastembed to use the system's libonnxruntime.so to prevent C++ aborts.
tmux send-keys -t axon:core "devenv shell -- bash -lc 'export AXON_PROJECTS_ROOT=\"$PROJECTS_ROOT\"; export AXON_PROJECT_ROOT=\"$PROJECT_ROOT\"; export ORT_STRATEGY=system; export ORT_DYLIB_PATH=\$(nix eval --raw nixpkgs#onnxruntime.outPath 2>/dev/null)/lib/libonnxruntime.so; echo \"🚀 Starting Axon Core...\"; RUST_LOG=info bin/axon-core'" C-m

# Start Control Plane
tmux new-window -t axon -n "nexus"
tmux send-keys -t axon:nexus "cd \"$PROJECT_ROOT\" && devenv shell -- bash -lc \"cd '$PROJECT_ROOT/src/dashboard' && PHX_PORT=$PHX_PORT HYDRA_TCP_PORT=$HYDRA_TCP_PORT AXON_REPO_SLUG=$REPO_SLUG AXON_WATCH_DIR=$WATCH_ROOT elixir --name axon_nexus@127.0.0.1 --cookie axon_secret -S mix phx.server\"" C-m

echo "⏳ Waiting for Axon Infrastructure to rise (Timeout: 60s)..."

# Parallel wait loop for both services
CORE_READY=false
DASHBOARD_READY=false

# Wait up to 120 * 0.5s = 60s
for i in {1..120}; do
    if [ "$CORE_READY" = false ]; then
        # Core is ready if the telemetry socket exists AND the MCP port is responding
        if [ -S "/tmp/axon-telemetry.sock" ] || nc -z localhost $HYDRA_HTTP_PORT 2>/dev/null; then
            echo "✅ Axon Data Plane is Ready."
            CORE_READY=true
        fi
    fi

    if [ "$DASHBOARD_READY" = false ]; then
        # Dashboard is ready if the Phoenix port is responding
        if nc -z localhost $PHX_PORT 2>/dev/null; then
            echo "✅ Axon Dashboard is Ready."
            DASHBOARD_READY=true
        fi
    fi

    if [ "$CORE_READY" = true ] && [ "$DASHBOARD_READY" = true ]; then
        break
    fi
    
    sleep 0.5
done

if [ "$CORE_READY" = false ]; then echo "⚠️ Timeout waiting for Axon Core."; fi
if [ "$DASHBOARD_READY" = false ]; then echo "⚠️ Timeout waiting for Axon Dashboard."; fi

if [ "$CORE_READY" = true ]; then
    echo ""
    echo "🧪 Verifying live SQL schema..."
    if ! verify_sql_gateway; then
        echo "❌ Axon Core exposed its port but failed the live schema check."
        echo "   Inspect TMUX with: tmux attach -t axon"
        exit 1
    fi
    echo "✅ Live SQL schema check succeeded."
fi

echo ""
echo "⚙️ Running MCP End-to-End Verification..."
if [ -x "bin/axon-mcp-tunnel" ] && ! echo '{"jsonrpc": "2.0", "method": "tools/list", "params": {}, "id": 1}' | bin/axon-mcp-tunnel | grep -q "axon_query"; then
    echo "❌ MCP tunnel verification failed."
    echo "   Inspect the TMUX session to debug."
elif [ -x "bin/axon-mcp-tunnel" ]; then
    echo "✅ MCP tunnel verification succeeded."
else
    echo "ℹ️ Skipping MCP tunnel verification because bin/axon-mcp-tunnel is not available."
fi

# 6. Final Report
WSL_IP=$(ip addr show eth0 | grep "inet " | awk '{print $2}' | cut -d/ -f1)
if [ -z "$WSL_IP" ]; then WSL_IP="127.0.0.1"; fi

echo ""
echo "🛡️ Axon is rising in TMUX session 'axon'."
echo "To view processes: 'tmux attach -t axon'"
echo "Dashboard: http://$WSL_IP:44127/cockpit"
echo "SQL Gateway: http://$WSL_IP:44129/sql"
echo "MCP Server: http://$WSL_IP:44129/mcp"
echo "Stop services with: ./scripts/stop-v2.sh"
echo ""
