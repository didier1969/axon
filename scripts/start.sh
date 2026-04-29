#!/bin/bash
set -euo pipefail

# Axon v2 - Daily Start Script
# Canonical daily workflow entrypoint for running Axon in TMUX.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
DEFAULT_PROJECTS_ROOT="$(cd "$PROJECT_ROOT/.." && pwd)"
# shellcheck source=scripts/lib/axon-instance.sh
source "$PROJECT_ROOT/scripts/lib/axon-instance.sh"
# shellcheck source=scripts/lib/axon-role-layout.sh
source "$PROJECT_ROOT/scripts/lib/axon-role-layout.sh"
# shellcheck source=scripts/lib/axon-resource-policy.sh
source "$PROJECT_ROOT/scripts/lib/axon-resource-policy.sh"
# shellcheck source=scripts/lib/axon-ort-runtime.sh
source "$PROJECT_ROOT/scripts/lib/axon-ort-runtime.sh"
# shellcheck source=scripts/lib/axon-version.sh
source "$PROJECT_ROOT/scripts/lib/axon-version.sh"
cd "$PROJECT_ROOT"

axon_load_worktree_env "$PROJECT_ROOT"
axon_resolve_instance "$PROJECT_ROOT" "$(basename "$PROJECT_ROOT")"
axon_resolve_resource_policy "$AXON_INSTANCE_KIND"
axon_resolve_version "$PROJECT_ROOT"

LIVE_RELEASE_CURRENT_MANIFEST="$PROJECT_ROOT/.axon/live-release/current.json"
LIVE_RELEASE_PENDING_MANIFEST="$PROJECT_ROOT/.axon/live-release/pending.json"
LIVE_RELEASE_MANIFEST_SOURCE="${AXON_LIVE_RELEASE_MANIFEST:-$LIVE_RELEASE_CURRENT_MANIFEST}"
LIVE_RELEASE_ACTIVE=0
LIVE_RELEASE_ARTIFACT=""
LIVE_RELEASE_BUILD_INFO=""
LIVE_RELEASE_BRAIN_ARTIFACT=""
LIVE_RELEASE_BRAIN_BUILD_INFO=""
LIVE_RELEASE_INDEXER_ARTIFACT=""
LIVE_RELEASE_INDEXER_BUILD_INFO=""

if [[ "$AXON_INSTANCE_KIND" == "live" \
    && -z "${AXON_LIVE_RELEASE_MANIFEST:-}" \
    && -f "$LIVE_RELEASE_PENDING_MANIFEST" ]]; then
    echo "❌ Refusing live start while a staged release is pending: $LIVE_RELEASE_PENDING_MANIFEST" >&2
    echo "   Complete promotion with scripts/axon promote-live --manifest <candidate> --restart-live," >&2
    echo "   or roll back/clear the pending manifest explicitly." >&2
    exit 1
fi

load_live_release_current() {
    [[ "$AXON_INSTANCE_KIND" == "live" && -f "$LIVE_RELEASE_MANIFEST_SOURCE" ]] || return 1

    local payload=""
    payload="$(python3 - "$LIVE_RELEASE_MANIFEST_SOURCE" <<'PY'
import json, pathlib, sys
manifest = json.loads(pathlib.Path(sys.argv[1]).read_text())
runtime = manifest.get("runtime_version") or {}
artifact = manifest.get("artifact") or {}
artifacts = manifest.get("artifacts") or {}
brain = artifacts.get("axon-brain") or {}
indexer = artifacts.get("axon-indexer") or {}
fields = [
    artifact.get("path", ""),
    artifact.get("build_info_path", "") or "",
    brain.get("path", ""),
    brain.get("build_info_path", "") or "",
    indexer.get("path", ""),
    indexer.get("build_info_path", "") or "",
    runtime.get("release_version", ""),
    runtime.get("package_version", ""),
    runtime.get("build_id", ""),
    runtime.get("install_generation", ""),
]
print("\n".join(fields))
PY
)"

    mapfile -t live_release_fields <<<"$payload"
    LIVE_RELEASE_ARTIFACT="${live_release_fields[0]:-}"
    LIVE_RELEASE_BUILD_INFO="${live_release_fields[1]:-}"
    LIVE_RELEASE_BRAIN_ARTIFACT="${live_release_fields[2]:-}"
    LIVE_RELEASE_BRAIN_BUILD_INFO="${live_release_fields[3]:-}"
    LIVE_RELEASE_INDEXER_ARTIFACT="${live_release_fields[4]:-}"
    LIVE_RELEASE_INDEXER_BUILD_INFO="${live_release_fields[5]:-}"

    [[ -n "$LIVE_RELEASE_BRAIN_ARTIFACT" && -f "$LIVE_RELEASE_BRAIN_ARTIFACT" ]] || {
        echo "❌ Live split manifest points to a missing brain artifact: ${LIVE_RELEASE_BRAIN_ARTIFACT:-<empty>}"
        exit 1
    }
    [[ -n "$LIVE_RELEASE_INDEXER_ARTIFACT" && -f "$LIVE_RELEASE_INDEXER_ARTIFACT" ]] || {
        echo "❌ Live split manifest points to a missing indexer artifact: ${LIVE_RELEASE_INDEXER_ARTIFACT:-<empty>}"
        exit 1
    }

    AXON_RELEASE_VERSION="${live_release_fields[6]:-$AXON_RELEASE_VERSION}"
    AXON_PACKAGE_VERSION="${live_release_fields[7]:-$AXON_PACKAGE_VERSION}"
    AXON_BUILD_ID="${live_release_fields[8]:-$AXON_BUILD_ID}"
    AXON_INSTALL_GENERATION="${live_release_fields[9]:-$AXON_INSTALL_GENERATION}"
    export AXON_RELEASE_VERSION AXON_PACKAGE_VERSION AXON_BUILD_ID AXON_INSTALL_GENERATION
    LIVE_RELEASE_ACTIVE=1
    return 0
}

load_live_release_current || true

WATCH_ROOT="${AXON_WATCH_DIR:-$DEFAULT_PROJECTS_ROOT}"
PROJECTS_ROOT="${AXON_PROJECTS_ROOT:-$WATCH_ROOT}"
PROJECT_CODE="${AXON_PROJECT_CODE:-}"
if [[ -z "$PROJECT_CODE" && -f "$PROJECT_ROOT/.axon/meta.json" ]]; then
    PROJECT_CODE="$(python3 -c 'import json; print(json.load(open("'"$PROJECT_ROOT"'/.axon/meta.json")).get("code",""))' 2>/dev/null || true)"
fi
PROJECT_CODE="${PROJECT_CODE:-$(basename "$PROJECT_ROOT")}"
RUNTIME_MODE="${AXON_RUNTIME_MODE:-indexer_graph}"
RUNTIME_SHADOW_ROLE="$(axon_runtime_shadow_role)"
RUNTIME_SHADOW_ONLY="${AXON_SPLIT_SHADOW_ONLY:-0}"
RUNTIME_EXECUTABLE="bin/axon-core"
RUNTIME_EXECUTABLE_NAME="$(axon_runtime_binary_name "$RUNTIME_SHADOW_ROLE")"
SELECTED_DEBUG_RUNTIME_BIN=""
SELECTED_RELEASE_RUNTIME_BIN=""
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

instance_runtime_pids() {
    local pids=""
    local pid=""
    local port_pid=""

    if [[ -f "$AXON_PID_FILE" ]]; then
        pid="$(cat "$AXON_PID_FILE" 2>/dev/null || true)"
        if [[ -n "$pid" ]]; then
            pids="$pids $pid"
        fi
    fi

    if ! axon_role_is_indexer "$RUNTIME_SHADOW_ROLE"; then
        port_pid="$(ss -ltnp 2>/dev/null | awk -v p="$HYDRA_HTTP_PORT" '
            $1 == "LISTEN" {
                split($4, addr_parts, ":")
                if (addr_parts[length(addr_parts)] != p) {
                    next
                }
                match($0, /pid=([0-9]+)/, m)
                if (m[1] != "") {
                    print m[1]
                    exit
                }
            }' || true)"
        if [[ -n "$port_pid" ]]; then
            pids="$pids $port_pid"
        fi
    fi

    echo "$pids" | tr ' ' '\n' | awk 'NF' | sort -u
}

cleanup_stale_runtime_state() {
    rm -f "$AXON_TELEMETRY_SOCK" "$AXON_MCP_SOCK" "$AXON_PID_FILE" "$AXON_RUNTIME_STATE_FILE"
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

    [[ "$response" == *"axon_query"* || "$response" == *'"query"'* ]]
}

has_live_runtime_dataplane() {
    local pid
    for pid in $(instance_runtime_pids); do
        if [[ -n "$pid" && -e "/proc/$pid" ]]; then
            return 0
        fi
    done

    if axon_role_is_indexer "$RUNTIME_SHADOW_ROLE"; then
        if [[ -S "$AXON_TELEMETRY_SOCK" ]]; then
            return 0
        fi
        return 1
    fi

    if nc -z localhost "$HYDRA_HTTP_PORT" 2>/dev/null; then
        return 0
    fi

    return 1
}

has_live_dashboard_dataplane() {
    if [[ "$START_DASHBOARD" != "1" ]]; then
        return 0
    fi

    if nc -z localhost "$PHX_PORT" 2>/dev/null; then
        return 0
    fi

    return 1
}

launch_dashboard_window() {
    [[ "$START_DASHBOARD" == "1" ]] || return 0

    if tmux list-windows -t "$TMUX_SESSION" 2>/dev/null | grep -qE '^[0-9]+: nexus'; then
        tmux kill-window -t "$TMUX_SESSION:nexus" 2>/dev/null || true
    fi

    tmux new-window -t "$TMUX_SESSION" -n "nexus"
    if [[ "$SKIP_ELIXIR_PREWARM" == "1" ]]; then
        tmux send-keys -t "$TMUX_SESSION:nexus" "cd \"$PROJECT_ROOT\" && devenv shell --no-reload --no-tui -- bash -lc \"cd '$PROJECT_ROOT/src/dashboard' && PHX_PORT=$PHX_PORT HYDRA_TCP_PORT=$HYDRA_TCP_PORT AXON_SQL_URL=$AXON_SQL_URL AXON_PROJECT_CODE=$PROJECT_CODE AXON_WATCH_DIR=$WATCH_ROOT AXON_INSTANCE_KIND=$AXON_INSTANCE_KIND AXON_RUNTIME_IDENTITY=$AXON_RUNTIME_IDENTITY AXON_MUTATION_POLICY=$AXON_MUTATION_POLICY elixir --name ${ELIXIR_NODE_NAME}@127.0.0.1 --cookie axon_secret -S mix phx.server\"" C-m
    else
        tmux send-keys -t "$TMUX_SESSION:nexus" "cd \"$PROJECT_ROOT\" && devenv shell --no-reload --no-tui -- bash -lc \"cd '$PROJECT_ROOT/src/dashboard' && mix local.hex --force >/dev/null && mix local.rebar --force >/dev/null && PHX_PORT=$PHX_PORT HYDRA_TCP_PORT=$HYDRA_TCP_PORT AXON_SQL_URL=$AXON_SQL_URL AXON_PROJECT_CODE=$PROJECT_CODE AXON_WATCH_DIR=$WATCH_ROOT AXON_INSTANCE_KIND=$AXON_INSTANCE_KIND AXON_RUNTIME_IDENTITY=$AXON_RUNTIME_IDENTITY AXON_MUTATION_POLICY=$AXON_MUTATION_POLICY elixir --name ${ELIXIR_NODE_NAME}@127.0.0.1 --cookie axon_secret -S mix phx.server\"" C-m
    fi
}

probe_writer_guard() {
    local label="$1"
    local lock_path="$2"
    local owner=""

    command -v flock >/dev/null 2>&1 || return 0
    [[ -f "$lock_path" ]] || return 0

    # Advisory preflight only. Rust startup enforcement remains authoritative.
    # Do not create lockfiles here, otherwise a refused or aborted shell launch
    # would leave stale-looking scaffolding before the runtime even starts.
    exec {guard_fd}<>"$lock_path"
    if ! flock -n "$guard_fd"; then
        owner="$(tr '\n' ';' < "$lock_path" 2>/dev/null || true)"
        echo "❌ Preflight: $label writer guard is already held."
        echo "   Lock: $lock_path"
        if [[ -n "$owner" ]]; then
            echo "   Recorded owner: $owner"
        fi
        echo "   Startup aborted before runtime launch. Rust writer enforcement remains authoritative."
        exit 1
    fi
    flock -u "$guard_fd" || true
    exec {guard_fd}>&-
}

selected_writer_guards() {
    if axon_role_is_brain "$RUNTIME_SHADOW_ROLE"; then
        printf 'SOLL %s\n' "$AXON_DB_ROOT/.axon-soll.writer.lock"
    elif axon_role_is_indexer "$RUNTIME_SHADOW_ROLE"; then
        printf 'IST %s\n' "$AXON_DB_ROOT/.axon-ist.writer.lock"
    else
        printf 'SOLL %s\n' "$AXON_DB_ROOT/.axon-soll.writer.lock"
        printf 'IST %s\n' "$AXON_DB_ROOT/.axon-ist.writer.lock"
    fi
}

require_writer_guard_probe() {
    local label="$1"
    local lock_path="$2"
    local owner=""

    command -v flock >/dev/null 2>&1 || {
        echo "❌ Strict preflight requires flock for $label writer guard verification."
        exit 1
    }

    if [[ ! -f "$lock_path" ]]; then
        echo "❌ Strict preflight: $label writer guard lockfile is missing."
        echo "   Lock: $lock_path"
        echo "   Combined-runtime rollback is unsupported; split writer ownership must be explicit."
        exit 1
    fi

    exec {guard_fd}<>"$lock_path"
    if ! flock -n "$guard_fd"; then
        owner="$(tr '\n' ';' < "$lock_path" 2>/dev/null || true)"
        echo "❌ Strict preflight: $label writer guard is still held."
        echo "   Lock: $lock_path"
        if [[ -n "$owner" ]]; then
            echo "   Recorded owner: $owner"
        fi
        echo "   Combined-runtime rollback aborted until writer ownership is provably released."
        exit 1
    fi
    flock -u "$guard_fd" || true
    exec {guard_fd}>&-
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --brain-only|--brainonly)
            RUNTIME_MODE="brain_only"
            RUNTIME_SHADOW_ROLE="brain"
            ;;
        --indexer-graph|--indexergraph)
            RUNTIME_MODE="indexer_graph"
            RUNTIME_SHADOW_ROLE="indexer"
            ;;
        --indexer-vector|--indexervector)
            RUNTIME_MODE="indexer_vector"
            RUNTIME_SHADOW_ROLE="indexer"
            ;;
        --indexer-full|--indexerfull)
            RUNTIME_MODE="indexer_full"
            RUNTIME_SHADOW_ROLE="indexer"
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
Usage: ./scripts/start.sh [--brain-only|--indexer-graph|--indexer-vector|--indexer-full] [--no-dashboard] [--skip-mcp-tests] [--skip-elixir-prewarm]

Modes:
  --brain-only      MCP + dashboard authority only, without graph or vector workers
  --indexer-graph   Indexer with graph ingestion only, without semantic/vector workers
  --indexer-vector  Indexer with semantic/vector workers only, without graph ingestion
  --indexer-full    Indexer with graph + semantic/vector workloads

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

RUNTIME_REACTIVATION_PATH="default"
RUNTIME_EXECUTABLE_NAME="$(axon_runtime_binary_name "$RUNTIME_SHADOW_ROLE")"

CARGO_TARGET_ROOT="${CARGO_TARGET_DIR:-$PROJECT_ROOT/.axon/cargo-target}"
DEVENV_DEBUG_BIN_ROOT="$CARGO_TARGET_ROOT/debug"

case "$RUNTIME_EXECUTABLE_NAME" in
    axon-brain)
        if [[ "$AXON_INSTANCE_KIND" == "live" && -x "$PROJECT_ROOT/bin/axon-brain" ]]; then
            RUNTIME_EXECUTABLE="bin/axon-brain"
        else
            RUNTIME_EXECUTABLE="${DEVENV_DEBUG_BIN_ROOT#$PROJECT_ROOT/}/axon-brain"
        fi
        SELECTED_DEBUG_RUNTIME_BIN="$PROJECT_ROOT/$RUNTIME_EXECUTABLE"
        SELECTED_RELEASE_RUNTIME_BIN="$CARGO_TARGET_ROOT/release/axon-brain"
        ;;
    axon-indexer)
        if [[ "$AXON_INSTANCE_KIND" == "live" && -x "$PROJECT_ROOT/bin/axon-indexer" ]]; then
            RUNTIME_EXECUTABLE="bin/axon-indexer"
        else
            RUNTIME_EXECUTABLE="${DEVENV_DEBUG_BIN_ROOT#$PROJECT_ROOT/}/axon-indexer"
        fi
        SELECTED_DEBUG_RUNTIME_BIN="$PROJECT_ROOT/$RUNTIME_EXECUTABLE"
        SELECTED_RELEASE_RUNTIME_BIN="$CARGO_TARGET_ROOT/release/axon-indexer"
        ;;
esac

axon_apply_runtime_role_layout "$PROJECT_ROOT" "$RUNTIME_SHADOW_ROLE" "$RUNTIME_EXECUTABLE_NAME"

export AXON_SKIP_ELIXIR_PREWARM="$SKIP_ELIXIR_PREWARM"

if [[ "$RUNTIME_MODE" != "indexer_full" && "$RUNTIME_MODE" != "indexer_vector" ]]; then
    EMBEDDING_PROVIDER_REQUEST="cpu"
elif [[ -n "${AXON_EMBEDDING_PROVIDER:-}" ]]; then
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
    if [[ "$RUNTIME_MODE" == "indexer_full" ]]; then
        STARTUP_TIMEOUT_S=240
    else
        STARTUP_TIMEOUT_S=120
    fi
fi

if ! command -v tmux >/dev/null 2>&1; then
    echo "❌ tmux is required to start Axon via scripts/start.sh"
    exit 1
fi

run_devenv_shell() {
    local cmd="$1"
    local attempt=1
    while true; do
        if devenv shell --no-reload --no-tui -- bash -lc "$cmd"; then
            return 0
        fi
        if (( attempt >= 2 )); then
            return 1
        fi
        echo "⚠️ devenv shell failed; running 'devenv gc' once before retrying..."
        devenv gc >/dev/null 2>&1 || true
        attempt=$((attempt + 1))
        sleep 1
    done
}

echo "📦 Validating Devenv environment..."
run_devenv_shell './scripts/validate-devenv.sh'

if [[ "$SKIP_ELIXIR_PREWARM" == "1" ]]; then
    echo "⏭️ Skipping Elixir pre-warm (AXON_SKIP_ELIXIR_PREWARM=1)."
else
    echo "📦 Pre-warming Elixir environment (Hex/Rebar)..."
    run_devenv_shell "cd '$PROJECT_ROOT/src/dashboard' && mix local.hex --force >/dev/null && mix local.rebar --force >/dev/null"
fi

if [ ! -x "$RUNTIME_EXECUTABLE" ]; then
    echo "❌ Missing $RUNTIME_EXECUTABLE_NAME"
    echo "   Expected executable: $RUNTIME_EXECUTABLE"
    if [[ "$RUNTIME_EXECUTABLE_NAME" == "axon-core" ]]; then
        echo "   Run ./scripts/setup.sh first."
    else
        echo "   Run cargo build --manifest-path src/axon-core/Cargo.toml --bin $RUNTIME_EXECUTABLE_NAME first."
    fi
    exit 1
fi

if [[ -n "$SELECTED_DEBUG_RUNTIME_BIN" ]] && find "$PROJECT_ROOT/src/axon-core/src" \
    -type f \( -name '*.rs' -o -name 'Cargo.toml' \) \
    -newer "$SELECTED_DEBUG_RUNTIME_BIN" -print -quit | grep -q .; then
    echo "⚠️ Detected newer axon-core sources than $SELECTED_DEBUG_RUNTIME_BIN"
    echo "   Rebuilding selected runtime role binary..."
    if ! run_devenv_shell "cd '$PROJECT_ROOT/src/axon-core' && cargo build --bin $RUNTIME_EXECUTABLE_NAME --release"; then
        echo "❌ Failed to rebuild $RUNTIME_EXECUTABLE_NAME"
        exit 1
    fi
    if [[ "$AXON_INSTANCE_KIND" == "live" && -n "$SELECTED_RELEASE_RUNTIME_BIN" && -f "$SELECTED_RELEASE_RUNTIME_BIN" ]]; then
        echo "🔄 Updating live $RUNTIME_EXECUTABLE from rebuilt runtime role artifact..."
        mkdir -p "$(dirname "$RUNTIME_EXECUTABLE")"
        install -m 755 "$SELECTED_RELEASE_RUNTIME_BIN" "$RUNTIME_EXECUTABLE"
    fi
fi

REUSE_RUNNING_CORE=0
if tmux has-session -t "$TMUX_SESSION" 2>/dev/null; then
    DELETED_EXE_PIDS=$(for pid in $(instance_runtime_pids); do
        exe=$(readlink -f "/proc/$pid/exe" 2>/dev/null || true)
        if [[ "$exe" == *"(deleted)"* ]]; then
            echo "$pid"
        fi
    done)

    if [ -n "${DELETED_EXE_PIDS:-}" ]; then
        echo "⚠️ Found Axon processes still running on deleted executables: $DELETED_EXE_PIDS"
        echo "   Resetting stale $RUNTIME_SHADOW_ROLE runtime state before restart..."
        tmux kill-session -t "$TMUX_SESSION" 2>/dev/null || true
        cleanup_stale_runtime_state
    fi

    if has_live_runtime_dataplane && { axon_role_is_indexer "$RUNTIME_SHADOW_ROLE" || verify_mcp_http; }; then
        if [[ "$START_DASHBOARD" == "1" ]] && ! has_live_dashboard_dataplane; then
            echo "⚠️ Axon core is healthy in TMUX session '$TMUX_SESSION' but dashboard is absent."
            echo "   Relaunching nexus window..."
            launch_dashboard_window
            REUSE_RUNNING_CORE=1
        else
            echo "ℹ️ Axon is already running in TMUX session '$TMUX_SESSION'."
            echo "   Attach with: tmux attach -t $TMUX_SESSION"
            exit 0
        fi
    else
        echo "⚠️ Found stale TMUX session '$TMUX_SESSION' without a healthy data plane. Resetting local runtime state..."
        tmux kill-session -t "$TMUX_SESSION" 2>/dev/null || true
        cleanup_stale_runtime_state
    fi
elif [[ -S "$AXON_TELEMETRY_SOCK" || -S "$AXON_MCP_SOCK" || -f "$AXON_PID_FILE" ]]; then
    echo "⚠️ Found stale local runtime state without a TMUX session. Cleaning sockets/pid and continuing..."
    cleanup_stale_runtime_state
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
LEGACY_RELEASE_BIN="$PROJECT_ROOT/src/axon-core/target/release/axon-core"
DEVENV_RELEASE_BIN="$CARGO_TARGET_ROOT/release/axon-core"
DEVENV_TUNNEL_BIN="$CARGO_TARGET_ROOT/release/axon-mcp-tunnel"

rebuild_core_release() {
    echo "🔧 Rebuilding axon-core release inside Devenv..."
    if ! run_devenv_shell "cd '$PROJECT_ROOT/src/axon-core' && cargo build --release"; then
        echo "❌ Automatic Devenv rebuild failed."
        return 1
    fi
    return 0
}

rebuild_tunnel_release() {
    echo "🔧 Rebuilding axon-mcp-tunnel release inside Devenv..."
    if ! run_devenv_shell "cd '$PROJECT_ROOT/src/axon-mcp-tunnel' && cargo build --release"; then
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

verify_role_ready() {
    if axon_role_is_indexer "$RUNTIME_SHADOW_ROLE"; then
        [[ -S "$AXON_TELEMETRY_SOCK" ]] || return 1
        instance_runtime_pids | awk 'NF { found=1; exit } END { exit(found ? 0 : 1) }'
    else
        probe_sql_gateway && verify_mcp_http
    fi
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

if [[ "${AXON_SKIP_BIN_SYNC:-0}" != "1" ]]; then
    if [[ "$LIVE_RELEASE_ACTIVE" -eq 1 ]]; then
        echo "🔄 Updating live split binaries from promoted artifacts..."
        mkdir -p bin
        install -m 755 "$LIVE_RELEASE_BRAIN_ARTIFACT" bin/axon-brain
        install -m 755 "$LIVE_RELEASE_INDEXER_ARTIFACT" bin/axon-indexer
        if [[ -n "$LIVE_RELEASE_BRAIN_BUILD_INFO" && -f "$LIVE_RELEASE_BRAIN_BUILD_INFO" ]]; then
            install -m 644 "$LIVE_RELEASE_BRAIN_BUILD_INFO" "$(axon_build_info_path_for "$PROJECT_ROOT" "axon-brain")"
        fi
        if [[ -n "$LIVE_RELEASE_INDEXER_BUILD_INFO" && -f "$LIVE_RELEASE_INDEXER_BUILD_INFO" ]]; then
            install -m 644 "$LIVE_RELEASE_INDEXER_BUILD_INFO" "$(axon_build_info_path_for "$PROJECT_ROOT" "axon-indexer")"
        fi
    elif [[ "$RUNTIME_EXECUTABLE_NAME" == "axon-core" && -f "$DEVENV_RELEASE_BIN" ]]; then
        echo "🔄 Updating bin/axon-core safely..."
        mkdir -p bin && install -m 755 "$DEVENV_RELEASE_BIN" bin/axon-core
        AXON_BUILD_ID="$(axon_workspace_build_id "$PROJECT_ROOT")"
        export AXON_BUILD_ID
        axon_write_export_file "$AXON_BUILD_INFO_FILE" \
            AXON_RELEASE_VERSION "$AXON_RELEASE_VERSION" \
            AXON_BUILD_ID "$AXON_BUILD_ID" \
            AXON_PACKAGE_VERSION "$AXON_PACKAGE_VERSION" \
            AXON_INSTALL_GENERATION "$AXON_INSTALL_GENERATION"
    fi
fi

if [[ "${AXON_SKIP_BIN_SYNC:-0}" != "1" ]] \
    && ! axon_role_is_indexer "$RUNTIME_SHADOW_ROLE" \
    && [ -f "$DEVENV_TUNNEL_BIN" ]; then
    echo "🔄 Updating bin/axon-mcp-tunnel safely..."
    mkdir -p bin && install -m 755 "$DEVENV_TUNNEL_BIN" bin/axon-mcp-tunnel
fi

echo "🚀 Starting Axon in TMUX session '$TMUX_SESSION'..."
echo "📂 Watch root: $WATCH_ROOT"
echo "🗂️ Projects root: $PROJECTS_ROOT"
echo "🧭 Runtime mode: $RUNTIME_MODE"
echo "🎭 Shadow role: $RUNTIME_SHADOW_ROLE"
echo "⚙️ Runtime binary: $RUNTIME_EXECUTABLE_NAME ($RUNTIME_EXECUTABLE)"
if [[ "$RUNTIME_SHADOW_ONLY" == "1" ]]; then
    echo "🧪 Split path: shadow-only / non-promotable until gates are green"
fi
echo "🧩 Instance kind: $AXON_INSTANCE_KIND"
echo "📊 Resource policy: priority=$AXON_RESOURCE_PRIORITY budget=$AXON_BACKGROUND_BUDGET_CLASS gpu=$AXON_GPU_ACCESS_POLICY watcher=$AXON_WATCHER_POLICY"
echo "🛠️ Worker cap: ${MAX_AXON_WORKERS:-auto} / Queue budget bytes: ${AXON_QUEUE_MEMORY_BUDGET_BYTES:-auto}"
echo "🏷️ Release version: $AXON_RELEASE_VERSION"
echo "🧱 Build id: $AXON_BUILD_ID"
if [[ "${AXON_PUBLIC_ENDPOINTS_AVAILABLE:-0}" == "1" ]]; then
    echo "🌐 Advertised host: $AXON_PUBLIC_HOST ($AXON_PUBLIC_HOST_SOURCE)"
else
    echo "🌐 Advertised host: unresolved"
fi
export SQL_URL="$AXON_SQL_URL"

mkdir -p "$AXON_DB_ROOT" "$AXON_RUN_ROOT"
# Clean only the sockets used by the selected runtime
rm -f "$AXON_TELEMETRY_SOCK" "$AXON_MCP_SOCK"
rm -f "$AXON_PID_FILE"

while read -r guard_label guard_path; do
    [[ -n "${guard_label:-}" ]] || continue
    probe_writer_guard "$guard_label" "$guard_path"
done < <(selected_writer_guards)

axon_write_export_file "$AXON_RUNTIME_STATE_FILE" \
  AXON_RUNTIME_MODE "$RUNTIME_MODE" \
  AXON_RUNTIME_SHADOW_ROLE "$RUNTIME_SHADOW_ROLE" \
  AXON_SPLIT_SHADOW_ONLY "$RUNTIME_SHADOW_ONLY" \
  AXON_RUNTIME_REACTIVATION_PATH "$RUNTIME_REACTIVATION_PATH" \
  AXON_DASHBOARD_ENABLED "$START_DASHBOARD" \
  AXON_INSTANCE_KIND "$AXON_INSTANCE_KIND" \
  AXON_RUNTIME_IDENTITY "$AXON_RUNTIME_IDENTITY" \
  AXON_RESOURCE_PRIORITY "$AXON_RESOURCE_PRIORITY" \
  AXON_BACKGROUND_BUDGET_CLASS "$AXON_BACKGROUND_BUDGET_CLASS" \
  AXON_GPU_ACCESS_POLICY "$AXON_GPU_ACCESS_POLICY" \
  AXON_WATCHER_POLICY "$AXON_WATCHER_POLICY" \
  MAX_AXON_WORKERS "${MAX_AXON_WORKERS:-}" \
  AXON_QUEUE_MEMORY_BUDGET_BYTES "${AXON_QUEUE_MEMORY_BUDGET_BYTES:-}" \
  AXON_WATCHER_SUBTREE_HINT_BUDGET "${AXON_WATCHER_SUBTREE_HINT_BUDGET:-}" \
  AXON_SPLIT_BRAIN_IST_READER_ONLY "${AXON_SPLIT_BRAIN_IST_READER_ONLY:-}" \
  AXON_DUCKDB_MEMORY_LIMIT_GB "${AXON_DUCKDB_MEMORY_LIMIT_GB:-}" \
  AXON_EMBEDDING_PROVIDER "${AXON_EMBEDDING_PROVIDER:-}" \
  AXON_RELEASE_VERSION "$AXON_RELEASE_VERSION" \
  AXON_BUILD_ID "$AXON_BUILD_ID" \
  AXON_PACKAGE_VERSION "$AXON_PACKAGE_VERSION" \
  AXON_INSTALL_GENERATION "$AXON_INSTALL_GENERATION" \
  AXON_PUBLIC_HOST "${AXON_PUBLIC_HOST:-}" \
  AXON_PUBLIC_HOST_SOURCE "${AXON_PUBLIC_HOST_SOURCE:-unresolved}" \
  AXON_PUBLIC_ENDPOINTS_AVAILABLE "${AXON_PUBLIC_ENDPOINTS_AVAILABLE:-0}" \
  AXON_MCP_PUBLIC_URL "${AXON_MCP_PUBLIC_URL:-}" \
  AXON_SQL_PUBLIC_URL "${AXON_SQL_PUBLIC_URL:-}" \
  AXON_DASHBOARD_PUBLIC_URL "${AXON_DASHBOARD_PUBLIC_URL:-}"

# Never discard DuckDB WAL during a normal restart. WAL replay is required to recover
# recent committed work when the main database file has not been checkpointed yet.
if [[ "${AXON_DROP_WAL_ON_START:-0}" == "1" ]]; then
  echo "⚠️ AXON_DROP_WAL_ON_START=1 set: deleting DuckDB WAL files before start."
  rm -f "$AXON_DB_ROOT/"*.wal 2>/dev/null || true
fi

# Create TMUX session
if ! tmux has-session -t "$TMUX_SESSION" 2>/dev/null; then
    tmux new-session -d -s "$TMUX_SESSION" -n "core"
fi

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
    AXON_RESOURCE_PRIORITY \
    AXON_BACKGROUND_BUDGET_CLASS \
    AXON_GPU_ACCESS_POLICY \
    AXON_WATCHER_POLICY \
    AXON_ENABLE_FEDERATION_ORCHESTRATOR \
    AXON_QUEUE_MEMORY_BUDGET_BYTES \
    AXON_WATCHER_SUBTREE_HINT_BUDGET \
    AXON_SPLIT_BRAIN_IST_READER_ONLY \
    AXON_DUCKDB_MEMORY_LIMIT_GB \
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
    AXON_GPU_EMBED_SERVICE_ENABLED \
    AXON_GPU_EMBED_SERVICE_RECYCLE_EVERY_BATCH \
    AXON_GPU_EMBED_SERVICE_TENSORRT \
    AXON_ORT_ARTIFACT_MANIFEST \
    AXON_RELEASE_VERSION \
    AXON_BUILD_ID \
    AXON_PACKAGE_VERSION \
    AXON_INSTALL_GENERATION \
    AXON_PUBLIC_HOST \
    AXON_PUBLIC_HOST_SOURCE \
    AXON_PUBLIC_ENDPOINTS_AVAILABLE \
    AXON_MCP_PUBLIC_URL \
    AXON_SQL_PUBLIC_URL \
    AXON_DASHBOARD_PUBLIC_URL \
    OMP_NUM_THREADS \
    OMP_WAIT_POLICY
do
    append_pass_through_export "$pass_through_var"
done
PROFILE_EXPORT=""
if [[ "$RUNTIME_MODE" == "indexer_full" ]]; then
    PROFILE_EXPORT="export AXON_ENABLE_AUTONOMOUS_INGESTOR=true; export AXON_RUNTIME_PROFILE=full_autonomous; "
fi
PRELAUNCH_LD_LIBRARY_PATH_EXPORT=""
CUDA_PACKAGE_SET="${AXON_CUDA_PACKAGE_SET:-cudaPackages}"
axon_resolve_ort_runtime "$PROJECT_ROOT" "$EMBEDDING_PROVIDER_REQUEST" || exit 1
if ! has_live_runtime_dataplane; then
    tmux send-keys -t "$TMUX_SESSION:core" "devenv shell --no-reload --no-tui -- bash -lc 'mkdir -p \"$AXON_RUN_ROOT\"; export AXON_PROJECTS_ROOT=\"$PROJECTS_ROOT\"; export AXON_WATCH_DIR=\"$WATCH_ROOT\"; export AXON_PROJECT_ROOT=\"$PROJECT_ROOT\"; export AXON_RUNTIME_MODE=\"$RUNTIME_MODE\"; export AXON_RUNTIME_SHADOW_ROLE=\"$RUNTIME_SHADOW_ROLE\"; export AXON_SPLIT_SHADOW_ONLY=\"$RUNTIME_SHADOW_ONLY\"; export AXON_MCP_MUTATION_JOBS=1; export AXON_INSTANCE_KIND=\"$AXON_INSTANCE_KIND\"; export AXON_RUNTIME_IDENTITY=\"$AXON_RUNTIME_IDENTITY\"; export AXON_DB_ROOT=\"$AXON_DB_ROOT\"; export AXON_RUN_ROOT=\"$AXON_RUN_ROOT\"; export AXON_PID_FILE=\"$AXON_PID_FILE\"; export AXON_TELEMETRY_SOCK=\"$AXON_TELEMETRY_SOCK\"; export AXON_MCP_SOCK=\"$AXON_MCP_SOCK\"; export PHX_PORT=\"$PHX_PORT\"; export HYDRA_TCP_PORT=\"$HYDRA_TCP_PORT\"; export HYDRA_HTTP_PORT=\"$HYDRA_HTTP_PORT\"; export HYDRA_ODATA_PORT=\"$HYDRA_ODATA_PORT\"; export HYDRA_HTTP2_PORT=\"$HYDRA_HTTP2_PORT\"; export HYDRA_MCP_PORT=\"$HYDRA_MCP_PORT\"; export AXON_SQL_URL=\"$AXON_SQL_URL\"; export AXON_MCP_URL=\"$AXON_MCP_URL\"; export AXON_DASHBOARD_URL=\"$AXON_DASHBOARD_URL\"; export AXON_MUTATION_POLICY=\"$AXON_MUTATION_POLICY\"; ${PROFILE_EXPORT}${WORKER_CAP_EXPORT}${EMBEDDING_PROVIDER_EXPORT}${PASS_THROUGH_EXPORTS}${PRELAUNCH_LD_LIBRARY_PATH_EXPORT}export ORT_STRATEGY=system; export ORT_DYLIB_PATH=\"$ORT_DYLIB_PATH\"; echo \"🚀 Starting $RUNTIME_EXECUTABLE_NAME...\"; \"$RUNTIME_EXECUTABLE\" & core_pid=\$!; echo \$core_pid > \"$AXON_PID_FILE\"; wait \$core_pid; core_status=\$?; rm -f \"$AXON_PID_FILE\"; exit \$core_status'" C-m
fi

if [ "$START_DASHBOARD" = "1" ] && ! has_live_dashboard_dataplane; then
    launch_dashboard_window
fi

echo "⏳ Waiting for Axon Infrastructure to rise (Timeout: ${STARTUP_TIMEOUT_S}s)..."

# Parallel wait loop for both services
CORE_READY=false
DASHBOARD_READY=false

# Wait up to STARTUP_TIMEOUT_S * 1s
for ((i=1; i<=STARTUP_TIMEOUT_S; i++)); do
    if [ "$CORE_READY" = false ]; then
        if verify_role_ready; then
            if axon_role_is_indexer "$RUNTIME_SHADOW_ROLE"; then
                echo "✅ Axon Indexer runtime is Ready."
            else
                echo "✅ Axon Data Plane and MCP Gateway are Ready."
            fi
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
    echo "   Inspect TMUX with: tmux attach -t $TMUX_SESSION"
    exit 1
fi

if [ "$CORE_READY" = true ] && ! axon_role_is_indexer "$RUNTIME_SHADOW_ROLE"; then
    echo ""
    echo "🧪 Verifying live SQL schema..."
    if ! verify_sql_gateway; then
        if axon_role_is_brain "$RUNTIME_SHADOW_ROLE" \
            && [[ "${AXON_SPLIT_BRAIN_IST_READER_ONLY:-0}" =~ ^(1|true|yes|on)$ ]]; then
            echo "⚠️ Brain started before a materialized IST reader replica was available."
            echo "   Continuing in degraded read mode until indexer publishes ist-reader.db."
        else
            echo "❌ Axon Core exposed its port but failed the live schema check."
            echo "   Inspect TMUX with: tmux attach -t $TMUX_SESSION"
            exit 1
        fi
    fi
    if verify_sql_gateway >/dev/null 2>&1; then
        echo "✅ Live SQL schema check succeeded."
    fi
fi

if ! axon_role_is_indexer "$RUNTIME_SHADOW_ROLE"; then
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
fi

if [ "$RUN_MCP_TESTS" = "1" ] && ! axon_role_is_indexer "$RUNTIME_SHADOW_ROLE"; then
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
    if run_devenv_shell "./scripts/axon --instance $AXON_INSTANCE_KIND quality-mcp"; then
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
if axon_role_is_indexer "$RUNTIME_SHADOW_ROLE"; then
    echo "Telemetry socket: $AXON_TELEMETRY_SOCK"
    echo "IST writer: $AXON_DB_ROOT/ist.db"
    echo "IST reader replica: $AXON_DB_ROOT/ist-reader.db"
elif [[ "${AXON_PUBLIC_ENDPOINTS_AVAILABLE:-0}" == "1" ]]; then
    if [ "$START_DASHBOARD" = "1" ]; then
        echo "Dashboard: ${AXON_DASHBOARD_PUBLIC_URL}cockpit"
    fi
    echo "SQL Gateway: $AXON_SQL_PUBLIC_URL"
    echo "MCP Server: $AXON_MCP_PUBLIC_URL"
else
    if [ "$START_DASHBOARD" = "1" ]; then
        echo "Dashboard (host-local): ${AXON_DASHBOARD_URL}cockpit"
    fi
    echo "SQL Gateway (host-local): $AXON_SQL_URL"
    echo "MCP Server (host-local): $AXON_MCP_URL"
    echo "Advertised endpoints unresolved. Set AXON_PUBLIC_HOST for isolated clients."
fi
echo "Stop services with: ./scripts/axon --instance $AXON_INSTANCE_KIND stop"
echo ""
