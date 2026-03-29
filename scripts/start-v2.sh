#!/bin/bash
set -e

# Axon v2 - Industrial Start Script
# Ensures a clean state by calling stop first, then launches everything in TMUX.

PROJECT_ROOT="/home/dstadel/projects/axon"
cd "$PROJECT_ROOT"

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
if [ -f "src/axon-core/target/release/axon-core" ]; then
    echo "🔄 Updating bin/axon-core safely..."
    install -m 755 src/axon-core/target/release/axon-core bin/axon-core
fi

if [ -f "src/axon-mcp-tunnel/target/release/axon-mcp-tunnel" ]; then
    echo "🔄 Updating bin/axon-mcp-tunnel safely..."
    install -m 755 src/axon-mcp-tunnel/target/release/axon-mcp-tunnel bin/axon-mcp-tunnel
fi

echo "🚀 Starting Axon v2 Architecture (Managed via TMUX)..."

# 3. Clean environment (Safety Protocol)
echo "🧹 Cleaning TMUX sessions and Socket locks..."
tmux kill-session -t axon 2>/dev/null || true
# If TMUX server is unstable, we reset it
if ! tmux ls >/dev/null 2>&1; then
    tmux kill-server 2>/dev/null || true
fi

rm -f /tmp/axon-*.sock
rm -f .axon/graph_v2/*.db.wal 2>/dev/null || true

echo "🔓 Releasing Database locks..."
fuser -k .axon/graph_v2/ist.db 2>/dev/null || true
fuser -k .axon/graph_v2/sanctuary/soll.db 2>/dev/null || true

# Configuration
export PHX_PORT=44127
export HYDRA_TCP_PORT=44128
export HYDRA_HTTP_PORT=44129
export HYDRA_ODATA_PORT=44130
export HYDRA_HTTP2_PORT=44131
export HYDRA_MCP_PORT=44132

# Create new TMUX session
tmux new-session -d -s axon -n "core" 

# Start Pod B (Data Plane)
# We use 'devenv shell' to ensure the runtime matches the pinned project toolchain.
# NEXUS v10.8: We force fastembed to use the system's libonnxruntime.so to prevent C++ aborts.
tmux send-keys -t axon:core "devenv shell -- bash -lc 'export ORT_STRATEGY=system; export ORT_DYLIB_PATH=\$(nix eval --raw nixpkgs#onnxruntime.outPath 2>/dev/null)/lib/libonnxruntime.so; while true; do echo \"🚀 Starting Axon Core...\"; RUST_LOG=info bin/axon-core; EXIT_CODE=\$?; echo \"⚠️ Axon Core exited with code \$EXIT_CODE. Restarting in 2s...\"; sleep 2; done'" C-m

# Start Pod A (Control Plane)
tmux new-window -t axon -n "nexus"
# PROTECTION: Rétablissement de hex, rebar et ecto.setup (indispensables pour la stabilité post-reset)
tmux send-keys -t axon:nexus "cd src/dashboard && devenv shell -- bash -lc \"mix local.hex --force && mix local.rebar --force && mix compile && mix ecto.setup && PHX_PORT=$PHX_PORT HYDRA_TCP_PORT=$HYDRA_TCP_PORT AXON_REPO_SLUG=workspace AXON_WATCH_DIR=/home/dstadel/projects elixir --name axon_nexus@127.0.0.1 --cookie axon_secret -S mix phx.server\"" C-m

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
if ! echo '{"jsonrpc": "2.0", "method": "tools/list", "params": {}, "id": 1}' | bin/axon-mcp-tunnel | grep -q "axon_query"; then
    echo "❌ FATAL: MCP Tunnel failed End-to-End verification."
    # We don't exit here to allow user to debug in tmux
else
    echo "✅ E2E Verification Success! System is healthy."
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
echo ""
