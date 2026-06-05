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
        AXON_WORKTREE_ENV_LOADED=1
    fi
}

# REQ-AXO-109 / REQ-AXO-241 — clear DERIVED AXON_* env vars
# inherited from a previous run in the same shell. Preserves operator-
# provided tuning knobs by default (allowlist-by-prefix); only the
# narrow denylist of derived per-instance vars is unset.
#
# Without this, a `dev` start followed by a `live` start in the same
# shell would leak dev's per-instance values (AXON_DB_ROOT, AXON_PID_FILE,
# AXON_RUN_ROOT, AXON_BRAIN_PORT, etc.) into the live runtime, breaking
# the Dual-Instance Operational Discipline Pillar (PIL-AXO-004). Operator
# tunables (AXON_VECTOR_WORKERS, AXON_DB_BACKEND, AXON_FOO_NEW, …) are
# preserved unchanged across runs.
#
# The single source of truth for the denylist is
# scripts/lib/axon-env-vars.sh; both this function and start.sh's
# PASS_THROUGH iterator consume it (REQ-AXO-241).
#
# Lifecycle scripts (start.sh, stop.sh, status.sh) MUST call this at
# entry, after sourcing this lib but before any other env mutation, so
# every lifecycle invocation starts from a deterministic env shape.
axon_clear_inherited_env() {
    local _axon_env_vars_lib="${BASH_SOURCE[0]%/*}/axon-env-vars.sh"
    if [[ -f "$_axon_env_vars_lib" && "${AXON_ENV_VARS_LOADED:-0}" != "1" ]]; then
        # shellcheck disable=SC1090
        source "$_axon_env_vars_lib"
        AXON_ENV_VARS_LOADED=1
    fi
    local name
    while IFS='=' read -r name _; do
        if axon_env_var_in_prefix_allowlist "$name" \
            && axon_env_var_is_derived "$name"; then
            unset "$name"
        fi
    done < <(env)
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
        export AXON_MCP_PUBLIC_URL="http://${public_host}:${AXON_BRAIN_PORT}/mcp"
        export AXON_SQL_PUBLIC_URL="http://${public_host}:${AXON_BRAIN_PORT}/sql"
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
        export ELIXIR_NODE_NAME="axon_dev_nexus"
        export PHX_PORT="44137"
        export AXON_BRAIN_PORT="44139"
        export AXON_DB_ROOT="$project_root/.axon-dev/graph_v2"
        export AXON_RUN_ROOT="$project_root/.axon-dev/run"
        export AXON_TELEMETRY_SOCK="/tmp/axon-dev-telemetry.sock"
        export AXON_MCP_SOCK="/tmp/axon-dev-mcp.sock"
        export AXON_MUTATION_POLICY="advisory_mutable"
    else
        export AXON_ENV="prod"
        export ELIXIR_NODE_NAME="axon_nexus"
        export PHX_PORT="44127"
        export AXON_BRAIN_PORT="44129"
        export AXON_DB_ROOT="$project_root/.axon/graph_v2"
        export AXON_RUN_ROOT="$project_root/.axon/live-run"
        export AXON_TELEMETRY_SOCK="/tmp/axon-live-telemetry.sock"
        export AXON_MCP_SOCK="/tmp/axon-live-mcp.sock"
        export AXON_MUTATION_POLICY="advisory_guarded"
    fi

    export AXON_PID_FILE="$AXON_RUN_ROOT/axon-core.pid"
    export AXON_RUNTIME_STATE_FILE="$AXON_RUN_ROOT/runtime.env"
    export AXON_DASHBOARD_URL="http://127.0.0.1:${PHX_PORT}/"
    export AXON_SQL_URL="http://127.0.0.1:${AXON_BRAIN_PORT}/sql"
    export AXON_MCP_URL="http://127.0.0.1:${AXON_BRAIN_PORT}/mcp"
    axon_resolve_public_endpoints || true
}
