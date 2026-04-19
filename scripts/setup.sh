#!/bin/bash
set -euo pipefail

# Axon v2 - Bootstrap Script
# Use this script for first-time setup or after significant dependency changes.

PROJECT_ROOT="/home/dstadel/projects/axon"
cd "$PROJECT_ROOT"
# shellcheck source=scripts/lib/axon-version.sh
source "$PROJECT_ROOT/scripts/lib/axon-version.sh"

ARTIFACT_ONLY=0

usage() {
    cat <<'EOF'
Usage: bash scripts/setup.sh [--artifact-only]

Options:
  --artifact-only  Build only the canonical Rust release artifact and build-info, then exit.
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --artifact-only)
            ARTIFACT_ONLY=1
            shift
            ;;
        --help|-h)
            usage
            exit 0
            ;;
        *)
            echo "Unknown option: $1" >&2
            usage >&2
            exit 1
            ;;
    esac
done

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
mkdir -p "$BIN_DIR"

echo "🔨 Building Rust core..."
devenv shell -- bash -lc "cd '$RUST_CORE_DIR' && cargo build --release"

RUST_RELEASE_BIN="$(axon_workspace_release_bin "$PROJECT_ROOT")"
if [[ ! -x "$RUST_RELEASE_BIN" ]]; then
    echo "❌ Canonical release binary missing after build: $RUST_RELEASE_BIN"
    exit 1
fi
install -m 755 "$RUST_RELEASE_BIN" "$TARGET_BIN"
AXON_BUILD_ID="$(axon_workspace_build_id "$PROJECT_ROOT")"
AXON_PACKAGE_VERSION="$(axon_package_version "$PROJECT_ROOT")"
AXON_ARTIFACT_SHA256="$(axon_file_sha256 "$TARGET_BIN")"
axon_write_export_file "$BIN_DIR/axon-core.build-info" \
    AXON_RELEASE_VERSION "$AXON_PACKAGE_VERSION" \
    AXON_BUILD_ID "$AXON_BUILD_ID" \
    AXON_PACKAGE_VERSION "$AXON_PACKAGE_VERSION" \
    AXON_INSTALL_GENERATION workspace \
    AXON_ARTIFACT_SHA256 "$AXON_ARTIFACT_SHA256" \
    AXON_ARTIFACT_SOURCE "$RUST_RELEASE_BIN"
echo "✅ Rust core available at bin/axon-core"

if [[ "$ARTIFACT_ONLY" -eq 1 ]]; then
    echo "🏁 Artifact-only bootstrap complete."
    exit 0
fi

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
