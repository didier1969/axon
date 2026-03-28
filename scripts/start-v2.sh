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
tmux kill-session -t axon 2>/dev/null || true
rm -f /tmp/axon-*.sock
rm -f .axon/graph_v2/*.db.wal 2>/dev/null || true

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
# We use 'nix develop' to ensure all WASM/AI dependencies are in path
# Optimization: Parallel start, no blocking wait for Elixir.
tmux send-keys -t axon:core "nix develop --impure --command bash -c 'while true; do echo \"🚀 Starting Axon Core...\"; RUST_LOG=info nice -n 19 ionice -c 3 bin/axon-core; EXIT_CODE=\$?; echo \"⚠️ Axon Core exited with code \$EXIT_CODE. Restarting in 2s...\"; sleep 2; done'" C-m

# Start Pod A (Control Plane)
tmux new-window -t axon -n "nexus"
# PROTECTION: Rétablissement de hex, rebar et ecto.setup (indispensables pour la stabilité post-reset)
tmux send-keys -t axon:nexus "cd src/dashboard && nix develop --impure --command bash -c \"mix local.hex --force && mix local.rebar --force && mix ecto.setup && PHX_PORT=$PHX_PORT HYDRA_TCP_PORT=$HYDRA_TCP_PORT AXON_REPO_SLUG=workspace AXON_WATCH_DIR=/home/dstadel/projects elixir --name axon_nexus@127.0.0.1 --cookie axon_secret -S mix phx.server\"" C-m

echo "⏳ Waiting for Axon Infrastructure to rise..."

# Parallel wait loop for both services
CORE_READY=false
DASHBOARD_READY=false

for i in {1..60}; do
    if [ "$CORE_READY" = false ]; then
        if [ -S "/tmp/axon-telemetry.sock" ] && nc -z localhost $HYDRA_HTTP_PORT 2>/dev/null; then
            echo "✅ Axon Data Plane is Ready."
            CORE_READY=true
        fi
    fi

    if [ "$DASHBOARD_READY" = false ]; then
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

echo ""
echo "🛡️ Axon is rising in TMUX session 'axon'."
echo "To view processes: 'tmux attach -t axon'"
echo "Dashboard: http://localhost:44127/cockpit"
echo ""
