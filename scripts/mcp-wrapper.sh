#!/bin/bash
# Axon MCP Wrapper - Auto-Booting & Tunneling
# This script is used by Gemini CLI to connect to the Axon Oracle.

PROJECT_ROOT="/home/dstadel/projects/axon"
TUNNEL_BIN="$PROJECT_ROOT/bin/axon-mcp-tunnel"
CORE_BIN="$PROJECT_ROOT/bin/axon-core"

# 1. Verification of binaries
if [ ! -f "$TUNNEL_BIN" ]; then
    echo "Error: Axon MCP Tunnel binary not found. Please build it first." >&2
    exit 1
fi

if [ ! -f "$CORE_BIN" ]; then
    echo "Error: Axon Core binary not found. Please build it first." >&2
    exit 1
fi

# 2. Auto-Boot Mechanism
# Check if Axon Core is listening on the HTTP port 44129
if ! nc -z 127.0.0.1 44129 2>/dev/null; then
    # Server is down, auto-start it in background
    rm -f "$PROJECT_ROOT/.axon/graph_v2/ist.db.wal" "$PROJECT_ROOT/.axon/graph_v2/ist.db" 2>/dev/null
    cd "$PROJECT_ROOT" && RUST_LOG=info "$CORE_BIN" > /dev/null 2>&1 &
    
    # Wait up to 5 seconds for the port to bind
    for i in {1..50}; do
        if nc -z 127.0.0.1 44129 2>/dev/null; then
            break
        fi
        sleep 0.1
    done
fi

# 3. Execute the tunnel
exec "$TUNNEL_BIN"
