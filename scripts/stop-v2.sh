#!/bin/bash

# Axon v2 - Industrial Precision Stop Script
# Kills ONLY Axon-related processes to avoid interfering with other projects.

echo "🛑 Stopping Axon v2 Architecture (Chirurgical Mode)..."

# 1. Kill by PGID/Command Pattern (The safest way)
PGID_CORE=$(pgrep -f "bin/axon-core")
PGID_TUNNEL=$(pgrep -f "bin/axon-mcp-tunnel")
PGID_NEXUS=$(pgrep -f "AXON_REPO_SLUG=axon")

if [ ! -z "$PGID_CORE" ]; then 
    echo "Killing Axon Core ($PGID_CORE)..."
    kill -9 $PGID_CORE 2>/dev/null || true
fi

if [ ! -z "$PGID_TUNNEL" ]; then 
    echo "Killing Axon MCP Tunnel ($PGID_TUNNEL)..."
    kill -9 $PGID_TUNNEL 2>/dev/null || true
fi

if [ ! -z "$PGID_NEXUS" ]; then 
    echo "Killing Axon Nexus Dashboard ($PGID_NEXUS)..."
    kill -9 $PGID_NEXUS 2>/dev/null || true
fi

# 2. Close TMUX session
if tmux has-session -t axon 2>/dev/null; then
    echo "Closing TMUX session 'axon'..."
    tmux kill-session -t axon 2>/dev/null || true
fi

# 3. Clean up sockets
rm -f "/tmp/axon-mcp.sock"
rm -f "/tmp/axon-telemetry.sock"
rm -f "/tmp/axon-v2.sock"

echo "✅ Axon stopped (Other projects preserved)."
