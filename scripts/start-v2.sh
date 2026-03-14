#!/bin/bash
set -e

# Axon v2 - Local Development Start Script
# With Daemon Guard

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUN_DIR="$PROJECT_ROOT/.axon/run"

mkdir -p "$RUN_DIR"

RUST_PID_FILE="$RUN_DIR/rust.pid"
ELIXIR_PID_FILE="$RUN_DIR/elixir.pid"

echo "🚀 Starting Axon v2 Architecture..."

# --- DAEMON GUARD ---
check_pid() {
    local pid_file=$1
    local name=$2
    if [ -f "$pid_file" ]; then
        local pid=$(cat "$pid_file")
        if ps -p "$pid" > /dev/null 2>&1; then
            echo "⚠️ $name is already running (PID: $pid). Initiating automated shutdown..."
            "$PROJECT_ROOT/scripts/stop-v2.sh"
            sleep 2
        else
            echo "⚠️ Found stale PID file for $name ($pid). Cleaning up..."
            rm -f "$pid_file"
        fi
    fi
}

check_pid "$RUST_PID_FILE" "Axon Core (Rust)"
check_pid "$ELIXIR_PID_FILE" "Axon Dashboard (Elixir)"

# Clean up socket if exists
if [ -S "/tmp/axon-v2.sock" ]; then
    echo "⚠️ Removing stale Unix socket /tmp/axon-v2.sock"
    rm -f "/tmp/axon-v2.sock"
fi

# 1. Check if binary exists
if [ ! -f "$PROJECT_ROOT/bin/axon-core" ]; then
    echo "❌ axon-core binary not found. Please run ./scripts/setup_v2.sh first."
    exit 1
fi

# 2. Start the Rust Data Plane in background with Telemetry
echo "⚡ Starting Data Plane (axon-core) with Telemetry..."
export RUST_LOG=info
"$PROJECT_ROOT/bin/axon-core" > "$PROJECT_ROOT/logs/axon-core-live.log" 2>&1 &
RUST_PID=$!
echo $RUST_PID > "$RUST_PID_FILE"
echo "   -> Telemetry running: 'tail -f logs/axon-core-live.log' to trace engine."

# 3. Start the Elixir Dashboard
echo "💧 Starting Control Plane (Elixir Dashboard) on port 44921..."
cd "$PROJECT_ROOT/src/dashboard"

# Stop background processes on exit
cleanup() {
    echo "🛑 Stopping Axon..."
    if [ -n "$RUST_PID" ]; then
        kill $RUST_PID 2>/dev/null || true
    fi
    rm -f "$RUST_PID_FILE" "$ELIXIR_PID_FILE"
    exit 0
}

trap cleanup SIGINT SIGTERM

# Start Phoenix server
PORT=44921 mix phx.server &
ELIXIR_PID=$!
echo $ELIXIR_PID > "$ELIXIR_PID_FILE"

# Wait for Elixir to exit
wait $ELIXIR_PID
cleanup
