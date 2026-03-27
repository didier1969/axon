#!/bin/bash

# Axon v2 - Industrial Precision Stop Script
# Kills ONLY Axon-related processes to avoid interfering with other projects.

PROJECT_ROOT="/home/dstadel/projects/axon"

echo "🛑 Stopping Axon v2 Architecture (Chirurgical Mode)..."

# 1. Graceful Elixir Shutdown via RPC (If node is named)
if command -v elixir >/dev/null 2>&1; then
    echo "Sending shutdown signal to Axon Nexus node..."
    # We try to stop the named node properly
    elixir --name stop_script@127.0.0.1 --cookie axon_secret --rpc "axon_nexus@127.0.0.1" :init :stop >/dev/null 2>&1 || true
    sleep 1
fi

# 2. Kill lingering processes by pattern and PGID
# Targeted patterns to avoid killing other beam.smp or unrelated tools
PIDS=$(pgrep -f "AXON_REPO_SLUG=workspace|bin/axon-core|bin/axon-mcp-tunnel|axon-db-start|beam.smp.*axon_nexus")

if [ ! -z "$PIDS" ]; then 
    echo "Cleaning up lingering Axon processes: $PIDS"
    kill -15 $PIDS 2>/dev/null || true
    sleep 1
    kill -9 $PIDS 2>/dev/null || true
fi

# 3. Close TMUX session
if tmux has-session -t axon 2>/dev/null; then
    echo "Closing TMUX session 'axon'..."
    tmux kill-session -t axon 2>/dev/null || true
fi

# 4. Clean up sockets and locks
rm -f "/tmp/axon-mcp.sock"
rm -f "/tmp/axon-telemetry.sock"
rm -f "/tmp/axon-v2.sock"
rm -f "$PROJECT_ROOT/.axon/graph_v2/lbug.db.lock"

echo "✅ Axon stopped (Other projects preserved)."
