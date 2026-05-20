#!/bin/bash
set -euo pipefail

# Axon v2 - Daily Start Script
# Canonical daily workflow entrypoint for running Axon in TMUX.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
DEFAULT_PROJECTS_ROOT="$(cd "$PROJECT_ROOT/.." && pwd)"
# shellcheck source=scripts/lib/axon-instance.sh
source "$PROJECT_ROOT/scripts/lib/axon-instance.sh"
# REQ-AXO-109 — clear AXON_*/HYDRA_* leaked from a previous run in
# this shell before any lib re-derives instance state.
axon_clear_inherited_env
# shellcheck source=scripts/lib/axon-role-layout.sh
source "$PROJECT_ROOT/scripts/lib/axon-role-layout.sh"
# shellcheck source=scripts/lib/axon-resource-policy.sh
source "$PROJECT_ROOT/scripts/lib/axon-resource-policy.sh"
# shellcheck source=scripts/lib/axon-ort-runtime.sh
source "$PROJECT_ROOT/scripts/lib/axon-ort-runtime.sh"
# shellcheck source=scripts/lib/axon-version.sh
source "$PROJECT_ROOT/scripts/lib/axon-version.sh"
# shellcheck source=scripts/lib/socket-lifecycle.sh
source "$PROJECT_ROOT/scripts/lib/socket-lifecycle.sh"
# shellcheck source=scripts/lib/ensure-runtime.sh
source "$PROJECT_ROOT/scripts/lib/ensure-runtime.sh"
cd "$PROJECT_ROOT"

axon_load_worktree_env "$PROJECT_ROOT"
axon_resolve_instance "$PROJECT_ROOT" "$(basename "$PROJECT_ROOT")"
axon_resolve_resource_policy "$AXON_INSTANCE_KIND"
axon_resolve_version "$PROJECT_ROOT"

LIVE_RELEASE_CURRENT_MANIFEST="$PROJECT_ROOT/.axon/live-release/current.json"
LIVE_RELEASE_PENDING_MANIFEST="$PROJECT_ROOT/.axon/live-release/pending.json"
LIVE_RELEASE_MANIFEST_SOURCE="${AXON_LIVE_RELEASE_MANIFEST:-$LIVE_RELEASE_CURRENT_MANIFEST}"
LIVE_RELEASE_ACTIVE=0
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
    # Fields 0/1 (combined artifact/build_info) are intentionally read
    # but discarded — REQ-AXO-083: the combined-artifact contract is
    # superseded by the per-role brain/indexer fields below. The python
    # block above still emits them so the shape stays positional.
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
# REQ-AXO-150 — runtime mode resolution priority: env override > last-known
# good (instance-state.json) > customer-facing default (brain_only). The
# previous default was `indexer_graph`, which left live MCP socket missing
# whenever a stale runtime triggered a fresh `start` without flags — bricking
# the customer-facing `axon init` workflow (observed 2026-05-03).
case "${AXON_INSTANCE_KIND:-live}" in
    live) AXON_INSTANCE_STATE_FILE="$PROJECT_ROOT/.axon/instance-state.json" ;;
    dev)  AXON_INSTANCE_STATE_FILE="$PROJECT_ROOT/.axon-dev/instance-state.json" ;;
    *)    AXON_INSTANCE_STATE_FILE="$PROJECT_ROOT/.axon/instance-state.json" ;;
esac

# Per-instance env-var overrides loaded from a sourced config file. Holds
# AXON_LIVE_DATABASE_URL / AXON_SOLL_SEED_PATH and any future runtime-flag
# persistence. Files are .gitignore'd because AXON_LIVE_DATABASE_URL contains
# credentials. Operator writes them once via the canonical wrapper or manually;
# start.sh propagates them to the brain process automatically on every start.
case "${AXON_INSTANCE_KIND:-live}" in
    live) AXON_RUNTIME_CONFIG_FILE="$PROJECT_ROOT/.axon/runtime-config.live.env" ;;
    dev)  AXON_RUNTIME_CONFIG_FILE="$PROJECT_ROOT/.axon-dev/runtime-config.dev.env" ;;
    *)    AXON_RUNTIME_CONFIG_FILE="" ;;
esac
if [[ -n "$AXON_RUNTIME_CONFIG_FILE" && -f "$AXON_RUNTIME_CONFIG_FILE" ]]; then
    # shellcheck disable=SC1090
    set -o allexport
    . "$AXON_RUNTIME_CONFIG_FILE"
    set +o allexport
    echo "🔧 Loaded runtime config from $AXON_RUNTIME_CONFIG_FILE"
fi

# Auto-bootstrap PG / role / DB before any binary check.
# Without this, a fresh WSL, a wiped .devenv/state, or a competing
# Docker container holding :44144 forces operator into a 5-step manual
# recovery. ensure_runtime_ready is idempotent — safe to call on every
# start. Runs inline (no devenv shell wrap) by resolving psql /
# pg_isready / devenv from /nix/store + PATH directly, saving the
# ~10-15s cost of a devenv shell entry on this machine.
if [[ "${AXON_SKIP_RUNTIME_BOOTSTRAP:-0}" != "1" ]]; then
    if ! ensure_runtime_ready "$AXON_INSTANCE_KIND"; then
        echo "❌ Runtime bootstrap (ensure_runtime_ready) failed; refusing to start." >&2
        exit 1
    fi
fi
AXON_LAST_RUNTIME_MODE=""
if [[ -f "$AXON_INSTANCE_STATE_FILE" ]]; then
    AXON_LAST_RUNTIME_MODE="$(python3 -c 'import json,sys
try:
    print(json.load(open(sys.argv[1])).get("last_mode",""))
except Exception:
    pass' "$AXON_INSTANCE_STATE_FILE" 2>/dev/null || true)"
fi
RUNTIME_MODE="${AXON_RUNTIME_MODE:-${AXON_LAST_RUNTIME_MODE:-brain_only}}"
# axon_runtime_shadow_role reads AXON_RUNTIME_MODE; export so the resolved
# mode (after env > state > default) is what the helper sees.
export AXON_RUNTIME_MODE="$RUNTIME_MODE"
RUNTIME_SHADOW_ROLE="$(axon_runtime_shadow_role)"
RUNTIME_SHADOW_ONLY="${AXON_SPLIT_SHADOW_ONLY:-0}"
RUNTIME_EXECUTABLE="bin/axon-core"
RUNTIME_EXECUTABLE_NAME="$(axon_runtime_binary_name "$RUNTIME_SHADOW_ROLE")"
SELECTED_DEBUG_RUNTIME_BIN=""
SELECTED_RELEASE_RUNTIME_BIN=""
START_DASHBOARD=1
RUN_MCP_TESTS=1
SKIP_ELIXIR_PREWARM="${AXON_SKIP_ELIXIR_PREWARM:-0}"
REQUEST_TENSORRT=0

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

# socket_responds backwards-compatible alias — real liveness probe lives
# in scripts/lib/socket-lifecycle.sh as axon_socket_responds (REQ-AXO-093).
socket_responds() {
    axon_socket_responds "$@"
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
        # REQ-AXO-093 — file-existence is not enough; orphan sockets must fail
        if socket_responds "$AXON_TELEMETRY_SOCK"; then
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
        --indexer-full|--indexerfull|--full)
            # REQ-AXO-100 — accept --full as an alias for --indexer-full so
            # docs/getting-started.md (which uses --full as the daily
            # shorthand) executes verbatim. Without the alias the canonical
            # doc command would fail with "Unknown option: --full".
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
        --fast)
            # Dev-iteration shorthand: brain serves MCP, nothing else.
            # Skips dashboard (Elixir/Phoenix not needed for MCP-only
            # work), Elixir Hex/Rebar prewarm, and the post-start MCP
            # quality gate. ~5× faster restart for pipeline / GPU work.
            START_DASHBOARD=0
            RUN_MCP_TESTS=0
            SKIP_ELIXIR_PREWARM=1
            ;;
        --tensorrt)
            REQUEST_TENSORRT=1
            ;;
        --help|-h)
            cat <<'EOF'
Usage: ./scripts/start.sh [--brain-only|--indexer-graph|--indexer-vector|--indexer-full] [--tensorrt] [--no-dashboard] [--skip-mcp-tests] [--skip-elixir-prewarm]

Modes:
  --brain-only      MCP + dashboard authority only, without graph or vector workers
  --indexer-graph   Indexer with graph ingestion only, without semantic/vector workers
  --indexer-vector  Indexer with semantic/vector workers only, without graph ingestion
  --indexer-full    Indexer with graph + semantic/vector workloads (alias: --full)

Options:
  --tensorrt      Enable the TensorRT GPU embedding service for indexer vector/full modes
  --no-dashboard   Disable Elixir LiveView dashboard
  --skip-mcp-tests Skip automatic MCP quality gate validation after startup
  --skip-elixir-prewarm Skip non-interactive `mix local.hex`/`mix local.rebar` bootstrap
  --fast          MCP-only dev iteration shorthand (=--no-dashboard --skip-mcp-tests --skip-elixir-prewarm)
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

if [[ "$REQUEST_TENSORRT" == "1" ]]; then
    if [[ "$RUNTIME_MODE" != "indexer_full" && "$RUNTIME_MODE" != "indexer_vector" ]]; then
        echo "❌ --tensorrt requires --indexer-vector or --indexer-full."
        echo "   The current mode is $RUNTIME_MODE, which does not run the vector lane."
        exit 1
    fi
    export AXON_EMBEDDING_PROVIDER="cuda"
    export AXON_GPU_EMBED_SERVICE_ENABLED="${AXON_GPU_EMBED_SERVICE_ENABLED:-1}"
    export AXON_GPU_EMBED_SERVICE_TENSORRT=1
    export AXON_GPU_EMBED_SERVICE_RECYCLE_EVERY_BATCH="${AXON_GPU_EMBED_SERVICE_RECYCLE_EVERY_BATCH:-0}"
    export AXON_GPU_TELEMETRY_BACKEND="${AXON_GPU_TELEMETRY_BACKEND:-nvml}"
    export AXON_NVML_LIBRARY_PATH="${AXON_NVML_LIBRARY_PATH:-/usr/lib/wsl/lib/libnvidia-ml.so.1}"
    export AXON_OPT_MAX_VRAM_USED_MB="${AXON_OPT_MAX_VRAM_USED_MB:-2048}"
    export AXON_CUDA_MEMORY_SOFT_LIMIT_MB="${AXON_CUDA_MEMORY_SOFT_LIMIT_MB:-$AXON_OPT_MAX_VRAM_USED_MB}"
    export AXON_CUDA_MEMORY_LIMIT_MB="${AXON_CUDA_MEMORY_LIMIT_MB:-1024}"
    export AXON_GPU_PRIMARY_WORKER_MAX_USED_MB="${AXON_GPU_PRIMARY_WORKER_MAX_USED_MB:-1536}"
    export AXON_GPU_TELEMETRY_CACHE_TTL_MS="${AXON_GPU_TELEMETRY_CACHE_TTL_MS:-250}"
    export AXON_TENSORRT_OVERSHOOT_MB="${AXON_TENSORRT_OVERSHOOT_MB:-7900}"
    export AXON_QUALIFY_STOP_ON_VRAM_OVERSHOOT="${AXON_QUALIFY_STOP_ON_VRAM_OVERSHOOT:-1}"
fi

# REQ-AXO-102 — apply the brain-only resource defaults from start-brain.sh
# directly when `--brain-only` is selected via the unified entrypoint, so
# `./scripts/axon start --brain-only` is contractually equivalent to
# `bash scripts/lib/start-brain.sh`. Without this block, the unified path
# runs a brain with default GPU tunings that diverge from the wrapper's
# intent (no GPU avoidance). All defaults use `:-` so any explicit override
# (env var or wrapper) wins.
if [[ "$RUNTIME_MODE" == "brain_only" ]]; then
    export AXON_GPU_ACCESS_POLICY="${AXON_GPU_ACCESS_POLICY:-avoid}"
fi

# REQ-AXO-91563 slice 1 — cap glibc per-thread mmap arenas. Without this,
# 170+ threads (Tokio + ORT inference + watcher + ingester) trigger glibc
# to create up to 64 arenas × 64 MB each (~10 GB virtual, mostly resident
# under bursty allocation). Once allocated, glibc NEVER returns these
# arenas to the OS, so RSS climbs monotonically and never recovers in
# idle. Session 42 measurement (graph-only, T+9m) : 4.39 GB → 2.59 GB
# (-41 %), 111 → 5 × 64MB arenas (-95 %). Indexer_full equivalent saving
# is expected ~5-7 GB. Override only if profiling identifies arena
# contention as a bottleneck (multi-threaded malloc-heavy workloads —
# not the case here, ORT + Rust runtime allocate large chunks rarely).
export MALLOC_ARENA_MAX="${MALLOC_ARENA_MAX:-2}"

RUNTIME_REACTIVATION_PATH="default"
RUNTIME_EXECUTABLE_NAME="$(axon_runtime_binary_name "$RUNTIME_SHADOW_ROLE")"

CARGO_TARGET_ROOT="${CARGO_TARGET_DIR:-$PROJECT_ROOT/.axon/cargo-target}"
DEVENV_DEBUG_BIN_ROOT="$CARGO_TARGET_ROOT/debug"

case "$RUNTIME_EXECUTABLE_NAME" in
    axon-brain)
        if [[ "$AXON_INSTANCE_KIND" == "live" && -x "$PROJECT_ROOT/bin/axon-brain" ]]; then
            RUNTIME_EXECUTABLE="bin/axon-brain"
        else
            RUNTIME_EXECUTABLE="${DEVENV_DEBUG_BIN_ROOT#"$PROJECT_ROOT"/}/axon-brain"
        fi
        SELECTED_DEBUG_RUNTIME_BIN="$PROJECT_ROOT/$RUNTIME_EXECUTABLE"
        SELECTED_RELEASE_RUNTIME_BIN="$CARGO_TARGET_ROOT/release/axon-brain"
        ;;
    axon-indexer)
        if [[ "$AXON_INSTANCE_KIND" == "live" && -x "$PROJECT_ROOT/bin/axon-indexer" ]]; then
            RUNTIME_EXECUTABLE="bin/axon-indexer"
        else
            RUNTIME_EXECUTABLE="${DEVENV_DEBUG_BIN_ROOT#"$PROJECT_ROOT"/}/axon-indexer"
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
        # REQ-AXO-91570 : bumped 240 → 900 s. Defense-in-depth pour
        # absorber un cold-compile TensorRT BGE-Large (5-15 min typique
        # quand le hash d'engine cache change : profile bump, ORT/TRT
        # upgrade, model file change). Cache HIT = 30-60 s usual.
        # Override via `AXON_STARTUP_TIMEOUT_S`.
        STARTUP_TIMEOUT_S=900
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
        axon_log_warn "devenv shell failed; running 'devenv gc' once before retrying..."
        devenv gc >/dev/null 2>&1 || true
        attempt=$((attempt + 1))
        sleep 1
    done
}

# REQ-AXO-149 — verify nix-daemon BEFORE the first devenv shell call. Without
# the daemon (e.g. fresh WSL boot), `devenv shell` fails with a cryptic
# `cannot connect to socket /nix/var/nix/daemon-socket/socket` and the
# customer-facing `axon init` cold-start cannot complete. Previous code did
# this check ~70 lines later, after the first devenv invocation had already
# failed.
if ! nix store info >/dev/null 2>&1; then
    axon_log_warn "Nix daemon is not responding. Attempting to start it..."
    if command -v systemctl >/dev/null && systemctl is-system-running >/dev/null 2>&1; then
        sudo systemctl start nix-daemon || true
    else
        # WSL2 / non-systemd hosts — direct daemon launch.
        sudo bash -c "/nix/var/nix/profiles/default/bin/nix-daemon --daemon &" || true
        sleep 2
    fi
    if ! nix store info >/dev/null 2>&1; then
        echo "❌ nix-daemon is still not responding after auto-start attempt."
        echo "   Check that /nix/var/nix/profiles/default/bin/nix-daemon exists, then retry."
        echo "   On WSL2 you may need: sudo /nix/var/nix/profiles/default/bin/nix-daemon &"
        exit 1
    fi
fi

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
    axon_log_warn "Detected newer axon-core sources than $SELECTED_DEBUG_RUNTIME_BIN"
    echo "   Rebuilding selected runtime role binary..."
    # REQ-AXO-174: dev launches the debug binary at .axon/cargo-target/debug/,
    # but the rebuild used to always pass --release — producing a fresh
    # release binary while leaving the debug binary stale. Match the build
    # profile to the binary the script is about to launch.
    if [[ "$AXON_INSTANCE_KIND" == "live" ]]; then
        BUILD_PROFILE_FLAG="--release"
    else
        BUILD_PROFILE_FLAG=""
    fi
    if ! run_devenv_shell "cd '$PROJECT_ROOT/src/axon-core' && cargo build --bin $RUNTIME_EXECUTABLE_NAME $BUILD_PROFILE_FLAG"; then
        echo "❌ Failed to rebuild $RUNTIME_EXECUTABLE_NAME"
        exit 1
    fi
    if [[ "$AXON_INSTANCE_KIND" == "live" && -n "$SELECTED_RELEASE_RUNTIME_BIN" && -f "$SELECTED_RELEASE_RUNTIME_BIN" ]]; then
        echo "🔄 Updating live $RUNTIME_EXECUTABLE from rebuilt runtime role artifact..."
        mkdir -p "$(dirname "$RUNTIME_EXECUTABLE")"
        install -m 755 "$SELECTED_RELEASE_RUNTIME_BIN" "$RUNTIME_EXECUTABLE"
    fi
fi

if tmux has-session -t "$TMUX_SESSION" 2>/dev/null; then
    DELETED_EXE_PIDS=$(for pid in $(instance_runtime_pids); do
        exe=$(readlink -f "/proc/$pid/exe" 2>/dev/null || true)
        if [[ "$exe" == *"(deleted)"* ]]; then
            echo "$pid"
        fi
    done)

    if [ -n "${DELETED_EXE_PIDS:-}" ]; then
        axon_log_warn "Found Axon processes still running on deleted executables: $DELETED_EXE_PIDS"
        echo "   Resetting stale $RUNTIME_SHADOW_ROLE runtime state before restart..."
        tmux kill-session -t "$TMUX_SESSION" 2>/dev/null || true
        cleanup_stale_runtime_state
    fi

    if has_live_runtime_dataplane && { axon_role_is_indexer "$RUNTIME_SHADOW_ROLE" || verify_mcp_http; }; then
        if [[ "$START_DASHBOARD" == "1" ]] && ! has_live_dashboard_dataplane; then
            axon_log_warn "Axon core is healthy in TMUX session '$TMUX_SESSION' but dashboard is absent."
            echo "   Relaunching nexus window..."
            launch_dashboard_window
        else
            echo "ℹ️ Axon is already running in TMUX session '$TMUX_SESSION'."
            echo "   Attach with: tmux attach -t $TMUX_SESSION"
            exit 0
        fi
    else
        axon_log_warn "Found stale TMUX session '$TMUX_SESSION' without a healthy data plane. Resetting local runtime state..."
        tmux kill-session -t "$TMUX_SESSION" 2>/dev/null || true
        cleanup_stale_runtime_state
    fi
elif [[ -S "$AXON_TELEMETRY_SOCK" || -S "$AXON_MCP_SOCK" || -f "$AXON_PID_FILE" ]]; then
    axon_log_warn "Found stale local runtime state without a TMUX session. Cleaning sockets/pid and continuing..."
    cleanup_stale_runtime_state
fi

# nix-daemon was verified earlier (REQ-AXO-149) before the first devenv
# shell invocation. No re-check needed here.

# Synchronize binaries (handle 'Text file busy' via install)
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
    local probe_query="SELECT table_name FROM information_schema.tables WHERE table_schema NOT IN ('pg_catalog','information_schema','soll','axon_runtime','ag_catalog')"
    response="$(curl -sS -X POST http://127.0.0.1:$HYDRA_HTTP_PORT/sql \
        -H 'content-type: application/json' \
        -d "{\"query\":\"$probe_query\"}" 2>/dev/null || true)"

    if [[ -z "$response" ]]; then
        echo "❌ SQL Gateway did not answer the schema probe."
        return 1
    fi

    # REQ-AXO-901633 — canonical names are lowercase post-MIL-AXO-017
    # (Apache AGE retired, IST tables renamed). PostgreSQL identifiers
    # are case-folded unless quoted, so the legacy `File`/`Symbol`/
    # `RuntimeMetadata` here produced a 100 %-false-negative warning on
    # every boot. `file` capital is also being retired alongside
    # FileVectorizationQueue (REQ-AXO-901632) — `indexedfile` is the
    # canonical post-REQ-AXO-289 name and already present in every live
    # DB. Aligned here so the check survives the legacy `file` drop.
    for table in indexedfile symbol runtimemetadata; do
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
    axon_log_warn "Found a release build outside Devenv at $LEGACY_RELEASE_BIN"
    echo "   Axon starts from $DEVENV_RELEASE_BIN. Attempting automatic rebuild..."
    rebuild_core_release || exit 1
fi

if [ -f "$LEGACY_RELEASE_BIN" ] && [ "$LEGACY_RELEASE_BIN" -nt "$DEVENV_RELEASE_BIN" ]; then
    axon_log_warn "Detected a newer release binary outside Devenv:"
    echo "   $LEGACY_RELEASE_BIN"
    echo "   Attempting to refresh the authoritative Devenv build..."
    rebuild_core_release || exit 1
fi

if [ ! -f "$DEVENV_RELEASE_BIN" ]; then
    axon_log_warn "Missing Devenv release binary at $DEVENV_RELEASE_BIN"
    rebuild_core_release || exit 1
fi

if [ ! -f "$DEVENV_TUNNEL_BIN" ]; then
    axon_log_warn "Missing Devenv tunnel binary at $DEVENV_TUNNEL_BIN"
    rebuild_tunnel_release || exit 1
fi

if [ -f "$DEVENV_RELEASE_BIN" ] && find "$PROJECT_ROOT/src/axon-core/src" \
    "$PROJECT_ROOT/src/axon-core/Cargo.toml" \
    "$PROJECT_ROOT/src/axon-core/Cargo.lock" \
    -newer "$DEVENV_RELEASE_BIN" -print -quit | grep -q .; then
    axon_log_warn "Detected newer axon-core sources than $DEVENV_RELEASE_BIN"
    echo "   Rebuilding authoritative Devenv release..."
    rebuild_core_release || exit 1
fi

if [ -f "$DEVENV_TUNNEL_BIN" ] && find "$PROJECT_ROOT/src/axon-mcp-tunnel/src" \
    "$PROJECT_ROOT/src/axon-mcp-tunnel/Cargo.toml" \
    "$PROJECT_ROOT/src/axon-mcp-tunnel/Cargo.lock" \
    -newer "$DEVENV_TUNNEL_BIN" -print -quit | grep -q .; then
    axon_log_warn "Detected newer axon-mcp-tunnel sources than $DEVENV_TUNNEL_BIN"
    echo "   Rebuilding authoritative Devenv tunnel release..."
    rebuild_tunnel_release || exit 1
fi

if [[ "${AXON_SKIP_BIN_SYNC:-0}" != "1" ]]; then
    if [[ "$LIVE_RELEASE_ACTIVE" -eq 1 ]]; then
        echo "🔄 Updating live split binaries from promoted artifacts..."
        mkdir -p bin
        rm -f bin/axon-brain bin/axon-indexer
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
        mkdir -p bin && rm -f bin/axon-core && install -m 755 "$DEVENV_RELEASE_BIN" bin/axon-core
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
echo "🛠️ Per-stage workers: A=${AXON_A_WORKERS:-auto}/B=${AXON_B_WORKERS:-auto}"
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
  AXON_A_WORKERS "${AXON_A_WORKERS:-}" \
  AXON_B_WORKERS "${AXON_B_WORKERS:-}" \
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

# Create TMUX session
if ! tmux has-session -t "$TMUX_SESSION" 2>/dev/null; then
    tmux new-session -d -s "$TMUX_SESSION" -n "core"
fi

# Start Data Plane
# We use 'devenv shell' to ensure the runtime matches the pinned project toolchain.
# NEXUS v10.8: We force fastembed to use the system's libonnxruntime.so to prevent C++ aborts.
EMBEDDING_PROVIDER_EXPORT=""
if [[ -n "${EMBEDDING_PROVIDER_REQUEST:-}" ]]; then
    EMBEDDING_PROVIDER_EXPORT="export AXON_EMBEDDING_PROVIDER=\"$EMBEDDING_PROVIDER_REQUEST\"; "
fi
PASS_THROUGH_EXPORTS=""
# REQ-AXO-241 — single source of truth for env var lifecycle. Iterate
# the parent shell env, propagating any var that matches the prefix
# allowlist (AXON_*/HYDRA_* + a narrow set of OMP_* knobs) AND is NOT a
# derived per-instance var (denylist in scripts/lib/axon-env-vars.sh).
# Vars set inline by start.sh on the supervisor command line (AXON_DB_ROOT,
# AXON_PID_FILE, etc.) are on the denylist so they are not re-exported
# here on top of their canonical inline values.
#
# Adding a new tunable knob now requires zero changes here: if the
# operator exports `AXON_FOO=bar`, the prefix match propagates it to the
# supervised process. Only NEW per-instance derived vars need the
# denylist updated.
while IFS='=' read -r _pass_through_var _; do
    if axon_env_var_in_prefix_allowlist "$_pass_through_var" \
        && ! axon_env_var_is_derived "$_pass_through_var"; then
        _pass_through_value="${!_pass_through_var-}"
        if [[ -n "${_pass_through_value:-}" ]]; then
            printf -v _pass_through_escaped '%q' "$_pass_through_value"
            PASS_THROUGH_EXPORTS+="export ${_pass_through_var}=${_pass_through_escaped}; "
        fi
    fi
done < <(env)
unset _pass_through_var _pass_through_value _pass_through_escaped
PROFILE_EXPORT=""
if [[ "$RUNTIME_MODE" == "indexer_full" ]]; then
    PROFILE_EXPORT="export AXON_ENABLE_AUTONOMOUS_INGESTOR=true; export AXON_RUNTIME_PROFILE=full_autonomous; "
fi
PRELAUNCH_LD_LIBRARY_PATH_EXPORT=""
axon_resolve_ort_runtime "$PROJECT_ROOT" "$EMBEDDING_PROVIDER_REQUEST" || exit 1
if ! has_live_runtime_dataplane; then
    # Resolve axonctl binary for process supervision
    AXONCTL_BIN="$PROJECT_ROOT/bin/axonctl"
    if [[ ! -x "$AXONCTL_BIN" ]]; then
        AXONCTL_BIN="$PROJECT_ROOT/src/axon-core/target/release/axonctl"
    fi
    if [[ ! -x "$AXONCTL_BIN" ]]; then
        echo "❌ axonctl binary not found. Build it: cargo build --manifest-path src/axon-core/Cargo.toml --release --bin axonctl"
        exit 1
    fi

    tmux send-keys -t "$TMUX_SESSION:core" "devenv shell --no-reload --no-tui -- bash -lc 'mkdir -p \"$AXON_RUN_ROOT\"; export AXON_PROJECTS_ROOT=\"$PROJECTS_ROOT\"; export AXON_WATCH_DIR=\"$WATCH_ROOT\"; export AXON_PROJECT_ROOT=\"$PROJECT_ROOT\"; export AXON_RUNTIME_MODE=\"$RUNTIME_MODE\"; export AXON_RUNTIME_SHADOW_ROLE=\"$RUNTIME_SHADOW_ROLE\"; export AXON_SPLIT_SHADOW_ONLY=\"$RUNTIME_SHADOW_ONLY\"; export AXON_MCP_MUTATION_JOBS=1; export AXON_INSTANCE_KIND=\"$AXON_INSTANCE_KIND\"; export AXON_RUNTIME_IDENTITY=\"$AXON_RUNTIME_IDENTITY\"; export AXON_DB_ROOT=\"$AXON_DB_ROOT\"; export AXON_RUN_ROOT=\"$AXON_RUN_ROOT\"; export AXON_PID_FILE=\"$AXON_PID_FILE\"; export AXON_TELEMETRY_SOCK=\"$AXON_TELEMETRY_SOCK\"; export AXON_MCP_SOCK=\"$AXON_MCP_SOCK\"; export PHX_PORT=\"$PHX_PORT\"; export HYDRA_TCP_PORT=\"$HYDRA_TCP_PORT\"; export HYDRA_HTTP_PORT=\"$HYDRA_HTTP_PORT\"; export HYDRA_ODATA_PORT=\"$HYDRA_ODATA_PORT\"; export HYDRA_HTTP2_PORT=\"$HYDRA_HTTP2_PORT\"; export HYDRA_MCP_PORT=\"$HYDRA_MCP_PORT\"; export AXON_SQL_URL=\"$AXON_SQL_URL\"; export AXON_MCP_URL=\"$AXON_MCP_URL\"; export AXON_DASHBOARD_URL=\"$AXON_DASHBOARD_URL\"; export AXON_MUTATION_POLICY=\"$AXON_MUTATION_POLICY\"; ${PROFILE_EXPORT}${EMBEDDING_PROVIDER_EXPORT}${PASS_THROUGH_EXPORTS}${PRELAUNCH_LD_LIBRARY_PATH_EXPORT}export ORT_STRATEGY=system; export ORT_DYLIB_PATH=\"$ORT_DYLIB_PATH\"; echo \"🚀 Starting $RUNTIME_EXECUTABLE_NAME...\"; \"$AXONCTL_BIN\" supervise --project-root \"$PROJECT_ROOT\" --instance-kind \"$AXON_INSTANCE_KIND\" --role \"$RUNTIME_SHADOW_ROLE\" -- \"$RUNTIME_EXECUTABLE\"'" C-m
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

if [ "$CORE_READY" = false ]; then axon_log_warn "Timeout waiting for Axon Core."; fi
if [ "$START_DASHBOARD" = "1" ] && [ "$DASHBOARD_READY" = false ]; then axon_log_warn "Timeout waiting for Axon Dashboard."; fi

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
            axon_log_warn "Brain started before a materialized IST reader replica was available."
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
    # REQ-AXO-095 — verification was a partial elif chain that emitted
    # "MCP tunnel verification failed" but never exited, so the start
    # script continued to the final "Axon is rising" line on a runtime
    # that was actually broken. The contract is now: try the tunnel
    # first, fall back to the HTTP probe when the tunnel binary is
    # missing OR the tunnel verify fails, and exit only when BOTH
    # paths reject the runtime — that way the "rising" message can
    # only print on a runtime the verification actually accepted.
    _axon_mcp_verified=0
    if [ -x "bin/axon-mcp-tunnel" ]; then
        if echo '{"jsonrpc": "2.0", "method": "tools/list", "params": {}, "id": 1}' \
            | bin/axon-mcp-tunnel | grep -q "axon_query"; then
            echo "✅ MCP tunnel verification succeeded."
            _axon_mcp_verified=1
        else
            axon_log_warn "MCP tunnel verification failed; falling back to HTTP probe."
        fi
    fi
    if [ "$_axon_mcp_verified" = "0" ]; then
        if verify_mcp_http; then
            echo "✅ MCP HTTP verification succeeded."
            _axon_mcp_verified=1
        else
            echo "❌ MCP verification failed (tunnel and HTTP both unreachable)."
            echo "   Inspect the TMUX session ($TMUX_SESSION) to debug."
            exit 1
        fi
    fi
fi

if [ "$RUN_MCP_TESTS" = "1" ] && ! axon_role_is_indexer "$RUNTIME_SHADOW_ROLE"; then
    # Ingestion stabilization is only meaningful when an indexer is
    # actually running and writing the IST. Brain-only restarts have no
    # active writer, so the loop just burns 60s waiting for a quantity
    # that will not change. Restrict to non-brain-only modes (full-stack
    # scenarios where this start invocation also spawned an indexer).
    if [[ "$RUNTIME_MODE" != "brain_only" ]]; then
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
    fi

    echo "🧪 Running MCP Quality Gate Validation..."
    # The dispatcher's verb is `qualify-mcp` (`quality-mcp` does not
    # exist — every restart under the previous wording exited 1 with
    # "Unknown command" and reported "❌ MCP Quality Gate failed"
    # regardless of actual MCP health). Call the canonical wrapper
    # directly so renames in `scripts/axon` cannot break us again.
    if run_devenv_shell "bash '$PROJECT_ROOT/scripts/mcp_quality_gate.sh'"; then
        echo "✅ MCP Quality Gate passed."
    else
        echo "❌ MCP Quality Gate failed."
        exit 1
    fi
fi

# 6. Final Report
echo ""

# REQ-AXO-098 / DEC-AXO-062 — read the subsystem-tagged tristate
# readiness from the brain and gate the rising-message wording on the
# rolled-up overall. Brain-only paths read it directly; indexer paths
# skip (the indexer does not bind a public MCP and has no readiness
# surface to query). Best-effort: a failure to read readiness is
# logged via axon_log_warn and falls back to the legacy "rising"
# message so a transient probe failure does not block the script.
readiness_kind="unknown"
if ! axon_role_is_indexer "$RUNTIME_SHADOW_ROLE"; then
    readiness_payload=$(
        AXON_MCP_URL="$AXON_MCP_URL" python3 \
            "$PROJECT_ROOT/scripts/mcp_call.py" call status \
            --args '{"mode":"brief"}' --format data --timeout 5 2>/dev/null || true
    )
    if [[ -n "$readiness_payload" ]]; then
        readiness_kind=$(
            python3 - "$readiness_payload" <<'PY' 2>/dev/null || true
import json, sys
try:
    payload = json.loads(sys.argv[1])
    print((payload.get("readiness") or {}).get("kind", "unknown"))
except Exception:
    print("unknown")
PY
        )
    fi
fi
case "$readiness_kind" in
    ready)
        echo "🛡️ Axon is Ready in TMUX session '$TMUX_SESSION'."
        ;;
    degraded)
        axon_log_warn "Axon started DEGRADED; check 'mcp__axon__status data.readiness.reasons' for the failing subsystem(s) (TMUX session '$TMUX_SESSION')."
        ;;
    failed)
        axon_log_warn "Axon FAILED to reach a ready state; check 'mcp__axon__status data.readiness.reasons' for the failing subsystem(s) (TMUX session '$TMUX_SESSION')."
        ;;
    *)
        echo "🛡️ Axon is rising in TMUX session '$TMUX_SESSION'."
        ;;
esac

# REQ-AXO-150 — persist the runtime mode so the next plain `start` resumes
# the same role. Only persist when the runtime reached at least `degraded`
# (i.e. the process is alive); a `failed` start should not poison future
# defaults. Survives WSL reboots, brain crashes, and stale-pid recoveries.
if [[ "$readiness_kind" == "ready" || "$readiness_kind" == "degraded" ]]; then
    mkdir -p "$(dirname "$AXON_INSTANCE_STATE_FILE")"
    python3 - "$AXON_INSTANCE_STATE_FILE" "$RUNTIME_MODE" "$RUNTIME_SHADOW_ROLE" <<'PY' 2>/dev/null || true
import json, sys, time
state_path, mode, role = sys.argv[1], sys.argv[2], sys.argv[3]
state = {"last_mode": mode, "last_role": role, "last_started_at_ms": int(time.time()*1000)}
open(state_path, "w").write(json.dumps(state, indent=2))
PY
fi
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
