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

# Start Pod A (Watcher)
tmux new-window -t axon:2 -n "watcher" "bash -c 'cd src/watcher && nix develop --impure --command bash -c \"mix ecto.migrate && AXON_REPO_SLUG=axon AXON_WATCH_DIR=../../ elixir --name watcher@127.0.0.1 --cookie axon_v2_cluster -S mix run --no-halt\"'"

# Start Control Plane (Dashboard)
tmux new-window -t axon:3 -n "dashboard" "bash -c 'cd src/dashboard && nix develop --impure --command bash -c \"PHX_PORT=44921 elixir --name dashboard@127.0.0.1 --cookie axon_v2_cluster -S mix phx.server\"'"

echo "🛡️ Axon is rising in TMUX session 'axon'."
echo "To view processes: 'tmux attach -t axon'"
echo "Dashboard will be at http://localhost:44921"
