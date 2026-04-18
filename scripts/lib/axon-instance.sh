#!/usr/bin/env bash

# Shared instance resolver for Axon lifecycle and qualification scripts.
# This layer keeps the current live runtime authoritative while making
# `live` and `dev` explicit and reusable across scripts.

axon_load_worktree_env() {
    local project_root="${1:?project root required}"
    local env_file="$project_root/.env.worktree"
    if [[ -f "$env_file" && "${AXON_WORKTREE_ENV_LOADED:-0}" != "1" ]]; then
        # shellcheck disable=SC1090
        source "$env_file"
        export AXON_WORKTREE_ENV_LOADED=1
    fi
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

axon_resolve_instance() {
    local project_root="${1:?project root required}"
    local repo_slug="${2:-$(basename "$project_root")}"
    local explicit_kind=""

    axon_load_worktree_env "$project_root"

    explicit_kind="$(axon_normalize_instance_kind "${AXON_INSTANCE_KIND:-${AXON_INSTANCE:-${AXON_ENV:-}}}" 2>/dev/null || true)"
    if [[ -z "$explicit_kind" ]]; then
        explicit_kind="live"
    fi

    export AXON_INSTANCE_KIND="$explicit_kind"
    export AXON_RUNTIME_IDENTITY="axon-${AXON_INSTANCE_KIND}-${repo_slug}"

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
}
