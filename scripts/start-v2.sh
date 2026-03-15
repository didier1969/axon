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

# Create new session and start Pod C (HydraDB)
tmux new-session -d -s axon -n "db" "nix develop --impure --command bash -c 'axon-db-start'"

# Start Pod B (Core / Parser)
tmux new-window -t axon:1 -n "core" "nix develop --impure --command bash -c 'bin/axon-core'"

# Start Pod A/Control (Nexus Monolith)
tmux new-window -t axon:2 -n "nexus" "bash -c 'cd src/dashboard && nix develop --impure --command bash -c \"mix ecto.setup && PHX_PORT=44921 AXON_REPO_SLUG=axon AXON_WATCH_DIR=../../ mix phx.server\"'"

echo "🛡️ Axon is rising in TMUX session 'axon'."
echo "To view processes: 'tmux attach -t axon'"
echo "Dashboard will be at http://localhost:44921"
