#!/bin/bash

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUN_DIR="$PROJECT_ROOT/.axon/run"

RUST_PID_FILE="$RUN_DIR/rust.pid"
ELIXIR_PID_FILE="$RUN_DIR/elixir.pid"

echo "🛑 Stopping Axon v2 Architecture..."

if [ -f "$ELIXIR_PID_FILE" ]; then
    PID=$(cat "$ELIXIR_PID_FILE")
    if ps -p "$PID" > /dev/null 2>&1; then
        echo "Killing Elixir Dashboard (PID $PID)..."
        kill -TERM "$PID" 2>/dev/null || kill -9 "$PID" 2>/dev/null || true
    fi
    rm -f "$ELIXIR_PID_FILE"
fi

if [ -f "$RUST_PID_FILE" ]; then
    PID=$(cat "$RUST_PID_FILE")
    if ps -p "$PID" > /dev/null 2>&1; then
        echo "Killing Rust Data Plane (PID $PID)..."
        kill -TERM "$PID" 2>/dev/null || kill -9 "$PID" 2>/dev/null || true
    fi
    rm -f "$RUST_PID_FILE"
fi

# Clean up socket
rm -f "/tmp/axon-v2.sock"

# Backup cleanup for orphan processes
echo "Cleaning up orphan processes..."
ps aux | grep axon-core | grep -v grep | awk '{print $2}' | xargs kill -9 2>/dev/null || true
ps aux | grep "mix phx.server" | grep -v grep | awk '{print $2}' | xargs kill -9 2>/dev/null || true
ps aux | grep "beam.smp" | grep "phx.server" | grep -v grep | awk '{print $2}' | xargs kill -9 2>/dev/null || true

echo "✅ Axon stopped."
