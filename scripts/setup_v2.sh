#!/bin/bash
set -e

# Axon v2 - Industrial Setup Script
# Strategy: Build once, store in bin/, reuse for development and tests.

echo "🚀 Starting Axon v2 Industrial Setup..."

# 1. Environment Check (asdf)
if ! command -v asdf &> /dev/null; then
    echo "❌ asdf not found. Please install it first."
    exit 1
fi

echo "📦 Installing runtimes via asdf..."
asdf install

# 2. Rust Data Plane (The "Factory")
BIN_DIR="$(pwd)/bin"
RUST_CORE_DIR="$(pwd)/src/axon-core"
TARGET_BIN="$BIN_DIR/axon-core"

mkdir -p "$BIN_DIR"

if [ ! -f "$TARGET_BIN" ]; then
    echo "🔨 Building Rust Data Plane (this may take ~5-10 min due to LadybugDB)..."
    cd "$RUST_CORE_DIR"
    cargo build --release
    cp target/release/axon-core "$TARGET_BIN"
    echo "✅ Rust Core compiled and stored in bin/axon-core"
    cd - > /dev/null
else
    echo "✅ Reusing existing Rust binary in bin/axon-core"
fi

# 3. Elixir Dashboard (The "Cockpit")
DASHBOARD_DIR="$(pwd)/src/dashboard"
echo "💧 Setting up Elixir Dashboard..."
cd "$DASHBOARD_DIR"
mix deps.get
mix compile
# mix assets.setup && mix assets.build # Décommenter si besoin de rebuild les assets JS/CSS
echo "✅ Elixir Dashboard ready."
cd - > /dev/null

# 4. Final Validation Suite
echo "🧪 Running Quality Audit..."

echo "--- Rust Unit Tests ---"
cd "$RUST_CORE_DIR"
cargo test --lib
cd - > /dev/null

echo "--- Elixir Business Tests (>85% Coverage) ---"
cd "$DASHBOARD_DIR"
mix test --cover
cd - > /dev/null

echo "--- E2E Orchestration Test ---"
export AXON_BIN="$TARGET_BIN"
python3 tests/e2e_v2_orchestration.py

echo "🏆 Axon v2 is fully operational!"
echo "Run './bin/axon-core --mcp' to start the engine."
echo "Run 'cd src/dashboard && mix phx.server' to start the cockpit."
