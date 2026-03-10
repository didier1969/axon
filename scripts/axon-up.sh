#!/bin/bash
# Axon Industrial v2.0 - Stable Release & Startup
# Nexus Lead Architect - 2026-03-09

set -e

# Silence noisy inotify warnings by ensuring we use the Nix-provided binaries
if ! command -v inotifywait &> /dev/null; then
    # If not in PATH, we attempt to use 'nix develop' to run ourselves
    if command -v nix &> /dev/null && [ -z "$IN_NIX_SHELL" ]; then
        echo "🔄 Relaunching Axon in isolated Nix environment..."
        exec nix develop -c "$0" "$@"
    fi
fi

echo "🚀 AXON INDUSTRIAL V2.0 - IGNITION"

# 1. Cleaning
echo "🧹 CLEANING ZOMBIE PROCESSES..."
fuser -k 6040/tcp 6061/tcp 7047/tcp 7000/tcp 2>/dev/null || true

# 2. Storage Setup
mkdir -p logs .axon/runtime

# 3. Pod C (HydraDB) - Start
echo "💎 STARTING HYDRADB (POD C)..."
cd .axon/runtime/hydradb
export TCP_PORT=6040
export HYDRA_DB_API_KEY=dev_key
# We start via mix run for dev flexibility in Pod C
nohup elixir --name hydra_axon@127.0.0.1 -S mix run --no-halt > ../../../logs/hydradb.log 2>&1 &
cd - > /dev/null

# 4. Pod A (Watcher & Cockpit) - Release Flow
echo "🛡️  STARTING WATCHER RELEASE (POD A)..."
cd src/watcher
# On s'assure que la DB locale est migrée
mix ecto.migrate --quiet

# Si la release n'existe pas, on la compile (équivalent image Docker locale)
if [ ! -f "_build/dev/rel/axon_watcher/bin/axon_watcher" ]; then
    echo "📦 Building local release (image-like)..."
    mix release --quiet
fi

export PHOENIX_PORT=6061
export AXON_REPO_SLUG=axon
# Démarrage instantané via la release
nohup _build/dev/rel/axon_watcher/bin/axon_watcher start > ../../logs/watcher_cockpit.log 2>&1 &
cd - > /dev/null

# 5. Intelligence Layer
echo "🧠 BOOTING MCP INTELLIGENCE (PORT 7000)..."
nohup uv run scripts/axon-mcp-sse.py > logs/mcp_sse.log 2>&1 &

echo "✨ AXON V2.0 IS LIVE!"
echo "   🛰️  COCKPIT:  http://localhost:6061/cockpit"
echo "   🔗 HYDRADB:  tcp://localhost:6040"
echo "   🧠 MCP SSE:  http://localhost:7000/sse"
