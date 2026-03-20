#!/bin/bash
set -e

# Axon v2 - Industrial Start Script using TMUX for Resilience in WSL
PROJECT_ROOT="/home/dstadel/projects/axon"
cd "$PROJECT_ROOT"

# Verify nix-daemon is running (WSL2 specific mitigation)
if ! nix store info >/dev/null 2>&1; then
    echo "⚠️ Nix daemon is not responding. Attempting to start it (sudo may be required)..."
    if command -v systemctl >/dev/null && systemctl is-system-running >/dev/null 2>&1; then
        sudo systemctl start nix-daemon
    else
        sudo bash -c "/nix/var/nix/profiles/default/bin/nix-daemon --daemon &"
        sleep 2
    fi
fi

# Clean up socket if exists
if [ -S "/tmp/axon-v2.sock" ]; then
    rm -f "/tmp/axon-v2.sock"
fi

# Clean up dangling processes and database locks
echo "🧹 Cleaning up legacy locks and orphan processes..."
pkill -f "bin/axon-core" 2>/dev/null || true
pkill -f "mix phx.server" 2>/dev/null || true
pkill -f "axon-db-start" 2>/dev/null || true
pkill -f "beam.smp.*hydra_axon" 2>/dev/null || true
rm -f "$PROJECT_ROOT/.axon/graph_v2/lbug.db.lock"

# Safety to avoid 'Text file busy' if the user just built a new version
if [ -f "src/axon-core/target/release/axon-core" ]; then
    echo "🔄 Updating bin/axon-core from latest release build..."
    rm -f bin/axon-core
    cp src/axon-core/target/release/axon-core bin/axon-core
fi

echo "🚀 Starting Axon v2 Architecture (Managed via TMUX)..."

# Generate deterministic ports > 40000 to avoid collisions
export PHX_PORT=44127
export HYDRA_TCP_PORT=44128
export HYDRA_HTTP_PORT=44129
export HYDRA_ODATA_PORT=44130
export HYDRA_HTTP2_PORT=44131
export HYDRA_MCP_PORT=44132

# Kill existing axon session if any
tmux kill-session -t axon 2>/dev/null || true
sleep 1

# Create new session
tmux new-session -d -s axon -n "db" 

# Start Pod C (HydraDB)
tmux send-keys -t axon:db "nix develop --impure --command bash -c \"HYDRA_TCP_PORT=$HYDRA_TCP_PORT HYDRA_HTTP_PORT=$HYDRA_HTTP_PORT HYDRA_ODATA_PORT=$HYDRA_ODATA_PORT HYDRA_HTTP2_PORT=$HYDRA_HTTP2_PORT HYDRA_MCP_PORT=$HYDRA_MCP_PORT axon-db-start\"" C-m
sleep 2

# Start Pod B (Core / Parser) with OS-level Niceness
tmux new-window -t axon -n "core"
tmux send-keys -t axon:core "nix develop --impure --command bash -c 'exec nice -n 19 ionice -c 3 bin/axon-core'" C-m

# Start Pod A/Control (Nexus Monolith)
tmux new-window -t axon -n "nexus"
tmux send-keys -t axon:nexus "cd src/dashboard && nix develop --impure --command bash -c \"mix ecto.setup && PHX_PORT=$PHX_PORT HYDRA_TCP_PORT=$HYDRA_TCP_PORT AXON_REPO_SLUG=workspace AXON_WATCH_DIR=/home/dstadel/projects mix phx.server\"" C-m

echo "⏳ Waiting for Axon Core (Rust Data Plane) to bind UDS socket (FastEmbed initialisation up to 10s)..."
# Polling loop pour vérifier que le socket répond (Évite le MCP Disconnected pour les agents)
for i in {1..30}; do
    if [ -S "/tmp/axon-v2.sock" ] && python3 -c 'import socket; s=socket.socket(socket.AF_UNIX, socket.SOCK_STREAM); s.connect("/tmp/axon-v2.sock"); s.close()' 2>/dev/null; then
        echo "✅ Axon Bridge (UDS) is Ready."
        break
    fi
    sleep 1
    if [ "$i" -eq 30 ]; then
        echo "⚠️ Timeout waiting for Axon Bridge UDS. Check 'tmux attach -t axon' for errors."
    fi
done

echo "⏳ Waiting for Axon Dashboard (Elixir Control Plane) to boot (Database + Compilation)..."
# Polling loop pour vérifier que Phoenix a fini sa compilation et lié son port
for i in {1..60}; do
    if nc -z localhost $PHX_PORT 2>/dev/null; then
        echo "✅ Axon Dashboard is Ready."
        break
    fi
    sleep 2
    if [ "$i" -eq 60 ]; then
        echo "⚠️ Timeout waiting for Axon Dashboard. Check 'tmux attach -t axon:nexus' for compilation errors."
    fi
done

echo ""
echo "⚙️ Running MCP End-to-End Verification..."
if ! python3 scripts/e2e_mcp_test.py; then
    echo "❌ FATAL: MCP Proxy failed End-to-End verification. The AI client will NOT be able to connect."
    echo "Check 'python3 scripts/mcp-stdio-proxy.py' for import errors or dependencies."
    exit 1
fi
echo ""

echo "🛡️ Axon is rising in TMUX session 'axon'."
echo "To view processes: 'tmux attach -t axon'"
echo ""

# Run the unified health check to show the state
if [ -x "bin/axol" ]; then
    ./bin/axol
fi
