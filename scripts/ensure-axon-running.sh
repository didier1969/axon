#!/bin/bash

# Axon v2 - Immune System Guard (TMUX Edition)
# Ensures Axon is ALWAYS running in the background via TMUX.

PROJECT_ROOT="/home/dstadel/projects/axon"

# Check if the tmux session 'axon' exists
if tmux has-session -t axon 2>/dev/null; then
    # Session exists, we assume it's healthy.
    # We could do more advanced checks here if needed.
    exit 0
fi

echo "🛡️ Axon Immune System is down. Resurrecting in TMUX..."

cd "$PROJECT_ROOT"
# Launch the start script which creates the tmux session
bash scripts/start-v2.sh > /dev/null 2>&1

echo "✅ Axon resurrection triggered. Check http://localhost:44127"
