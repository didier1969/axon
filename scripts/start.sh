#!/bin/bash
set -euo pipefail

# Axon start script — rewritten session 55 (2026-05-25).
# Usage: ./scripts/axon-dev start <mode>
# Modes: brain | full
# Options: --no-dashboard, --fast

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
DEFAULT_PROJECTS_ROOT="$(cd "$PROJECT_ROOT/.." && pwd)"

# --- Libraries ---
# shellcheck source=scripts/lib/axon-instance.sh
source "$PROJECT_ROOT/scripts/lib/axon-instance.sh"
_SAVED_INSTANCE_KIND="${AXON_INSTANCE_KIND:-}"
_SAVED_WATCH_DIR="${AXON_WATCH_DIR:-}"
_SAVED_BULK_WRITER="${AXON_BULK_WRITER_ENABLED:-}"
axon_clear_inherited_env
[[ -n "$_SAVED_INSTANCE_KIND" ]] && export AXON_INSTANCE_KIND="$_SAVED_INSTANCE_KIND"
[[ -n "$_SAVED_WATCH_DIR" ]] && export AXON_WATCH_DIR="$_SAVED_WATCH_DIR"
[[ -n "$_SAVED_BULK_WRITER" ]] && export AXON_BULK_WRITER_ENABLED="$_SAVED_BULK_WRITER"
unset _SAVED_INSTANCE_KIND _SAVED_WATCH_DIR _SAVED_BULK_WRITER
# shellcheck source=scripts/lib/axon-role-layout.sh
source "$PROJECT_ROOT/scripts/lib/axon-role-layout.sh"
# shellcheck source=scripts/lib/axon-resource-policy.sh
source "$PROJECT_ROOT/scripts/lib/axon-resource-policy.sh"
# shellcheck source=scripts/lib/axon-ort-runtime.sh
source "$PROJECT_ROOT/scripts/lib/axon-ort-runtime.sh"
# shellcheck source=scripts/lib/axon-version.sh
source "$PROJECT_ROOT/scripts/lib/axon-version.sh"
# shellcheck source=scripts/lib/ensure-runtime.sh
source "$PROJECT_ROOT/scripts/lib/ensure-runtime.sh"
cd "$PROJECT_ROOT"

axon_load_worktree_env "$PROJECT_ROOT"
axon_resolve_instance "$PROJECT_ROOT" "$(basename "$PROJECT_ROOT")"
axon_resolve_resource_policy "$AXON_INSTANCE_KIND"
axon_resolve_version "$PROJECT_ROOT"

# --- Arguments ---
RUNTIME_MODE="brain_only"
START_DASHBOARD=1
RUN_MCP_TESTS=1

FORCE_RELEASE=0

while [[ $# -gt 0 ]]; do
    case "$1" in
        brain)           RUNTIME_MODE="brain_only" ;;
        full|indexer)    RUNTIME_MODE="indexer_full" ;;
        --release)       FORCE_RELEASE=1 ;;
        --no-dashboard)  START_DASHBOARD=0 ;;
        --fast)          START_DASHBOARD=0; RUN_MCP_TESTS=0 ;;
        --help|-h)
            cat <<'EOF'
Usage: ./scripts/axon-dev start <mode> [options]

Modes:
  brain       MCP server only (no indexation)
  full        Brain + indexer + GPU embedder + dashboard

Options:
  --release        Use release binaries (10× less RAM, 10× faster)
  --no-dashboard   Disable dashboard
  --fast           No dashboard, no MCP tests
EOF
            exit 0 ;;
        *) echo "❌ Unknown: $1. Use --help."; exit 1 ;;
    esac
    shift
done

export AXON_RUNTIME_MODE="$RUNTIME_MODE"

# --- GPU detection (auto TensorRT when GPU present + mode uses vectors) ---
detect_gpu() {
    command -v nvidia-smi >/dev/null 2>&1 && nvidia-smi -L >/dev/null 2>&1 && return 0
    [[ -x /usr/lib/wsl/lib/nvidia-smi ]] && /usr/lib/wsl/lib/nvidia-smi -L >/dev/null 2>&1 && return 0
    return 1
}

if [[ "$RUNTIME_MODE" == indexer_full || "$RUNTIME_MODE" == indexer_vector ]]; then
    if detect_gpu; then
        export AXON_EMBEDDING_PROVIDER="tensorrt"
        _nvml="$(/usr/lib/wsl/lib/nvidia-smi --query 2>/dev/null | head -1 || true)"
        for candidate in /usr/lib/wsl/lib/libnvidia-ml.so.1 /usr/lib/x86_64-linux-gnu/libnvidia-ml.so.1; do
            [[ -f "$candidate" ]] && export AXON_NVML_LIBRARY_PATH="$candidate" && break
        done
    else
        export AXON_EMBEDDING_PROVIDER="cpu"
        export AXON_VECTOR_WORKERS=1
        export OMP_NUM_THREADS=4
        export OMP_WAIT_POLICY=PASSIVE
    fi
else
    export AXON_EMBEDDING_PROVIDER="cpu"
fi

# --- Load per-instance runtime config ---
case "${AXON_INSTANCE_KIND:-live}" in
    dev)  RUNTIME_CONFIG="$PROJECT_ROOT/.axon-dev/runtime-config.dev.env" ;;
    live) RUNTIME_CONFIG="$PROJECT_ROOT/.axon/runtime-config.live.env" ;;
    *)    RUNTIME_CONFIG="" ;;
esac
if [[ -n "$RUNTIME_CONFIG" && -f "$RUNTIME_CONFIG" ]]; then
    set -o allexport; . "$RUNTIME_CONFIG"; set +o allexport
fi

# --- Preflight ---

# 1. Nix daemon
if ! nix store info >/dev/null 2>&1; then
    echo "⚠️  Nix daemon not responding, attempting start..."
    if command -v systemctl >/dev/null && systemctl is-system-running >/dev/null 2>&1; then
        sudo systemctl start nix-daemon || true
    else
        sudo bash -c "/nix/var/nix/profiles/default/bin/nix-daemon --daemon &" || true
        sleep 2
    fi
    nix store info >/dev/null 2>&1 || { echo "❌ Nix daemon still down."; exit 1; }
fi

# 2. Devenv shell helper
run_devenv() {
    devenv shell --no-reload --no-tui -- bash -lc "$1"
}

# 3. Validate devenv
echo "📦 Validating devenv..."
run_devenv './scripts/validate-devenv.sh'

# 4. PG bootstrap
if ! ensure_runtime_ready "$AXON_INSTANCE_KIND"; then
    echo "❌ PG bootstrap failed."; exit 1
fi

# 5. Already running check
if nc -z localhost "$HYDRA_HTTP_PORT" 2>/dev/null; then
    echo "ℹ️  Already running on :$HYDRA_HTTP_PORT. Stop first."
    exit 0
fi

# --- Resolve binaries ---
CARGO_TARGET="$PROJECT_ROOT/.axon/cargo-target"

if [[ "$AXON_INSTANCE_KIND" == "live" ]]; then
    BRAIN_BIN="$PROJECT_ROOT/bin/axon-brain"
    INDEXER_BIN="$PROJECT_ROOT/bin/axon-indexer"

    # Live release manifest: install promoted binaries
    MANIFEST="$PROJECT_ROOT/.axon/live-release/current.json"
    if [[ -f "$MANIFEST" ]]; then
        BRAIN_ARTIFACT="$(python3 -c "import json; print(json.load(open('$MANIFEST')).get('artifacts',{}).get('axon-brain',{}).get('path',''))")"
        INDEXER_ARTIFACT="$(python3 -c "import json; print(json.load(open('$MANIFEST')).get('artifacts',{}).get('axon-indexer',{}).get('path',''))")"
        if [[ -n "$BRAIN_ARTIFACT" && -f "$BRAIN_ARTIFACT" ]]; then
            mkdir -p bin
            install -m 755 "$BRAIN_ARTIFACT" "$BRAIN_BIN"
            install -m 755 "$INDEXER_ARTIFACT" "$INDEXER_BIN"
        fi
    fi
else
    if [[ "$FORCE_RELEASE" == "1" ]] || [[ -x "$CARGO_TARGET/release/axon-brain" && -x "$CARGO_TARGET/release/axon-indexer" ]]; then
        BRAIN_BIN="$CARGO_TARGET/release/axon-brain"
        INDEXER_BIN="$CARGO_TARGET/release/axon-indexer"
        BUILD_PROFILE="--release"
    else
        BRAIN_BIN="$CARGO_TARGET/debug/axon-brain"
        INDEXER_BIN="$CARGO_TARGET/debug/axon-indexer"
        BUILD_PROFILE=""
    fi
fi

# 6. Auto-rebuild (dev only)
if [[ "$AXON_INSTANCE_KIND" != "live" ]]; then
    if [[ ! -x "$BRAIN_BIN" || ! -x "$INDEXER_BIN" ]]; then
        echo "🔨 Binary missing, building ${BUILD_PROFILE:-debug}..."
        run_devenv "cargo build --manifest-path src/axon-core/Cargo.toml ${BUILD_PROFILE:-} --bin axon-brain --bin axon-indexer"
    elif find "$PROJECT_ROOT/src/axon-core/src" -type f \( -name '*.rs' -o -name 'Cargo.toml' \) -newer "$BRAIN_BIN" -print -quit | grep -q .; then
        echo "🔨 Sources newer than binary, rebuilding ${BUILD_PROFILE:-debug}..."
        run_devenv "cargo build --manifest-path src/axon-core/Cargo.toml ${BUILD_PROFILE:-} --bin axon-brain --bin axon-indexer"
    fi
fi

# Check binaries exist
for bin in "$BRAIN_BIN" "$INDEXER_BIN"; do
    [[ -x "$bin" ]] || { echo "❌ Missing: $bin"; exit 1; }
done

# --- Env exports ---
# Limit glibc per-thread mmap arenas. Without this, 170+ threads create
# up to 64 arenas × 64 MB each (~10 GB virtual). REQ-AXO-91563.
export MALLOC_ARENA_MAX="${MALLOC_ARENA_MAX:-2}"
# ORT loads libonnxruntime from ORT_DYLIB_PATH instead of bundling it.
export ORT_STRATEGY=system
# Root directory containing all projects to watch/index.
export AXON_PROJECTS_ROOT="${AXON_PROJECTS_ROOT:-$DEFAULT_PROJECTS_ROOT}"
# Filesystem scope for the inotify watcher. Defaults to all projects.
export AXON_WATCH_DIR="${AXON_WATCH_DIR:-$DEFAULT_PROJECTS_ROOT}"
# Use COPY BINARY for PG writes instead of INSERT VALUES SQL text.
export AXON_BULK_WRITER_ENABLED="${AXON_BULK_WRITER_ENABLED:-1}"
# Absolute path to this Axon repo root.
export AXON_PROJECT_ROOT="$PROJECT_ROOT"
# HTTP health port for the indexer process (brain port + 10).
export AXON_INDEXER_HEALTH_PORT=$((HYDRA_HTTP_PORT + 10))
# Absolute paths to the brain and indexer binaries for process-compose.
export AXON_BRAIN_BIN="$BRAIN_BIN"
export AXON_INDEXER_BIN="$INDEXER_BIN"
# Writer lock role: brain=SOLL writer, indexer=IST writer.
export AXON_RUNTIME_SHADOW_ROLE="$( [[ "$RUNTIME_MODE" == "brain_only" ]] && echo brain || echo indexer )"
# Random cookie for Erlang node authentication (dashboard).
export AXON_ERLANG_COOKIE="${AXON_ERLANG_COOKIE:-$(head -c 32 /dev/urandom | base64 | tr -d '/+=' | head -c 20)}"
# Dashboard toggle for process-compose YAML.
[[ "$START_DASHBOARD" == "1" ]] && export AXON_DASHBOARD_DISABLED=false || export AXON_DASHBOARD_DISABLED=true

# Resolve ORT runtime (sets ORT_DYLIB_PATH, LD_LIBRARY_PATH)
axon_resolve_ort_runtime "$PROJECT_ROOT" "${AXON_EMBEDDING_PROVIDER:-cpu}" || exit 1
# The ort resolver sets PRELAUNCH_LD_LIBRARY_PATH_EXPORT as a string
# "export LD_LIBRARY_PATH=...". Eval it so process-compose children inherit it.
if [[ -n "${PRELAUNCH_LD_LIBRARY_PATH_EXPORT:-}" ]]; then
    eval "$PRELAUNCH_LD_LIBRARY_PATH_EXPORT"
fi

# Role layout (sets DB paths, socket paths per instance)
axon_apply_runtime_role_layout "$PROJECT_ROOT" "$AXON_RUNTIME_SHADOW_ROLE" "axon-brain"

mkdir -p "$AXON_DB_ROOT" "${AXON_RUN_ROOT:-/tmp}"
rm -f "${AXON_TELEMETRY_SOCK:-}" "${AXON_MCP_SOCK:-}" "${AXON_PID_FILE:-}"

# --- Process selection ---
PC_PROCESSES=(axon-brain)
READYZ_PORT="$HYDRA_HTTP_PORT"

if [[ "$RUNTIME_MODE" == indexer_* ]]; then
    PC_PROCESSES+=(axon-indexer)
    READYZ_PORT="$AXON_INDEXER_HEALTH_PORT"
fi
[[ "$START_DASHBOARD" == "1" ]] && PC_PROCESSES+=(dashboard)

# --- Launch ---
PC_YAML="$PROJECT_ROOT/process-compose.${AXON_INSTANCE_KIND}.yaml"
[[ -f "$PC_YAML" ]] || { echo "❌ Missing: $PC_YAML"; exit 1; }

case "$AXON_INSTANCE_KIND" in
    live) PC_PORT=8080 ;; dev) PC_PORT=8081 ;; *) PC_PORT=8080 ;;
esac

# Resolve process-compose binary from devenv
PC_BIN="$(run_devenv 'which process-compose' 2>/dev/null | tail -1)"
[[ -x "${PC_BIN:-}" ]] || { echo "❌ process-compose not found in devenv."; exit 1; }
export AXON_PGREADY_BIN="$(run_devenv 'which pg_isready' 2>/dev/null | tail -1)"

echo "🚀 Starting Axon (instance=$AXON_INSTANCE_KIND, mode=$RUNTIME_MODE)"
echo "   Brain: $BRAIN_BIN | Indexer: $INDEXER_BIN"
echo "   MCP: http://127.0.0.1:$HYDRA_HTTP_PORT/mcp"
echo "   Embedding: ${AXON_EMBEDDING_PROVIDER:-cpu}"

"$PC_BIN" up -f "$PC_YAML" -p "$PC_PORT" -t=false -D --ordered-shutdown --disable-dotenv "${PC_PROCESSES[@]}"

# --- Wait for readiness ---
TIMEOUT_S=$([[ "$RUNTIME_MODE" == indexer_full ]] && echo 900 || echo 120)
echo "⏳ Waiting for :${READYZ_PORT}/readyz (timeout ${TIMEOUT_S}s)..."
for ((i=1; i<=TIMEOUT_S; i++)); do
    curl -sf "http://127.0.0.1:${READYZ_PORT}/readyz" >/dev/null 2>&1 && {
        echo "✅ Axon ready (instance=$AXON_INSTANCE_KIND, mode=$RUNTIME_MODE)"
        echo "   MCP: http://127.0.0.1:$HYDRA_HTTP_PORT/mcp"
        echo "   Stop: ./scripts/axon --instance $AXON_INSTANCE_KIND stop"
        exit 0
    }
    (( i % 15 == 0 )) && echo "  ⏳ ${i}s/${TIMEOUT_S}s..."
    sleep 1
done

echo "❌ Timeout on :${READYZ_PORT}/readyz"
echo "   Logs: process-compose -p $PC_PORT process logs axon-brain"
exit 1
