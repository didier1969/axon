#!/usr/bin/env bash

axon_apply_runtime_role_layout() {
    local project_root="${1:?project root required}"
    local role="${2:-${AXON_RUNTIME_SHADOW_ROLE:-${AXON_RUNTIME_BOOT_ROLE:-legacy_monolith}}}"
    local runtime_executable_name="${3:-}"
    local role_name=""
    local run_root_base=""
    local pid_basename=""

    case "$role" in
        brain|brain_shadow)
            role_name="brain"
            ;;
        indexer|indexer_shadow)
            role_name="indexer"
            ;;
        *)
            return 0
            ;;
    esac

    if [[ "${AXON_INSTANCE_KIND:-live}" == "dev" ]]; then
        run_root_base="$project_root/.axon-dev"
        TMUX_SESSION="axon-dev-$role_name"
    else
        run_root_base="$project_root/.axon"
        TMUX_SESSION="axon-$role_name"
    fi

    if [[ -n "$runtime_executable_name" ]]; then
        pid_basename="$runtime_executable_name"
    else
        pid_basename="axon-$role_name"
    fi

    AXON_RUN_ROOT="$run_root_base/run-$role_name"
    AXON_PID_FILE="$AXON_RUN_ROOT/${pid_basename}.pid"
    AXON_RUNTIME_STATE_FILE="$AXON_RUN_ROOT/runtime.env"
    AXON_TELEMETRY_SOCK="/tmp/axon-${AXON_INSTANCE_KIND}-${role_name}-telemetry.sock"
    AXON_MCP_SOCK="/tmp/axon-${AXON_INSTANCE_KIND}-${role_name}-mcp.sock"
    AXON_RUNTIME_IDENTITY="${AXON_RUNTIME_IDENTITY}-${role_name}"

    export TMUX_SESSION AXON_RUN_ROOT AXON_PID_FILE AXON_RUNTIME_STATE_FILE
    export AXON_TELEMETRY_SOCK AXON_MCP_SOCK AXON_RUNTIME_IDENTITY
}
