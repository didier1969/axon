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
rm -f "$PROJECT_ROOT/.axon/graph_v2/lbug.db.lock"

echo "🚀 Starting Axon v2 Architecture (Managed via TMUX)..."

# Generate random ports to avoid collisions
export PHX_PORT=$((40000 + RANDOM % 1000))
export HYDRA_TCP_PORT=$((41000 + RANDOM % 1000))
export HYDRA_HTTP_PORT=$((42000 + RANDOM % 1000))
export HYDRA_ODATA_PORT=$((43000 + RANDOM % 1000))
export HYDRA_HTTP2_PORT=$((44000 + RANDOM % 1000))
export HYDRA_MCP_PORT=$((45000 + RANDOM % 1000))

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
sleep 2

# Start Pod A/Control (Nexus Monolith)
tmux new-window -t axon -n "nexus"
tmux send-keys -t axon:nexus "cd src/dashboard && nix develop --impure --command bash -c \"mix ecto.setup && PHX_PORT=$PHX_PORT HYDRA_TCP_PORT=$HYDRA_TCP_PORT AXON_REPO_SLUG=workspace AXON_WATCH_DIR=/home/dstadel/projects mix phx.server\"" C-m

echo "🛡️ Axon is rising in TMUX session 'axon'."
echo "To view processes: 'tmux attach -t axon'"
echo "Dashboard: http://localhost:$PHX_PORT"
echo "HydraDB TCP: $HYDRA_TCP_PORT"
echo "HydraDB Services: TCP:$HYDRA_TCP_PORT, HTTP:$HYDRA_HTTP_PORT, MCP:$HYDRA_MCP_PORT"
