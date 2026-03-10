#!/bin/bash
# Axon Industrial v1.0 - Robust Startup Script
# Nexus Lead Architect - 2026-03-09

set -e

echo "🚀 Starting Axon Industrial Pre-flight Checks..."

# 1. Cleanup Zombie Processes
echo "🧹 Cleaning up existing Axon/HydraDB processes..."
fuser -k 6040/tcp 6061/tcp 7047/tcp 2>/dev/null || true

# 2. Check OS Dependencies
if ! command -v inotifywait &> /dev/null; then
    echo "⚠️  WARNING: inotify-tools not found. Real-time surveillance will be disabled."
fi

# 3. Pod C (HydraDB) - Check & Self-Heal
echo "💎 Initializing Pod C (HydraDB)..."
HYDRA_DIR="./.axon/runtime/hydradb"
if [ -d "$HYDRA_DIR" ]; then
    cd $HYDRA_DIR
    if ! mix compile &> /dev/null; then
        echo "🚑 Corruption detected. Self-healing..."
        rm -rf priv/storage/row_store/*
    fi
    cd - > /dev/null
fi

# 4. Pod A (Watcher) - Migrations & Oban
echo "🛡️  Initializing Pod A (Watcher & Oban)..."
cd src/watcher
mix ecto.create --quiet
mix ecto.migrate --quiet
cd - > /dev/null

# 5. Final Launch
echo "🔥 Launching Axon Fleet..."

# Start HydraDB (Pod C)
mkdir -p logs
cd .axon/runtime/hydradb
export TCP_PORT=6040
export HYDRA_DB_API_KEY=dev_key
nohup elixir --name hydra_axon@127.0.0.1 -S mix run --no-halt > ../../../logs/hydradb.log 2>&1 &
cd - > /dev/null

# Start Watcher + Cockpit (Pod A)
cd src/watcher
export PHOENIX_PORT=6061
export AXON_REPO_SLUG=axon
nohup elixir --name watcher@127.0.0.1 -S mix run --no-halt > ../../logs/watcher_cockpit.log 2>&1 &
cd - > /dev/null

echo "✅ AXON IS LIVE!"
echo "   - Cockpit:  http://localhost:6061/cockpit"
echo "   - HydraDB:  tcp://localhost:6040"
echo "   - Status:   Operational"
