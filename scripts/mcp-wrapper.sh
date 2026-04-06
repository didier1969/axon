#!/bin/bash
set -euo pipefail
# Axon MCP Wrapper - Fast Tunneling
# This script is used by LLM clients to connect to the Axon Oracle.
# It does NOT attempt to auto-boot the server to avoid MCP handshake timeouts.
# The underlying Rust tunnel handles offline states gracefully via JSON-RPC errors.

PROJECT_ROOT="/home/dstadel/projects/axon"
TUNNEL_BIN="$PROJECT_ROOT/bin/axon-mcp-tunnel"

# 1. Verification of tunnel binary
if [ ! -f "$TUNNEL_BIN" ]; then
    echo "Error: Axon MCP Tunnel binary not found. Please build the project first." >&2
    exit 1
fi

# 2. Execute the tunnel (Instant relay)
exec "$TUNNEL_BIN"
