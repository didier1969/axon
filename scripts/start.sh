#!/bin/bash
set -euo pipefail

# Axon v2 - Daily Start Script
# Canonical daily workflow entrypoint for running Axon in TMUX.

PROJECT_ROOT="$(pwd)"
DEFAULT_PROJECTS_ROOT="/home/dstadel/projects"
cd "$PROJECT_ROOT"

if [ -f "$PROJECT_ROOT/.env.worktree" ]; then
    echo "🔧 Loading .env.worktree configuration..."
    source "$PROJECT_ROOT/.env.worktree"
fi

AXON_ENV="${AXON_ENV:-prod}"
TMUX_SESSION="${TMUX_SESSION:-axon}"

WATCH_ROOT="${AXON_WATCH_DIR:-$DEFAULT_PROJECTS_ROOT}"
PROJECTS_ROOT="${AXON_PROJECTS_ROOT:-$WATCH_ROOT}"
PROJECT_CODE="${AXON_PROJECT_CODE:-}"
if [[ -z "$PROJECT_CODE" && -f "$PROJECT_ROOT/.axon/meta.json" ]]; then
    PROJECT_CODE="$(python3 -c 'import json; print(json.load(open("'"$PROJECT_ROOT"'/.axon/meta.json")).get("code",""))' 2>/dev/null || true)"
fi
PROJECT_CODE="${PROJECT_CODE:-$(basename "$PROJECT_ROOT")}"
RUNTIME_MODE="${AXON_RUNTIME_MODE:-full}"
START_DASHBOARD=1
RUN_MCP_TESTS=1
SKIP_ELIXIR_PREWARM="${AXON_SKIP_ELIXIR_PREWARM:-0}"

detect_accessible_gpu() {
    if command -v nvidia-smi >/dev/null 2>&1 && nvidia-smi -L >/dev/null 2>&1; then
        return 0
    fi
    if [ -x /usr/lib/wsl/lib/nvidia-smi ] && /usr/lib/wsl/lib/nvidia-smi -L >/dev/null 2>&1; then
        return 0
    fi
    return 1
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --full)
            RUNTIME_MODE="full"
            ;;
        --graph-only|--graphonly)
            RUNTIME_MODE="graph_only"
            ;;
        --read-only|--readonly)
            RUNTIME_MODE="read_only"
            ;;
        --mcp-only|--mcponly)
            RUNTIME_MODE="mcp_only"
            START_DASHBOARD=0
            ;;
        --no-dashboard)
            START_DASHBOARD=0
            ;;
        --skip-mcp-tests)
            RUN_MCP_TESTS=0
            ;;
        --skip-elixir-prewarm)
            SKIP_ELIXIR_PREWARM=1
            ;;
        --help|-h)
            cat <<'EOF'
Usage: ./scripts/start.sh [--full|--read-only|--mcp-only] [--no-dashboard] [--skip-mcp-tests] [--skip-elixir-prewarm]

Modes:
  --full           Full runtime: scan + watcher + ingestion + SQL/MCP + dashboard
  --graph-only     Scan + watcher + graph indexing + SQL/MCP + dashboard, without semantic/vector workers
  --read-only      SQL/MCP + dashboard only, without scan/watcher/ingestion workers
  --mcp-only       SQL/MCP only, without dashboard and without scan/watcher/ingestion workers

Options:
  --no-dashboard   Disable Elixir LiveView dashboard
  --skip-mcp-tests Skip automatic MCP quality gate validation after startup
  --skip-elixir-prewarm Skip non-interactive `mix local.hex`/`mix local.rebar` bootstrap
EOF
            exit 0
            ;;
        *)
            echo "❌ Unknown option: $1"
            echo "   Use --help to list supported modes."
            exit 1
            ;;
    esac
    shift
done

export AXON_SKIP_ELIXIR_PREWARM="$SKIP_ELIXIR_PREWARM"

if [[ -n "${AXON_EMBEDDING_PROVIDER:-}" ]]; then
    EMBEDDING_PROVIDER_REQUEST="$AXON_EMBEDDING_PROVIDER"
elif detect_accessible_gpu; then
    EMBEDDING_PROVIDER_REQUEST="cuda"
else
    EMBEDDING_PROVIDER_REQUEST="cpu"
fi
export AXON_EMBEDDING_PROVIDER="$EMBEDDING_PROVIDER_REQUEST"

if [[ "$EMBEDDING_PROVIDER_REQUEST" == "cpu" ]]; then
    if [[ -z "${AXON_VECTOR_WORKERS:-}" ]]; then
        export AXON_VECTOR_WORKERS=1
    fi
    if [[ -z "${AXON_CHUNK_BATCH_SIZE:-}" ]]; then
        export AXON_CHUNK_BATCH_SIZE=24
    fi
    if [[ -z "${AXON_FILE_VECTORIZATION_BATCH_SIZE:-}" ]]; then
        export AXON_FILE_VECTORIZATION_BATCH_SIZE=8
    fi
    if [[ -z "${OMP_NUM_THREADS:-}" ]]; then
        export OMP_NUM_THREADS=4
    fi
    if [[ -z "${OMP_WAIT_POLICY:-}" ]]; then
        export OMP_WAIT_POLICY=PASSIVE
    fi
fi

STARTUP_TIMEOUT_S="${AXON_STARTUP_TIMEOUT_S:-}"
if [[ -z "$STARTUP_TIMEOUT_S" ]]; then
    if [[ "$RUNTIME_MODE" == "full" ]]; then
        STARTUP_TIMEOUT_S=240
    else
        STARTUP_TIMEOUT_S=120
    fi
fi

if ! command -v tmux >/dev/null 2>&1; then
    echo "❌ tmux is required to start Axon via scripts/start.sh"
    exit 1
fi

echo "📦 Validating Devenv environment..."
devenv shell -- bash -lc './scripts/validate-devenv.sh'

if [[ "$SKIP_ELIXIR_PREWARM" == "1" ]]; then
    echo "⏭️ Skipping Elixir pre-warm (AXON_SKIP_ELIXIR_PREWARM=1)."
else
    echo "📦 Pre-warming Elixir environment (Hex/Rebar)..."
    devenv shell -- bash -lc "cd '$PROJECT_ROOT/src/dashboard' && mix local.hex --force >/dev/null && mix local.rebar --force >/dev/null"
fi

if [ ! -x "bin/axon-core" ]; then
    echo "❌ Missing bin/axon-core"
    echo "   Run ./scripts/setup.sh first."
    exit 1
fi

if tmux has-session -t axon 2>/dev/null; then
    DELETED_EXE_PIDS=$(for pid in $(pgrep -f "$PROJECT_ROOT/bin/axon-core" || true); do
        exe=$(readlink -f "/proc/$pid/exe" 2>/dev/null || true)
        if [[ "$exe" == *"(deleted)"* ]]; then
            echo "$pid"
        fi
    done)

    if [ -n "${DELETED_EXE_PIDS:-}" ]; then
        echo "⚠️ Found Axon processes still running on deleted executables: $DELETED_EXE_PIDS"
        echo "   Resetting stale runtime state before restart..."
        bash "$PROJECT_ROOT/scripts/stop.sh"
    fi

    if nc -z localhost 44129 2>/dev/null || [ -S "/tmp/axon-telemetry.sock" ]; then
        echo "ℹ️ Axon is already running in TMUX session 'axon'."
        echo "   Attach with: tmux attach -t axon"
        exit 0
    fi

    echo "⚠️ Found stale TMUX session 'axon' without a healthy data plane. Resetting local runtime state..."
    tmux kill-session -t axon 2>/dev/null || true
fi

# 1. Verify nix-daemon is running (WSL2 specific mitigation)
if ! nix store info >/dev/null 2>&1; then
    echo "⚠️ Nix daemon is not responding. Attempting to start it..."
    if command -v systemctl >/dev/null && systemctl is-system-running >/dev/null 2>&1; then
        sudo systemctl start nix-daemon
    else
        sudo bash -c "/nix/var/nix/profiles/default/bin/nix-daemon --daemon &"
        sleep 2
    fi
fi

# 2. Synchronize binaries (handle 'Text file busy' via install)
CARGO_TARGET_ROOT="${CARGO_TARGET_DIR:-$PROJECT_ROOT/.axon/cargo-target}"
LEGACY_RELEASE_BIN="$PROJECT_ROOT/src/axon-core/target/release/axon-core"
DEVENV_RELEASE_BIN="$CARGO_TARGET_ROOT/release/axon-core"
DEVENV_TUNNEL_BIN="$CARGO_TARGET_ROOT/release/axon-mcp-tunnel"

rebuild_core_release() {
    echo "🔧 Rebuilding axon-core release inside Devenv..."
    if ! devenv shell -- bash -lc "cd '$PROJECT_ROOT/src/axon-core' && cargo build --release"; then
        echo "❌ Automatic Devenv rebuild failed."
        return 1
    fi
    return 0
}

rebuild_tunnel_release() {
    echo "🔧 Rebuilding axon-mcp-tunnel release inside Devenv..."
    if ! devenv shell -- bash -lc "cd '$PROJECT_ROOT/src/axon-mcp-tunnel' && cargo build --release"; then
        echo "❌ Automatic Devenv rebuild for axon-mcp-tunnel failed."
        return 1
    fi
    return 0
}

verify_sql_gateway() {
    local response
    response="$(curl -sS -X POST http://127.0.0.1:$HYDRA_HTTP_PORT/sql \
        -H 'content-type: application/json' \
        -d '{"query":"SHOW TABLES"}' 2>/dev/null || true)"

    if [[ -z "$response" ]]; then
        echo "❌ SQL Gateway did not answer the schema probe."
        return 1
    fi

    for table in File Symbol RuntimeMetadata; do
        if [[ "$response" != *"\"$table\""* ]]; then
            echo "❌ SQL Gateway is up but missing required table '$table'."
            echo "   Response: $response"
            return 1
        fi
    done

    return 0
}

probe_sql_gateway() {
    curl -sS -X POST "http://127.0.0.1:$HYDRA_HTTP_PORT/sql" \
        -H 'content-type: application/json' \
        -d '{"query":"SELECT 1"}' >/dev/null 2>&1
}

verify_mcp_http() {
    local response
    response="$(curl -sS -X POST "http://127.0.0.1:$HYDRA_HTTP_PORT/mcp" \
        -H 'content-type: application/json' \
        -d '{"jsonrpc":"2.0","method":"tools/list","params":{},"id":1}' 2>/dev/null || true)"

    [[ "$response" == *"axon_query"* ]]
}

if [ -f "$LEGACY_RELEASE_BIN" ] && [ ! -f "$DEVENV_RELEASE_BIN" ]; then
    echo "⚠️ Found a release build outside Devenv at $LEGACY_RELEASE_BIN"
    echo "   Axon starts from $DEVENV_RELEASE_BIN. Attempting automatic rebuild..."
    rebuild_core_release || exit 1
fi

if [ -f "$LEGACY_RELEASE_BIN" ] && [ "$LEGACY_RELEASE_BIN" -nt "$DEVENV_RELEASE_BIN" ]; then
    echo "⚠️ Detected a newer release binary outside Devenv:"
    echo "   $LEGACY_RELEASE_BIN"
    echo "   Attempting to refresh the authoritative Devenv build..."
    rebuild_core_release || exit 1
fi

if [ ! -f "$DEVENV_RELEASE_BIN" ]; then
    echo "⚠️ Missing Devenv release binary at $DEVENV_RELEASE_BIN"
    rebuild_core_release || exit 1
fi

if [ ! -f "$DEVENV_TUNNEL_BIN" ]; then
    echo "⚠️ Missing Devenv tunnel binary at $DEVENV_TUNNEL_BIN"
    rebuild_tunnel_release || exit 1
fi

if [ -f "$DEVENV_RELEASE_BIN" ] && find "$PROJECT_ROOT/src/axon-core/src" \
    "$PROJECT_ROOT/src/axon-core/Cargo.toml" \
    "$PROJECT_ROOT/src/axon-core/Cargo.lock" \
    -newer "$DEVENV_RELEASE_BIN" -print -quit | grep -q .; then
    echo "⚠️ Detected newer axon-core sources than $DEVENV_RELEASE_BIN"
    echo "   Rebuilding authoritative Devenv release..."
    rebuild_core_release || exit 1
fi

if [ -f "$DEVENV_TUNNEL_BIN" ] && find "$PROJECT_ROOT/src/axon-mcp-tunnel/src" \
    "$PROJECT_ROOT/src/axon-mcp-tunnel/Cargo.toml" \
    "$PROJECT_ROOT/src/axon-mcp-tunnel/Cargo.lock" \
    -newer "$DEVENV_TUNNEL_BIN" -print -quit | grep -q .; then
    echo "⚠️ Detected newer axon-mcp-tunnel sources than $DEVENV_TUNNEL_BIN"
    echo "   Rebuilding authoritative Devenv tunnel release..."
    rebuild_tunnel_release || exit 1
fi

if [ -f "$DEVENV_RELEASE_BIN" ]; then
    echo "🔄 Updating bin/axon-core safely..."
    mkdir -p bin && install -m 755 "$DEVENV_RELEASE_BIN" bin/axon-core
fi

if [ -f "$DEVENV_TUNNEL_BIN" ]; then
    echo "🔄 Updating bin/axon-mcp-tunnel safely..."
    mkdir -p bin && install -m 755 "$DEVENV_TUNNEL_BIN" bin/axon-mcp-tunnel
fi

echo "🚀 Starting Axon in TMUX session '$TMUX_SESSION'..."
echo "📂 Watch root: $WATCH_ROOT"
echo "🗂️ Projects root: $PROJECTS_ROOT"
echo "🧭 Runtime mode: $RUNTIME_MODE"

# Configuration
if [ "$AXON_ENV" = "dev" ]; then
    export PHX_PORT=44137
    export HYDRA_TCP_PORT=44138
    export HYDRA_HTTP_PORT=44139
    export HYDRA_ODATA_PORT=44140
    export HYDRA_HTTP2_PORT=44141
    export HYDRA_MCP_PORT=44142
    TMUX_SESSION="axon-dev"
    ELIXIR_NODE_NAME="axon_dev_nexus"
else
    export PHX_PORT=44127
    export HYDRA_TCP_PORT=44128
    export HYDRA_HTTP_PORT=44129
    export HYDRA_ODATA_PORT=44130
    export HYDRA_HTTP2_PORT=44131
    export HYDRA_MCP_PORT=44132
    ELIXIR_NODE_NAME="axon_nexus"
fi
export WSL_IP
WSL_IP=$(ip addr show eth0 | grep "inet " | awk '{print $2}' | cut -d/ -f1)
if [ -z "$WSL_IP" ]; then
    WSL_IP="127.0.0.1"
fi
export AXON_SQL_URL="http://$WSL_IP:$HYDRA_HTTP_PORT/sql"
export SQL_URL="$AXON_SQL_URL"

# Clean only the sockets used by the active runtime path
rm -f /tmp/axon-telemetry.sock /tmp/axon-mcp.sock
rm -f "$PROJECT_ROOT/.axon/graph_v2/"*.lock 2>/dev/null || true

# Never discard DuckDB WAL during a normal restart. WAL replay is required to recover
# recent committed work when the main database file has not been checkpointed yet.
if [[ "${AXON_DROP_WAL_ON_START:-0}" == "1" ]]; then
  echo "⚠️ AXON_DROP_WAL_ON_START=1 set: deleting DuckDB WAL files before start."
  rm -f "$PROJECT_ROOT/.axon/graph_v2/"*.wal 2>/dev/null || true
fi

# Create TMUX session
tmux new-session -d -s "$TMUX_SESSION" -n "core" 

# Start Data Plane
# We use 'devenv shell' to ensure the runtime matches the pinned project toolchain.
# NEXUS v10.8: We force fastembed to use the system's libonnxruntime.so to prevent C++ aborts.
WORKER_CAP_EXPORT=""
if [[ -n "${MAX_AXON_WORKERS:-}" ]]; then
    WORKER_CAP_EXPORT="export MAX_AXON_WORKERS=\"$MAX_AXON_WORKERS\"; "
fi
EMBEDDING_PROVIDER_EXPORT=""
if [[ -n "${EMBEDDING_PROVIDER_REQUEST:-}" ]]; then
    EMBEDDING_PROVIDER_EXPORT="export AXON_EMBEDDING_PROVIDER=\"$EMBEDDING_PROVIDER_REQUEST\"; "
fi
PASS_THROUGH_EXPORTS=""
append_pass_through_export() {
    local var_name="$1"
    local value="${!var_name-}"
    if [[ -n "${value:-}" ]]; then
        local escaped=""
        printf -v escaped '%q' "$value"
        PASS_THROUGH_EXPORTS+="export ${var_name}=${escaped}; "
    fi
}
for pass_through_var in \
    AXON_ENABLE_FEDERATION_ORCHESTRATOR \
    AXON_VECTOR_WORKERS \
    AXON_GRAPH_WORKERS \
    AXON_CHUNK_BATCH_SIZE \
    AXON_FILE_VECTORIZATION_BATCH_SIZE \
    AXON_VECTOR_PREPARE_WORKERS_PER_VECTOR \
    AXON_VECTOR_PREPARE_QUEUE_BOUND \
    AXON_VECTOR_PREPARE_PIPELINE_DEPTH \
    AXON_VECTOR_READY_QUEUE_DEPTH \
    AXON_VECTOR_PERSIST_QUEUE_BOUND \
    AXON_VECTOR_MAX_INFLIGHT_PERSISTS \
    AXON_EMBED_MICRO_BATCH_MAX_ITEMS \
    AXON_EMBED_MICRO_BATCH_MAX_TOTAL_TOKENS \
    AXON_MAX_EMBED_BATCH_BYTES \
    OMP_NUM_THREADS \
    OMP_WAIT_POLICY
do
    append_pass_through_export "$pass_through_var"
done
PROFILE_EXPORT=""
if [[ "$RUNTIME_MODE" == "full" ]]; then
    PROFILE_EXPORT="export AXON_ENABLE_AUTONOMOUS_INGESTOR=true; export AXON_RUNTIME_PROFILE=full_autonomous; "
fi
PRELAUNCH_LD_LIBRARY_PATH_EXPORT=""
ORT_BUILD_LOG="$(mktemp /tmp/axon-ort-build.XXXXXX.log)"
ORT_BUILD_TARGET="nixpkgs#onnxruntime"
ORT_ARTIFACT_MANIFEST="${AXON_ORT_ARTIFACT_MANIFEST:-$PROJECT_ROOT/.axon/ort-artifacts/onnxruntime-cuda/current.json}"
ORT_OUT_PATH=""
ORT_DYLIB_PATH=""
if [[ "$EMBEDDING_PROVIDER_REQUEST" == "cuda" ]]; then
    if [[ -f "$ORT_ARTIFACT_MANIFEST" ]]; then
        ORT_DYLIB_PATH="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get("core_lib",""))' "$ORT_ARTIFACT_MANIFEST" 2>/dev/null || true)"
        CUDA_PROVIDER_PATH="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get("cuda_provider_lib",""))' "$ORT_ARTIFACT_MANIFEST" 2>/dev/null || true)"
        if [[ -n "${ORT_DYLIB_PATH:-}" && -f "$ORT_DYLIB_PATH" && -n "${CUDA_PROVIDER_PATH:-}" && -f "$CUDA_PROVIDER_PATH" ]]; then
            ORT_OUT_PATH="$(dirname "$(dirname "$ORT_DYLIB_PATH")")"
            echo "♻️ Using external CUDA ONNX Runtime artifact from manifest..."
            echo "   Manifest: $ORT_ARTIFACT_MANIFEST"
        else
            echo "⚠️ Ignoring invalid external CUDA artifact manifest: $ORT_ARTIFACT_MANIFEST"
            echo "   Falling back to nixpkgs materialization."
            ORT_DYLIB_PATH=""
        fi
    fi
    if [[ -z "${ORT_DYLIB_PATH:-}" ]]; then
        ORT_BUILD_TARGET="(import (builtins.getFlake \"nixpkgs\").outPath {
          system = builtins.currentSystem;
          config = {
            cudaSupport = true;
            allowUnfreePredicate = _: true;
          };
        }).onnxruntime"
        echo "🔧 Materializing CUDA-enabled ONNX Runtime from nixpkgs..."
    fi
fi
if [[ -z "${ORT_DYLIB_PATH:-}" ]]; then
    if [[ "$ORT_BUILD_TARGET" == "nixpkgs#onnxruntime" ]]; then
        ORT_OUT_PATH="$(nix build --no-link --print-out-paths "$ORT_BUILD_TARGET" 2>&1 | tee "$ORT_BUILD_LOG" | tail -n 1)"
    else
        ORT_OUT_PATH="$(nix build --impure --no-link --print-out-paths --expr "$ORT_BUILD_TARGET" 2>&1 | tee "$ORT_BUILD_LOG" | tail -n 1)"
    fi
    if [[ -z "${ORT_OUT_PATH:-}" || ! -f "$ORT_OUT_PATH/lib/libonnxruntime.so" ]]; then
        echo "❌ Unable to materialize a valid ONNX Runtime output path."
        if [[ "$EMBEDDING_PROVIDER_REQUEST" == "cuda" ]]; then
            echo "   Tried to build nixpkgs onnxruntime with cudaSupport=true."
            if rg -q "unexpected eof while reading|cannot download .*cudnn|developer\\.download\\.nvidia\\.com" "$ORT_BUILD_LOG" 2>/dev/null; then
                echo "   The failure came from downloading NVIDIA CUDA/cuDNN artifacts, not from Axon itself."
                echo "   Retry the start once connectivity to developer.download.nvidia.com is stable."
            fi
        fi
        echo "   Build log: $ORT_BUILD_LOG"
        exit 1
    fi
    ORT_DYLIB_PATH="$ORT_OUT_PATH/lib/libonnxruntime.so"
fi
if [[ "$EMBEDDING_PROVIDER_REQUEST" == "cuda" ]]; then
    ORT_LIB_DIR="$(dirname "$ORT_DYLIB_PATH")"
    CUDA_LD_PATH_SEGMENTS=()
    if [[ -d "$ORT_LIB_DIR" ]]; then
        CUDA_LD_PATH_SEGMENTS+=("$ORT_LIB_DIR")
    fi
    if [[ -d "/usr/lib/wsl/lib" ]]; then
        CUDA_LD_PATH_SEGMENTS+=("/usr/lib/wsl/lib")
    fi
    if [[ ${#CUDA_LD_PATH_SEGMENTS[@]} -gt 0 ]]; then
        CUDA_LD_PREFIX="$(IFS=:; echo "${CUDA_LD_PATH_SEGMENTS[*]}")"
        if [[ -n "${LD_LIBRARY_PATH:-}" ]]; then
            PRELAUNCH_LD_LIBRARY_PATH_EXPORT="export LD_LIBRARY_PATH=\"$CUDA_LD_PREFIX:$LD_LIBRARY_PATH\"; "
        else
            PRELAUNCH_LD_LIBRARY_PATH_EXPORT="export LD_LIBRARY_PATH=\"$CUDA_LD_PREFIX\"; "
        fi
    fi
fi
if [[ "$EMBEDDING_PROVIDER_REQUEST" == "cuda" && ! -f "$ORT_OUT_PATH/lib/libonnxruntime_providers_cuda.so" ]]; then
    echo "⚠️ The selected ONNX Runtime package does not include libonnxruntime_providers_cuda.so."
    echo "   CUDA embedding cannot activate with this system ORT package; Axon will fall back to CPU diagnostics."
fi
tmux send-keys -t "$TMUX_SESSION:core" "devenv shell -- bash -lc 'export AXON_PROJECTS_ROOT=\"$PROJECTS_ROOT\"; export AXON_WATCH_DIR=\"$WATCH_ROOT\"; export AXON_PROJECT_ROOT=\"$PROJECT_ROOT\"; export AXON_RUNTIME_MODE=\"$RUNTIME_MODE\"; export AXON_MCP_MUTATION_JOBS=1; ${PROFILE_EXPORT}${WORKER_CAP_EXPORT}${EMBEDDING_PROVIDER_EXPORT}${PASS_THROUGH_EXPORTS}${PRELAUNCH_LD_LIBRARY_PATH_EXPORT}export ORT_STRATEGY=system; export ORT_DYLIB_PATH=\"$ORT_DYLIB_PATH\"; echo \"🚀 Starting Axon Core...\"; RUST_LOG=info bin/axon-core'" C-m

if [ "$START_DASHBOARD" = "1" ]; then
    # Start Visualization Plane
    tmux new-window -t "$TMUX_SESSION" -n "nexus"
    if [[ "$SKIP_ELIXIR_PREWARM" == "1" ]]; then
        tmux send-keys -t "$TMUX_SESSION:nexus" "cd \"$PROJECT_ROOT\" && devenv shell -- bash -lc \"cd '$PROJECT_ROOT/src/dashboard' && PHX_PORT=$PHX_PORT HYDRA_TCP_PORT=$HYDRA_TCP_PORT AXON_SQL_URL=$AXON_SQL_URL AXON_PROJECT_CODE=$PROJECT_CODE AXON_WATCH_DIR=$WATCH_ROOT elixir --name ${ELIXIR_NODE_NAME}@127.0.0.1 --cookie axon_secret -S mix phx.server\"" C-m
    else
        tmux send-keys -t "$TMUX_SESSION:nexus" "cd \"$PROJECT_ROOT\" && devenv shell -- bash -lc \"cd '$PROJECT_ROOT/src/dashboard' && mix local.hex --force >/dev/null && mix local.rebar --force >/dev/null && PHX_PORT=$PHX_PORT HYDRA_TCP_PORT=$HYDRA_TCP_PORT AXON_SQL_URL=$AXON_SQL_URL AXON_PROJECT_CODE=$PROJECT_CODE AXON_WATCH_DIR=$WATCH_ROOT elixir --name ${ELIXIR_NODE_NAME}@127.0.0.1 --cookie axon_secret -S mix phx.server\"" C-m
    fi
fi

echo "⏳ Waiting for Axon Infrastructure to rise (Timeout: ${STARTUP_TIMEOUT_S}s)..."

# Parallel wait loop for both services
CORE_READY=false
DASHBOARD_READY=false

# Wait up to STARTUP_TIMEOUT_S * 1s
for ((i=1; i<=STARTUP_TIMEOUT_S; i++)); do
    if [ "$CORE_READY" = false ]; then
        if probe_sql_gateway && verify_mcp_http; then
            echo "✅ Axon Data Plane and MCP Gateway are Ready."
            CORE_READY=true
        fi
    fi

    if [ "$START_DASHBOARD" = "1" ] && [ "$DASHBOARD_READY" = false ]; then
        # Dashboard is ready if the Phoenix port is responding
        if nc -z localhost $PHX_PORT 2>/dev/null; then
            echo "✅ Axon Dashboard is Ready."
            DASHBOARD_READY=true
        fi
    fi

    if [ "$START_DASHBOARD" = "0" ]; then
        DASHBOARD_READY=true
    fi

    if [ "$CORE_READY" = true ] && [ "$DASHBOARD_READY" = true ]; then
        break
    fi
    
    sleep 1
done

if [ "$CORE_READY" = false ]; then echo "⚠️ Timeout waiting for Axon Core."; fi
if [ "$START_DASHBOARD" = "1" ] && [ "$DASHBOARD_READY" = false ]; then echo "⚠️ Timeout waiting for Axon Dashboard."; fi

if [ "$CORE_READY" = false ] || [ "$DASHBOARD_READY" = false ]; then
    echo "❌ Axon did not reach a fully ready state within the startup budget."
    echo "   Inspect TMUX with: tmux attach -t axon"
    exit 1
fi

if [ "$CORE_READY" = true ]; then
    echo ""
    echo "🧪 Verifying live SQL schema..."
    if ! verify_sql_gateway; then
        echo "❌ Axon Core exposed its port but failed the live schema check."
        echo "   Inspect TMUX with: tmux attach -t axon"
        exit 1
    fi
    echo "✅ Live SQL schema check succeeded."
fi

echo ""
echo "⚙️ Running MCP End-to-End Verification..."
if [ -x "bin/axon-mcp-tunnel" ] && ! echo '{"jsonrpc": "2.0", "method": "tools/list", "params": {}, "id": 1}' | bin/axon-mcp-tunnel | grep -q "axon_query"; then
    echo "❌ MCP tunnel verification failed."
    echo "   Inspect the TMUX session ($TMUX_SESSION) to debug."
elif [ -x "bin/axon-mcp-tunnel" ]; then
    echo "✅ MCP tunnel verification succeeded."
elif verify_mcp_http; then
    echo "✅ MCP HTTP verification succeeded."
else
    echo "❌ MCP HTTP verification failed."
    echo "   Inspect the TMUX session ($TMUX_SESSION) to debug."
    exit 1
fi

if [ "$RUN_MCP_TESTS" = "1" ]; then
    echo ""
    echo "⏳ Waiting for initial ingestion to stabilize..."
    for i in {1..30}; do
        pending=$(curl -sS -X POST "http://127.0.0.1:$HYDRA_HTTP_PORT/sql" -H 'content-type: application/json' -d '{"query":"SELECT count(*) FROM File WHERE status IN ('\''pending'\'', '\''indexing'\'')"}' 2>/dev/null | grep -o '[0-9]\+' | head -n1 || echo "0")
        indexed=$(curl -sS -X POST "http://127.0.0.1:$HYDRA_HTTP_PORT/sql" -H 'content-type: application/json' -d '{"query":"SELECT count(*) FROM File WHERE status IN ('\''indexed'\'', '\''indexed_degraded'\'')"}' 2>/dev/null | grep -o '[0-9]\+' | head -n1 || echo "0")
        if [ "$pending" = "0" ]; then
            echo "✅ Ingestion stabilized ($indexed files indexed)."
            break
        fi
        sleep 2
    done

    echo "🧪 Running MCP Quality Gate Validation..."
    if devenv shell -- bash -lc './scripts/mcp_quality_gate.sh --allow-mutations'; then
        echo "✅ MCP Quality Gate passed."
    else
        echo "❌ MCP Quality Gate failed."
        exit 1
    fi
fi

# 6. Final Report
echo ""
echo "🛡️ Axon is rising in TMUX session '$TMUX_SESSION'."
echo "To view processes: 'tmux attach -t $TMUX_SESSION'"
if [ "$START_DASHBOARD" = "1" ]; then
    echo "Dashboard: http://$WSL_IP:44127/cockpit"
fi
echo "SQL Gateway: http://$WSL_IP:44129/sql"
echo "MCP Server: http://$WSL_IP:44129/mcp"
echo "Stop services with: ./scripts/stop.sh"
echo ""
