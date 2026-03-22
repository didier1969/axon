#!/bin/bash
# Axon MCP Wrapper - Redirects to the native Rust tunnel
# This script is used by Gemini CLI to connect to the Axon Oracle.

TUNNEL_BIN="/home/dstadel/projects/axon/bin/axon-mcp-tunnel"

if [ ! -f "$TUNNEL_BIN" ]; then
    echo "Error: Axon MCP Tunnel binary not found. Please build it first." >&2
    exit 1
fi

exec "$TUNNEL_BIN"
