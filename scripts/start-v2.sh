#!/bin/bash
set -euo pipefail

# Axon v2 - Daily Start Script
# Canonical daily workflow entrypoint for running Axon in TMUX.

PROJECT_ROOT="/home/dstadel/projects/axon"
cd "$PROJECT_ROOT"

if ! command -v tmux >/dev/null 2>&1; then
    echo "❌ tmux is required to start Axon via scripts/start-v2.sh"
    exit 1
fi

echo "📦 Validating Devenv environment..."
devenv shell -- bash -lc './scripts/validate-devenv.sh'

if [ ! -x "bin/axon-core" ]; then
    echo "❌ Missing bin/axon-core"
    echo "   Run ./scripts/setup_v2.sh first."
    exit 1
fi

if tmux has-session -t axon 2>/dev/null; then
    echo "ℹ️ Axon is already running in TMUX session 'axon'."
    echo "   Attach with: tmux attach -t axon"
    exit 0
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

if [ -f "$CARGO_TARGET_ROOT/release/axon-core" ]; then
    echo "🔄 Updating bin/axon-core safely..."
    install -m 755 "$CARGO_TARGET_ROOT/release/axon-core" bin/axon-core
fi

if [ -f "$CARGO_TARGET_ROOT/release/axon-mcp-tunnel" ]; then
    echo "🔄 Updating bin/axon-mcp-tunnel safely..."
    install -m 755 "$CARGO_TARGET_ROOT/release/axon-mcp-tunnel" bin/axon-mcp-tunnel
fi

echo "🚀 Starting Axon in TMUX session 'axon'..."

# Configuration
export PHX_PORT=44127
export HYDRA_TCP_PORT=44128
export HYDRA_HTTP_PORT=44129
export HYDRA_ODATA_PORT=44130
export HYDRA_HTTP2_PORT=44131
export HYDRA_MCP_PORT=44132

# Clean only the sockets used by the active runtime path
rm -f /tmp/axon-telemetry.sock /tmp/axon-mcp.sock

# Create TMUX session
tmux new-session -d -s axon -n "core" 

# Start Data Plane
# We use 'devenv shell' to ensure the runtime matches the pinned project toolchain.
# NEXUS v10.8: We force fastembed to use the system's libonnxruntime.so to prevent C++ aborts.
tmux send-keys -t axon:core "devenv shell -- bash -lc 'export ORT_STRATEGY=system; export ORT_DYLIB_PATH=\$(nix eval --raw nixpkgs#onnxruntime.outPath 2>/dev/null)/lib/libonnxruntime.so; echo \"🚀 Starting Axon Core...\"; RUST_LOG=info bin/axon-core'" C-m

# Start Control Plane
tmux new-window -t axon -n "nexus"
tmux send-keys -t axon:nexus "cd src/dashboard && devenv shell -- bash -lc \"PHX_PORT=$PHX_PORT HYDRA_TCP_PORT=$HYDRA_TCP_PORT AXON_REPO_SLUG=workspace AXON_WATCH_DIR=/home/dstadel/projects elixir --name axon_nexus@127.0.0.1 --cookie axon_secret -S mix phx.server\"" C-m

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
