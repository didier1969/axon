#!/usr/bin/env bash

# Shared instance resolver for Axon lifecycle and qualification scripts.
# This layer keeps the current live runtime authoritative while making
# `live` and `dev` explicit and reusable across scripts.

# REQ-AXO-094 — Boot output hygiene: route ⚠️ warnings through a single
# helper so they (1) emit on stderr instead of polluting the
# informational stdout stream that operator dashboards / qualify scripts
# capture, and (2) carry a machine-parseable `[WARN][axon-start]` prefix
# alongside the human-readable ⚠️ marker. Lifecycle scripts (start.sh,
# stop.sh, qualify, lib helpers) MUST use this helper instead of raw
# `echo "⚠️ ..."` so future contract tightening (degraded-readiness
# escalation per REQ-AXO-098) has a single capture point.
axon_log_warn() {
    printf '[WARN][axon-start] ⚠️ %s\n' "$*" >&2
}

axon_load_worktree_env() {
    local project_root="${1:?project root required}"
    local env_file="$project_root/.env.worktree"
    if [[ -f "$env_file" && "${AXON_WORKTREE_ENV_LOADED:-0}" != "1" ]]; then
        # shellcheck disable=SC1090
        source "$env_file"
        export AXON_WORKTREE_ENV_LOADED=1
    fi
}

# REQ-AXO-109 — clear AXON_*/HYDRA_* env vars inherited from a previous
# run in the same shell, preserving an allowlist of vars that callers
# may legitimately set as user input. Without this, a `dev` start
# followed by a `live` start in the same shell leaks dev's tuning vars
# (AXON_VECTOR_WORKERS, AXON_GPU_EMBED_SERVICE_TENSORRT, AXON_DB_ROOT,
# etc.) into the live runtime and the live BEAM dashboard, breaking the
# Dual-Instance Operational Discipline Pillar (PIL-AXO-004).
#
# Lifecycle scripts (start.sh, stop.sh, status.sh) MUST call this at
# entry, after sourcing this lib but before any other env mutation, so
# every lifecycle invocation starts from a deterministic env shape.
axon_clear_inherited_env() {
    # Allowlist of vars that lifecycle scripts treat as user input — set
    # by the user, by wrappers (axon-live / axon-dev), by the dispatcher
    # (scripts/axon), or by callers like qualify / benchmark scripts.
    # Anything not in this set is treated as derived and unset.
    local -a preserve=(
        # Instance selection
        AXON_INSTANCE_KIND AXON_INSTANCE AXON_ENV
        # Project / scope
        AXON_PROJECT_ROOT AXON_PROJECT_CODE AXON_DEV_PROJECT_ROOT
        AXON_WATCH_DIR AXON_PROJECTS_ROOT AXON_REPO_SLUG
        # Role / mode selectors
        AXON_RUNTIME_SHADOW_ROLE AXON_RUNTIME_BOOT_ROLE
        AXON_RUNTIME_MODE AXON_SPLIT_SHADOW_ONLY
        AXON_DASHBOARD_ENABLED AXON_SPLIT_BRAIN_IST_READER_ONLY
        # GPU / embedding overrides
        AXON_GPU_BACKEND AXON_GPU_ACCESS_POLICY AXON_EMBEDDING_PROVIDER
        AXON_GRAPH_EMBEDDINGS_ENABLED AXON_GRAPH_WORKERS
        AXON_GPU_EMBED_SERVICE_ENABLED
        AXON_GPU_EMBED_SERVICE_RECYCLE_EVERY_BATCH
        AXON_GPU_RECYCLE_ON_VRAM_SUMMIT AXON_GPU_RECYCLE_IMMEDIATE_ON_VRAM_SUMMIT
        AXON_GPU_STUCK_RECOVERY_ENABLED
        AXON_GPU_PRIMARY_WORKER_MAX_USED_MB AXON_GPU_TELEMETRY_BACKEND
        AXON_GPU_TELEMETRY_CACHE_TTL_MS AXON_NVML_LIBRARY_PATH
        AXON_OPT_MAX_VRAM_USED_MB AXON_TENSORRT_OVERSHOOT_MB
        AXON_CUDA_MEMORY_SOFT_LIMIT_MB AXON_CUDA_MEMORY_LIMIT_MB
        AXON_QUALIFY_STOP_ON_VRAM_OVERSHOOT
        AXON_VECTOR_WORKERS AXON_CHUNK_BATCH_SIZE
        AXON_FILE_VECTORIZATION_BATCH_SIZE
        AXON_VECTOR_PIPELINE_INLINE
        AXON_HOT_STATUS_CACHE_ENABLED
        AXON_DIAG_SKIP_CHUNKEMBED
        AXON_DUCKDB_SYNC_MODE
        AXON_PARQUET_EMBEDDING_STORE_ENABLED
        AXON_PARQUET_CHUNK_CONTENT_ENABLED
        AXON_ASYNC_WRITER_ENABLED
        # Tuning / resource policy
        AXON_RESOURCE_PRIORITY AXON_BACKGROUND_BUDGET_CLASS
        AXON_WATCHER_POLICY AXON_QUEUE_MEMORY_BUDGET_BYTES
        AXON_WATCHER_SUBTREE_HINT_BUDGET MAX_AXON_WORKERS
        AXON_DUCKDB_MEMORY_LIMIT_GB
        # MIL-AXO-015: PostgreSQL backend selection + bulk_writer
        AXON_DB_BACKEND AXON_LIVE_DATABASE_URL AXON_DEV_DATABASE_URL
        AXON_INDEXER_PG_OPT_IN AXON_BULK_WRITER_ENABLED
        AXON_AGE_DUAL_WRITE AXON_AGE_READ AXON_AGE_ONLY_RELATIONS
        AXON_SOLL_SEED_PATH
        # GPU embed service / TensorRT EP selection
        AXON_GPU_EMBED_SERVICE_TENSORRT
        # Vector lane tuning (REQ-AXO-221 + REQ-AXO-238 bench cells)
        AXON_VECTOR_PERSIST_QUEUE_BOUND
        AXON_VECTOR_PREPARE_QUEUE_BOUND
        AXON_VECTOR_PREPARE_PIPELINE_DEPTH
        AXON_VECTOR_PREPARE_WORKERS_PER_VECTOR
        AXON_VECTOR_READY_QUEUE_DEPTH
        AXON_VECTOR_MAX_INFLIGHT_PERSISTS
        AXON_EMBED_MICRO_BATCH_MAX_ITEMS
        AXON_EMBED_MICRO_BATCH_MAX_TOTAL_TOKENS
        AXON_MAX_EMBED_BATCH_BYTES
        AXON_CUDA_ALLOW_TF32
        # Release / promotion
        AXON_LIVE_RELEASE_MANIFEST
        # Stop / cleanup flags
        AXON_DROP_WAL_ON_STOP AXON_NO_AUTO_VECTORS
        # Networking / public exposure
        AXON_PUBLIC_HOST AXON_ADVERTISED_HOST
        # Benchmark
        AXON_BENCHMARK_ACTIVE AXON_BENCHMARK_GPU_BACKEND
        # Preflight
        AXON_SKIP_ELIXIR_PREWARM AXON_STARTUP_TIMEOUT_S
        # User-overridable HYDRA ports
        HYDRA_GRPC_PORT
    )
    declare -A _axon_preserve_set
    local v
    for v in "${preserve[@]}"; do
        _axon_preserve_set[$v]=1
    done
    local name
    while IFS='=' read -r name _; do
        case "$name" in
            AXON_*|HYDRA_*)
                if [[ -z "${_axon_preserve_set[$name]:-}" ]]; then
                    unset "$name"
                fi
                ;;
        esac
    done < <(env)
    unset _axon_preserve_set
}

axon_normalize_instance_kind() {
    local raw="${1:-}"
    case "$raw" in
        "" )
            return 1
            ;;
        live|LIVE|prod|PROD|production|PRODUCTION)
            printf 'live\n'
            ;;
        dev|DEV|development|DEVELOPMENT)
            printf 'dev\n'
            ;;
        *)
            return 1
            ;;
    esac
}

axon_is_loopback_host() {
    local host="${1:-}"
    case "$host" in
        ""|localhost|127.0.0.1|::1)
            return 0
            ;;
        *)
            return 1
            ;;
    esac
}

axon_detect_public_host() {
    local candidate=""

    for candidate in \
        "${AXON_PUBLIC_HOST:-}" \
        "${AXON_ADVERTISED_HOST:-}" \
        "${WSL_IP:-}"
    do
        if [[ -n "$candidate" ]] && ! axon_is_loopback_host "$candidate"; then
            printf '%s\n' "$candidate"
            return 0
        fi
    done

    if command -v ip >/dev/null 2>&1; then
        candidate="$(
            ip route get 1.1.1.1 2>/dev/null | awk '{
                for (i = 1; i <= NF; i++) {
                    if ($i == "src" && (i + 1) <= NF) {
                        print $(i + 1)
                        exit
                    }
                }
            }' || true
        )"
        if [[ -n "$candidate" ]] && ! axon_is_loopback_host "$candidate"; then
            printf '%s\n' "$candidate"
            return 0
        fi

        candidate="$(
            ip addr show eth0 2>/dev/null | awk '/inet / { split($2, addr, "/"); print addr[1]; exit }' || true
        )"
        if [[ -n "$candidate" ]] && ! axon_is_loopback_host "$candidate"; then
            printf '%s\n' "$candidate"
            return 0
        fi
    fi

    return 1
}

axon_resolve_public_endpoints() {
    local public_host=""
    local public_host_source="unresolved"

    if [[ -n "${AXON_PUBLIC_HOST:-}" ]] && ! axon_is_loopback_host "${AXON_PUBLIC_HOST}"; then
        public_host="${AXON_PUBLIC_HOST}"
        public_host_source="explicit"
    elif public_host="$(axon_detect_public_host 2>/dev/null || true)"; then
        if [[ -n "$public_host" ]]; then
            public_host_source="derived"
        fi
    fi

    if [[ -n "$public_host" ]] && ! axon_is_loopback_host "$public_host"; then
        export AXON_PUBLIC_HOST="$public_host"
        export AXON_PUBLIC_HOST_SOURCE="$public_host_source"
        export AXON_PUBLIC_ENDPOINTS_AVAILABLE="1"
        export AXON_MCP_PUBLIC_URL="http://${public_host}:${HYDRA_HTTP_PORT}/mcp"
        export AXON_SQL_PUBLIC_URL="http://${public_host}:${HYDRA_HTTP_PORT}/sql"
        export AXON_DASHBOARD_PUBLIC_URL="http://${public_host}:${PHX_PORT}/"
        return 0
    fi

    export AXON_PUBLIC_HOST=""
    export AXON_PUBLIC_HOST_SOURCE="unresolved"
    export AXON_PUBLIC_ENDPOINTS_AVAILABLE="0"
    export AXON_MCP_PUBLIC_URL=""
    export AXON_SQL_PUBLIC_URL=""
    export AXON_DASHBOARD_PUBLIC_URL=""
    return 1
}

axon_resolve_instance() {
    local project_root="${1:?project root required}"
    local repo_name="${2:-$(basename "$project_root")}"
    local explicit_kind=""

    axon_load_worktree_env "$project_root"

    explicit_kind="$(axon_normalize_instance_kind "${AXON_INSTANCE_KIND:-${AXON_INSTANCE:-${AXON_ENV:-}}}" 2>/dev/null || true)"
    if [[ -z "$explicit_kind" ]]; then
        explicit_kind="live"
    fi

    export AXON_INSTANCE_KIND="$explicit_kind"
    export AXON_RUNTIME_IDENTITY="axon-${AXON_INSTANCE_KIND}-${repo_name}"

    if [[ "$AXON_INSTANCE_KIND" == "dev" ]]; then
        export AXON_ENV="dev"
        export TMUX_SESSION="axon-dev"
        export ELIXIR_NODE_NAME="axon_dev_nexus"
        export PHX_PORT="44137"
        export HYDRA_TCP_PORT="44138"
        export HYDRA_HTTP_PORT="44139"
        export HYDRA_ODATA_PORT="44140"
        export HYDRA_HTTP2_PORT="44141"
        export HYDRA_MCP_PORT="44142"
        export AXON_DB_ROOT="$project_root/.axon-dev/graph_v2"
        export AXON_RUN_ROOT="$project_root/.axon-dev/run"
        export AXON_TELEMETRY_SOCK="/tmp/axon-dev-telemetry.sock"
        export AXON_MCP_SOCK="/tmp/axon-dev-mcp.sock"
        export AXON_MUTATION_POLICY="advisory_mutable"
    else
        export AXON_ENV="prod"
        export TMUX_SESSION="axon"
        export ELIXIR_NODE_NAME="axon_nexus"
        export PHX_PORT="44127"
        export HYDRA_TCP_PORT="44128"
        export HYDRA_HTTP_PORT="44129"
        export HYDRA_ODATA_PORT="44130"
        export HYDRA_HTTP2_PORT="44131"
        export HYDRA_MCP_PORT="44132"
        export AXON_DB_ROOT="$project_root/.axon/graph_v2"
        export AXON_RUN_ROOT="$project_root/.axon/live-run"
        export AXON_TELEMETRY_SOCK="/tmp/axon-live-telemetry.sock"
        export AXON_MCP_SOCK="/tmp/axon-live-mcp.sock"
        export AXON_MUTATION_POLICY="advisory_guarded"
    fi

    export AXON_PID_FILE="$AXON_RUN_ROOT/axon-core.pid"
    export AXON_RUNTIME_STATE_FILE="$AXON_RUN_ROOT/runtime.env"
    export AXON_DASHBOARD_URL="http://127.0.0.1:${PHX_PORT}/"
    export AXON_SQL_URL="http://127.0.0.1:${HYDRA_HTTP_PORT}/sql"
    export AXON_MCP_URL="http://127.0.0.1:${HYDRA_HTTP_PORT}/mcp"
    axon_resolve_public_endpoints || true
}
