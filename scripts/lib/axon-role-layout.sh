#!/usr/bin/env bash

# REQ-AXO-178 — detect the active runtime role of a given instance by inspecting
# pid files under .axon{,-dev}/run-{brain,indexer}/. Returns "brain" or
# "indexer" on the first alive PID found; non-zero exit when none alive.
# Env-var override (AXON_RUNTIME_SHADOW_ROLE / AXON_RUNTIME_BOOT_ROLE) is
# honoured by callers that prefer it over auto-detection.
axon_detect_role_from_pid_files() {
    local project_root="${1:?project root required}"
    local instance_kind="${2:-live}"
    local run_root_base
    if [[ "$instance_kind" == "dev" ]]; then
        run_root_base="$project_root/.axon-dev"
    else
        run_root_base="$project_root/.axon"
    fi

    _axon_pid_alive() {
        local pid_file="$1"
        [[ -f "$pid_file" ]] || return 1
        local pid
        pid="$(tr -d '[:space:]' < "$pid_file" 2>/dev/null)"
        [[ -n "$pid" ]] || return 1
        kill -0 "$pid" 2>/dev/null
    }

    if _axon_pid_alive "$run_root_base/run-brain/axon-brain.pid"; then
        printf 'brain\n'
        return 0
    fi
    if _axon_pid_alive "$run_root_base/run-indexer/axon-indexer.pid"; then
        printf 'indexer\n'
        return 0
    fi
    return 1
}

axon_runtime_shadow_role() {
    local role="${AXON_RUNTIME_SHADOW_ROLE:-${AXON_RUNTIME_BOOT_ROLE:-}}"
    local mode="${AXON_RUNTIME_MODE:-}"
    role="${role#"${role%%[![:space:]]*}"}"
    role="${role%"${role##*[![:space:]]}"}"
    mode="${mode#"${mode%%[![:space:]]*}"}"
    mode="${mode%"${mode##*[![:space:]]}"}"

    if [[ -z "$role" ]]; then
        case "$mode" in
            brain_only|brain-only)
                printf 'brain\n'
                ;;
            indexer_graph|indexer-graph|indexer_vector|indexer-vector|indexer_full|indexer-full)
                printf 'indexer\n'
                ;;
            *)
                printf 'indexer\n'
                ;;
        esac
    else
        printf '%s\n' "$role"
    fi
}

axon_runtime_binary_name() {
    local role="${1:-$(axon_runtime_shadow_role)}"

    case "$role" in
        brain)
            printf 'axon-brain\n'
            ;;
        indexer)
            printf 'axon-indexer\n'
            ;;
        *)
            printf 'axon-core\n'
            ;;
    esac
}

axon_role_is_brain() {
    local role="${1:-$(axon_runtime_shadow_role)}"
    [[ "$role" == "brain" ]]
}

axon_role_is_indexer() {
    local role="${1:-$(axon_runtime_shadow_role)}"
    [[ "$role" == "indexer" ]]
}

axon_role_is_split() {
    local topology="${1:-$(axon_contract_topology)}"
    [[ "$topology" == "split" ]]
}

axon_contract_process_role() {
    local role="${1:-$(axon_runtime_shadow_role)}"
    if axon_role_is_brain "$role"; then
        printf 'brain\n'
    else
        printf 'indexer\n'
    fi
}

axon_contract_topology() {
    local role="${1:-$(axon_runtime_shadow_role)}"
    if axon_role_is_brain "$role"; then
        if [[ "${AXON_SPLIT_SHADOW_ONLY:-0}" == "1" ]]; then
            printf 'split\n'
        else
            printf 'brain_only\n'
        fi
    elif axon_role_is_indexer "$role"; then
        if [[ "${AXON_SPLIT_SHADOW_ONLY:-0}" == "1" ]]; then
            printf 'split\n'
        else
            printf 'indexer_only\n'
        fi
    else
        printf 'indexer_only\n'
    fi
}

axon_contract_public_mcp_authority() {
    printf 'brain\n'
}

axon_contract_soll_writer_authority() {
    printf 'brain\n'
}

axon_contract_ist_writer_authority() {
    printf 'indexer\n'
}

axon_apply_runtime_role_layout() {
    local project_root="${1:?project root required}"
    local role="${2:-$(axon_runtime_shadow_role)}"
    local runtime_executable_name="${3:-}"
    local role_name=""
    local run_root_base=""
    local pid_basename=""

    case "$role" in
        brain)
            role_name="brain"
            ;;
        indexer)
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
