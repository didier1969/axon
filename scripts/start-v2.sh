#!/bin/bash
set -e

# Axon v2 - Industrial Start Script using TMUX for Resilience in WSL
PROJECT_ROOT="/home/dstadel/projects/axon"
cd "$PROJECT_ROOT"

# Clean up socket if exists
if [ -S "/tmp/axon-v2.sock" ]; then
    rm -f "/tmp/axon-v2.sock"
fi

echo "🚀 Starting Axon v2 Architecture (Managed via TMUX)..."

# Kill existing axon session if any
tmux kill-session -t axon 2>/dev/null || true

# Create new session and start Pod B (Core)
tmux new-session -d -s axon -n "core" "nix develop --impure --command bash -c 'bin/axon-core'"

# Start Pod A (Watcher)
tmux new-window -t axon:1 -n "watcher" "nix develop --impure --command bash -c 'cd src/watcher && AXON_REPO_SLUG=axon AXON_WATCH_DIR=$PROJECT_ROOT mix run --no-halt'"

# Start Control Plane (Dashboard)
tmux new-window -t axon:2 -n "dashboard" "nix develop --impure --command bash -c 'cd src/dashboard && PHX_PORT=44921 mix phx.server'"

echo "🛡️ Axon is rising in TMUX session 'axon'."
echo "To view processes: 'tmux attach -t axon'"
echo "Dashboard will be at http://localhost:44921"
