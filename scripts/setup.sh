#!/bin/bash
set -euo pipefail

# Axon v2 - Bootstrap Script
# Use this script for first-time setup or after significant dependency changes.

PROJECT_ROOT="/home/dstadel/projects/axon"
cd "$PROJECT_ROOT"

echo "🚀 Starting Axon bootstrap..."

# 1. Environment Check (Devenv)
if ! command -v devenv &> /dev/null; then
    echo "❌ devenv not found. Please install it first."
    exit 1
fi

echo "📦 Validating Devenv environment..."
devenv shell -- bash -lc './scripts/validate-devenv.sh'

# 2. Rust Core build
BIN_DIR="$PROJECT_ROOT/bin"
RUST_CORE_DIR="$PROJECT_ROOT/src/axon-core"
TARGET_BIN="$BIN_DIR/axon-core"
CARGO_TARGET_ROOT="${CARGO_TARGET_DIR:-$PROJECT_ROOT/.axon/cargo-target}"
RUST_RELEASE_BIN=$(find "$PROJECT_ROOT" -name "axon-core" -path "*/release/*" -type f | head -n 1)

mkdir -p "$BIN_DIR"

echo "🔨 Building Rust core..."
devenv shell -- bash -lc "cd '$RUST_CORE_DIR' && cargo build --release"
install -m 755 "$RUST_RELEASE_BIN" "$TARGET_BIN"
echo "✅ Rust core available at bin/axon-core"

# 3. Dashboard dependencies and compile
DASHBOARD_DIR="$PROJECT_ROOT/src/dashboard"
echo "💧 Preparing Elixir dashboard..."
devenv shell -- bash -lc "cd '$DASHBOARD_DIR' && mix local.hex --force >/dev/null && mix local.rebar --force >/dev/null && mix deps.get && mix compile"
echo "✅ Elixir dashboard compiled"

# 4. Core validation
echo "🧪 Running validation suite..."

echo "--- Rust Unit Tests ---"
devenv shell -- bash -lc "cd '$RUST_CORE_DIR' && cargo test"

echo "--- Elixir Dashboard Tests ---"
devenv shell -- bash -lc "cd '$DASHBOARD_DIR' && mix test"

echo "🏁 Bootstrap complete."
echo "Next step: ./scripts/start.sh"
echo "Stop running services with: ./scripts/stop.sh"
