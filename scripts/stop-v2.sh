#!/bin/bash

# Axon v2 - Industrial Stop Script
# Kills TMUX session and cleans up.

echo "🛑 Stopping Axon v2 Architecture..."

tmux kill-session -t axon 2>/dev/null || true

# Specific cleanup for Axon Pods just in case
killall -9 axon-core 2>/dev/null || true
killall -9 beam.smp 2>/dev/null || true

# Clean up socket
rm -f "/tmp/axon-v2.sock"

echo "✅ Axon stopped."
