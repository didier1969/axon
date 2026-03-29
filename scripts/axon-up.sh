#!/bin/bash
# Axon Industrial v2.0 - Stable Release & Startup
# Nexus Lead Architect - 2026-03-09

set -e

# Silence noisy inotify warnings by ensuring we use the Nix-provided binaries
# [DEBUG] Skipping Nix relaunch due to devenv detection error in current environment.
# if ! command -v inotifywait &> /dev/null; then
#     # If not in PATH, we attempt to use 'nix develop' to run ourselves
#     if command -v nix &> /dev/null && [ -z "$IN_NIX_SHELL" ]; then
#         echo "🔄 Relaunching Axon in isolated Nix environment..."
#         exec nix develop -c "$0" "$@"
#     fi
# fi

echo "🚀 AXON INDUSTRIAL V2.0 - IGNITION"

# 1. Cleaning
echo "🧹 CLEANING ZOMBIE PROCESSES..."
fuser -k 6040/tcp 6061/tcp 7047/tcp 7000/tcp 2>/dev/null || true

# 2. Storage Setup
mkdir -p logs .axon/runtime

# 3. Data Plane (Rust - Axon Core) - Pod C (Nexus Seal)
echo "💎 STARTING AXON CORE DATA PLANE (POD C)..."
# We ensure the telemetry socket is clear
rm -f /tmp/axon-telemetry.sock /tmp/axon-mcp.sock

# RUN: Multi-threaded Tokio Engine
nohup ./src/axon-core/target/release/axon-core > logs/axon_core.log 2>&1 &

# Wait for socket to be ready
timeout 10 bash -c 'until [ -S /tmp/axon-telemetry.sock ]; do sleep 0.5; done' || (echo "❌ TIMEOUT: Axon Core failed to bind socket" && exit 1)
echo "✅ AXON CORE IS LISTENING."

# 4. Pod A (Watcher & Cockpit) - Release Flow
echo "🛡️  STARTING WATCHER RELEASE (POD A)..."
cd src/watcher
# On s'assure que la DB locale est migrée
mix ecto.migrate --quiet

# REBUILD FORCE: Garantir que les changements de PoolFacade (socket) sont appliqués
echo "📦 Rebuilding local release..."
rm -rf _build/dev/rel/axon_watcher
mix release --quiet

export PHOENIX_PORT=6061
export AXON_REPO_SLUG=axon
# Démarrage instantané via la release
nohup _build/dev/rel/axon_watcher/bin/axon_watcher start > ../../logs/watcher_cockpit.log 2>&1 &
cd - > /dev/null

# 5. Intelligence Layer (Integrated MCP Server)
echo "🧠 AXON CORE ALREADY BOOTED MCP ENGINE (PORT 44129)."

echo "✨ AXON V2.2 IS LIVE!"
echo "   🛰️  COCKPIT:  http://localhost:6061/cockpit"
echo "   🔗 TELEMETRY: unix:///tmp/axon-telemetry.sock"
echo "   🧠 MCP API:   http://localhost:44129/sse"
