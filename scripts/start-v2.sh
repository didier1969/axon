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
rm -f "$PROJECT_ROOT/.axon/graph_v2/lbug.db.lock"

echo "🚀 Starting Axon v2 Architecture (Managed via TMUX)..."

# Kill existing axon session if any
tmux kill-session -t axon 2>/dev/null || true

# Create new session and start Pod C (HydraDB)
tmux new-session -d -s axon -n "db" "nix develop --impure --command bash -c 'axon-db-start'"

# Start Pod B (Core / Parser) with OS-level Niceness (Idle Priority for CPU and I/O)
tmux new-window -t axon:1 -n "core" "nix develop --impure --command bash -c 'exec nice -n 19 ionice -c 3 bin/axon-core'"

# Start Pod A/Control (Nexus Monolith)
tmux new-window -t axon:2 -n "nexus" "bash -c 'cd src/dashboard && nix develop --impure --command bash -c \"mix ecto.setup && PHX_PORT=44921 AXON_REPO_SLUG=axon AXON_WATCH_DIR=../../ mix phx.server\"'"

echo "🛡️ Axon is rising in TMUX session 'axon'."
echo "To view processes: 'tmux attach -t axon'"
echo "Dashboard will be at http://localhost:44921"
