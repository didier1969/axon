#!/bin/bash
set -euo pipefail

# Axon v2 - Bootstrap Script
# Use this script for first-time setup or after significant dependency changes.

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$PROJECT_ROOT"
# shellcheck source=scripts/lib/axon-version.sh
source "$PROJECT_ROOT/scripts/lib/axon-version.sh"

ARTIFACT_ONLY=0
WITH_TENSORRT=0
TENSORRT_QUALIFY=0
TENSORRT_ARGS=()

usage() {
    cat <<'EOF'
Usage: bash scripts/setup.sh [--artifact-only] [--with-tensorrt] [--tensorrt-qualify]

Options:
  --artifact-only  Build only the canonical Rust release artifact and build-info, then exit.
  --with-tensorrt  Also build and validate the local TensorRT ORT artifact.
  --tensorrt-qualify
                   With --with-tensorrt, run bounded cold TensorRT qualification.
  --tensorrt-arg ARG
                   Forward one argument to scripts/setup-tensorrt.sh.

TensorRT requires the NVIDIA-approved local tarball:
  .axon/downloads/TensorRT-10.14.1.48.Linux.x86_64-gnu.cuda-12.9.tar.gz
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --artifact-only)
            ARTIFACT_ONLY=1
            shift
            ;;
        --with-tensorrt)
            WITH_TENSORRT=1
            shift
            ;;
        --tensorrt-qualify)
            WITH_TENSORRT=1
            TENSORRT_QUALIFY=1
            shift
            ;;
        --tensorrt-arg)
            TENSORRT_ARGS+=("$2")
            shift 2
            ;;
        --tensorrt-arg=*)
            TENSORRT_ARGS+=("${1#*=}")
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
devenv shell -- bash -lc "cd '$RUST_CORE_DIR' && cargo build --release --bins"

install_release_bin() {
    local bin_name="$1"
    local release_bin
    local target_bin
    local build_info_path
    release_bin="$(axon_workspace_release_bin_for "$PROJECT_ROOT" "$bin_name")"
    target_bin="$BIN_DIR/$bin_name"
    build_info_path="$(axon_build_info_path_for "$PROJECT_ROOT" "$bin_name")"
    if [[ ! -x "$release_bin" ]]; then
        echo "❌ Canonical release binary missing after build: $release_bin"
        exit 1
    fi
    install -m 755 "$release_bin" "$target_bin"
    AXON_ARTIFACT_SHA256="$(axon_file_sha256 "$target_bin")"
    axon_write_export_file "$build_info_path" \
        AXON_RELEASE_VERSION "$AXON_PACKAGE_VERSION" \
        AXON_BUILD_ID "$AXON_BUILD_ID" \
        AXON_PACKAGE_VERSION "$AXON_PACKAGE_VERSION" \
        AXON_INSTALL_GENERATION workspace \
        AXON_ARTIFACT_SHA256 "$AXON_ARTIFACT_SHA256" \
        AXON_ARTIFACT_SOURCE "$release_bin"
    echo "✅ Rust binary available at bin/$bin_name"
}

AXON_BUILD_ID="$(axon_workspace_build_id "$PROJECT_ROOT")"
AXON_PACKAGE_VERSION="$(axon_package_version "$PROJECT_ROOT")"
install_release_bin "axon-core"
install_release_bin "axon-brain"
install_release_bin "axon-indexer"
# REQ-AXO-153 — axonctl supervises the runtime processes and exposes the
# status JSON consumed by `axon status` / qualify-mcp. Including it in the
# release artifact set ensures every promotion ships the supervisor that
# matches the runtime contract; without it, axonctl-side fixes (e.g.
# REQ-AXO-151 role_contract_violations) compile and commit but stay inert
# in production.
install_release_bin "axonctl"

if [[ "$ARTIFACT_ONLY" -eq 1 ]]; then
    if [[ "$WITH_TENSORRT" -eq 1 ]]; then
        echo "🧩 Building requested TensorRT artifact..."
        if [[ "$TENSORRT_QUALIFY" -eq 1 ]]; then
            bash "$PROJECT_ROOT/scripts/setup-tensorrt.sh" --qualify "${TENSORRT_ARGS[@]}"
        else
            bash "$PROJECT_ROOT/scripts/setup-tensorrt.sh" "${TENSORRT_ARGS[@]}"
        fi
    fi
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

if [[ "$WITH_TENSORRT" -eq 1 ]]; then
    echo "🧩 Building requested TensorRT artifact..."
    if [[ "$TENSORRT_QUALIFY" -eq 1 ]]; then
        bash "$PROJECT_ROOT/scripts/setup-tensorrt.sh" --qualify "${TENSORRT_ARGS[@]}"
    else
        bash "$PROJECT_ROOT/scripts/setup-tensorrt.sh" "${TENSORRT_ARGS[@]}"
    fi
fi

echo "🏁 Bootstrap complete."
echo "Next step: ./scripts/start.sh"
echo "Stop running services with: ./scripts/stop.sh"
