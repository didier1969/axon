#!/bin/bash
set -euo pipefail

# Axon v2 - Industrial Precision Stop Script
# Kills ONLY Axon-related processes to avoid interfering with other projects.

PROJECT_ROOT="/home/dstadel/projects/axon"
REPO_SLUG="${AXON_REPO_SLUG:-$(basename "$PROJECT_ROOT")}"

wait_for_exit() {
    local pattern="$1"
    for _ in {1..20}; do
        if ! pgrep -f "$pattern" >/dev/null 2>&1; then
            return 0
        fi
        sleep 0.25
    done
    return 1
}

echo "🛑 Stopping Axon v2 Architecture (Chirurgical Mode)..."

# 1. Graceful Elixir Shutdown via RPC (If node is named)
if command -v elixir >/dev/null 2>&1; then
    echo "Sending shutdown signal to Axon Nexus node..."
    # We try to stop the named node properly
    elixir --name stop_script@127.0.0.1 --cookie axon_secret --rpc "axon_nexus@127.0.0.1" :init :stop >/dev/null 2>&1 || true
    sleep 1
fi

# 2. Close TMUX session first so pane-owned processes lose their supervisor
if tmux has-session -t axon 2>/dev/null; then
    echo "Closing TMUX session 'axon'..."
    tmux kill-session -t axon 2>/dev/null || true
fi

# 3. Kill lingering processes by exact project patterns
PATTERN="$PROJECT_ROOT/bin/axon-core|$PROJECT_ROOT/bin/axon-mcp-tunnel|beam.smp.*axon_nexus|AXON_REPO_SLUG=$REPO_SLUG|AXON_REPO_SLUG=workspace"
PIDS=$(pgrep -f "$PATTERN" || true)

if [ -n "${PIDS:-}" ]; then
    echo "Cleaning up lingering Axon processes: $PIDS"
    kill -15 $PIDS 2>/dev/null || true
    if ! wait_for_exit "$PATTERN"; then
        kill -9 $PIDS 2>/dev/null || true
        wait_for_exit "$PATTERN" || true
    fi
fi

# 4. Clean up sockets and locks
echo "Cleaning up sockets, ports and locks..."
fuser -k 44127/tcp 44128/tcp 44129/tcp 44130/tcp 44131/tcp 44132/tcp 2>/dev/null || true
fuser -k /tmp/axon-telemetry.sock /tmp/axon-mcp.sock 2>/dev/null || true
rm -f "/tmp/axon-mcp.sock"
rm -f "/tmp/axon-telemetry.sock"
rm -f "/tmp/axon-v2.sock"
rm -f "$PROJECT_ROOT/.axon/graph_v2/"*.wal
rm -f "$PROJECT_ROOT/.axon/graph_v2/"*.lock

if tmux has-session -t axon 2>/dev/null; then
    echo "⚠️ TMUX session 'axon' still present after cleanup."
    exit 1
fi

if pgrep -f "$PATTERN" >/dev/null 2>&1; then
    echo "⚠️ Axon-related processes still running after cleanup."
    pgrep -af "$PATTERN" || true
    exit 1
fi

echo "✅ Axon stopped (Other projects preserved)."
