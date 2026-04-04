#!/bin/bash
set -euo pipefail
# Axon MCP Wrapper - Auto-Booting & Tunneling
# This script is used by Gemini CLI to connect to the Axon Oracle.

PROJECT_ROOT="/home/dstadel/projects/axon"
TUNNEL_BIN="$PROJECT_ROOT/bin/axon-mcp-tunnel"
CORE_BIN="$PROJECT_ROOT/bin/axon-core"
START_SCRIPT="$PROJECT_ROOT/scripts/start.sh"

# 1. Verification of binaries
if [ ! -f "$CORE_BIN" ]; then
    echo "Error: Axon Core binary not found. Please build it first." >&2
    exit 1
fi

if [ ! -f "$START_SCRIPT" ]; then
    echo "Error: start.sh not found at $START_SCRIPT" >&2
    exit 1
fi

# 2. Auto-Boot Mechanism
# Check if Axon Core is listening on the HTTP port 44129
if ! nc -z 127.0.0.1 44129 2>/dev/null; then
    # Server is down, auto-start it with the canonical startup pipeline
    cd "$PROJECT_ROOT"
    "$START_SCRIPT" --mcp-only --no-dashboard >/dev/null 2>&1 || true

    # Wait up to 120 seconds for the port to bind
    for i in {1..120}; do
        if nc -z 127.0.0.1 44129 2>/dev/null; then
            break
        fi
        sleep 1
    done
fi

if [ ! -f "$TUNNEL_BIN" ]; then
    echo "Error: Axon MCP Tunnel binary not found after startup. Run scripts/start.sh once." >&2
    exit 1
fi

# 3. Execute the tunnel
exec "$TUNNEL_BIN"
