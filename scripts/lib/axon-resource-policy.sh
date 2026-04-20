#!/usr/bin/env bash

# Shared resource-policy resolver for Axon live/dev dual-instance operation.
# Policy decisions live here, then project onto existing runtime knobs.

axon_detect_host_cpu_cores() {
    if command -v nproc >/dev/null 2>&1; then
        nproc
        return 0
    fi

    getconf _NPROCESSORS_ONLN 2>/dev/null || printf '4\n'
}

axon_detect_host_ram_gb() {
    local kb=""
    kb="$(sed -n 's/^MemTotal:[[:space:]]*\([0-9][0-9]*\)[[:space:]]*kB$/\1/p' /proc/meminfo 2>/dev/null | head -n1)"
    if [[ -n "$kb" ]]; then
        printf '%s\n' "$(( kb / 1024 / 1024 ))"
        return 0
    fi

    printf '16\n'
}

axon_normalize_resource_priority() {
    case "${1:-}" in
        critical|CRITICAL) printf 'critical\n' ;;
        best_effort|BEST_EFFORT|besteffort|BESTEFFORT) printf 'best_effort\n' ;;
        *) return 1 ;;
    esac
}

axon_normalize_background_budget_class() {
    case "${1:-}" in
        conservative|CONSERVATIVE) printf 'conservative\n' ;;
        balanced|BALANCED) printf 'balanced\n' ;;
        aggressive|AGGRESSIVE) printf 'aggressive\n' ;;
        *) return 1 ;;
    esac
}

axon_normalize_gpu_access_policy() {
    case "${1:-}" in
        preferred|PREFERRED) printf 'preferred\n' ;;
        shared|SHARED) printf 'shared\n' ;;
        avoid|AVOID) printf 'avoid\n' ;;
        *) return 1 ;;
    esac
}

axon_normalize_watcher_policy() {
    case "${1:-}" in
        full|FULL) printf 'full\n' ;;
        bounded|BOUNDED) printf 'bounded\n' ;;
        off|OFF) printf 'off\n' ;;
        *) return 1 ;;
    esac
}

axon_default_resource_priority() {
    case "${1:?instance kind required}" in
        live) printf 'critical\n' ;;
        *) printf 'best_effort\n' ;;
    esac
}

axon_default_background_budget_class() {
    case "${1:?instance kind required}" in
        live) printf 'balanced\n' ;;
        *) printf 'conservative\n' ;;
    esac
}

axon_default_gpu_access_policy() {
    case "${1:?instance kind required}" in
        live) printf 'preferred\n' ;;
        *) printf 'avoid\n' ;;
    esac
}

axon_default_watcher_policy() {
    case "${1:?instance kind required}" in
        live) printf 'full\n' ;;
        *) printf 'bounded\n' ;;
    esac
}

axon_compute_worker_cap() {
    local instance_kind="${1:?instance kind required}"
    local budget_class="${2:?budget class required}"
    local cpu_cores="${3:?cpu cores required}"
    local cap=1

    case "$budget_class" in
        aggressive)
            cap="$cpu_cores"
            ;;
        balanced)
            cap=$(( cpu_cores - 1 ))
            ;;
        conservative)
            cap=$(( cpu_cores / 3 ))
            ;;
    esac

    if [[ "$instance_kind" == "dev" && "$cap" -gt 6 ]]; then
        cap=6
    fi

    if [[ "$instance_kind" == "live" && "$cap" -gt 12 ]]; then
        cap=12
    fi

    if [[ "$instance_kind" == "live" && "$cap" -lt 2 ]]; then
        cap=2
    elif [[ "$instance_kind" != "live" && "$cap" -lt 1 ]]; then
        cap=1
    fi

    printf '%s\n' "$cap"
}

axon_compute_queue_memory_budget_bytes() {
    local budget_class="${1:?budget class required}"
    local ram_gb="${2:?ram_gb required}"
    local budget_gb=1

    case "$budget_class" in
        aggressive)
            budget_gb=$(( ram_gb / 3 ))
            ;;
        balanced)
            budget_gb=$(( ram_gb / 4 ))
            ;;
        conservative)
            budget_gb=$(( ram_gb / 8 ))
            ;;
    esac

    if [[ "$budget_class" == "balanced" || "$budget_class" == "aggressive" ]]; then
        if [[ "$budget_gb" -lt 2 ]]; then
            budget_gb=2
        fi
    elif [[ "$budget_gb" -lt 1 ]]; then
        budget_gb=1
    fi

    if [[ "$budget_class" == "conservative" && "$budget_gb" -gt 4 ]]; then
        budget_gb=4
    fi
    if [[ "$budget_gb" -gt 8 ]]; then
        budget_gb=8
    fi

    printf '%s\n' "$(( budget_gb * 1024 * 1024 * 1024 ))"
}

axon_compute_watcher_subtree_hint_budget() {
    case "${1:?watcher policy required}" in
        full) printf '128\n' ;;
        bounded) printf '32\n' ;;
        off) printf '0\n' ;;
    esac
}

axon_resolve_resource_policy() {
    local instance_kind="${1:?instance kind required}"
    local cpu_cores=""
    local ram_gb=""

    if [[ -n "${AXON_RESOURCE_POLICY_COMPUTED_INSTANCE:-}" && "$AXON_RESOURCE_POLICY_COMPUTED_INSTANCE" != "$instance_kind" ]]; then
        for scoped_var in \
            AXON_RESOURCE_PRIORITY \
            AXON_BACKGROUND_BUDGET_CLASS \
            AXON_GPU_ACCESS_POLICY \
            AXON_WATCHER_POLICY \
            MAX_AXON_WORKERS \
            AXON_QUEUE_MEMORY_BUDGET_BYTES \
            AXON_WATCHER_SUBTREE_HINT_BUDGET \
            AXON_EMBEDDING_PROVIDER
        do
            local source_var="AXON_POLICY_SOURCE_${scoped_var}"
            if [[ "${!source_var:-}" == "policy_default" ]]; then
                unset "$scoped_var"
            fi
        done
    fi

    cpu_cores="$(axon_detect_host_cpu_cores)"
    ram_gb="$(axon_detect_host_ram_gb)"

    if axon_normalize_resource_priority "${AXON_RESOURCE_PRIORITY:-}" >/dev/null 2>&1; then
        export AXON_RESOURCE_PRIORITY
        export AXON_POLICY_SOURCE_AXON_RESOURCE_PRIORITY="explicit"
    else
        export AXON_RESOURCE_PRIORITY="$(axon_default_resource_priority "$instance_kind")"
        export AXON_POLICY_SOURCE_AXON_RESOURCE_PRIORITY="policy_default"
    fi
    if axon_normalize_background_budget_class "${AXON_BACKGROUND_BUDGET_CLASS:-}" >/dev/null 2>&1; then
        export AXON_BACKGROUND_BUDGET_CLASS
        export AXON_POLICY_SOURCE_AXON_BACKGROUND_BUDGET_CLASS="explicit"
    else
        export AXON_BACKGROUND_BUDGET_CLASS="$(axon_default_background_budget_class "$instance_kind")"
        export AXON_POLICY_SOURCE_AXON_BACKGROUND_BUDGET_CLASS="policy_default"
    fi
    if axon_normalize_gpu_access_policy "${AXON_GPU_ACCESS_POLICY:-}" >/dev/null 2>&1; then
        export AXON_GPU_ACCESS_POLICY
        export AXON_POLICY_SOURCE_AXON_GPU_ACCESS_POLICY="explicit"
    else
        export AXON_GPU_ACCESS_POLICY="$(axon_default_gpu_access_policy "$instance_kind")"
        export AXON_POLICY_SOURCE_AXON_GPU_ACCESS_POLICY="policy_default"
    fi
    if axon_normalize_watcher_policy "${AXON_WATCHER_POLICY:-}" >/dev/null 2>&1; then
        export AXON_WATCHER_POLICY
        export AXON_POLICY_SOURCE_AXON_WATCHER_POLICY="explicit"
    else
        export AXON_WATCHER_POLICY="$(axon_default_watcher_policy "$instance_kind")"
        export AXON_POLICY_SOURCE_AXON_WATCHER_POLICY="policy_default"
    fi

    export AXON_RESOURCE_POLICY_CPU_CORES="$cpu_cores"
    export AXON_RESOURCE_POLICY_RAM_GB="$ram_gb"
    export AXON_EFFECTIVE_MAX_AXON_WORKERS="$(
        axon_compute_worker_cap "$instance_kind" "$AXON_BACKGROUND_BUDGET_CLASS" "$cpu_cores"
    )"
    export AXON_EFFECTIVE_QUEUE_MEMORY_BUDGET_BYTES="$(
        axon_compute_queue_memory_budget_bytes "$AXON_BACKGROUND_BUDGET_CLASS" "$ram_gb"
    )"
    export AXON_EFFECTIVE_WATCHER_SUBTREE_HINT_BUDGET="$(
        axon_compute_watcher_subtree_hint_budget "$AXON_WATCHER_POLICY"
    )"

    if [[ -z "${MAX_AXON_WORKERS:-}" ]]; then
        export MAX_AXON_WORKERS="$AXON_EFFECTIVE_MAX_AXON_WORKERS"
        export AXON_POLICY_SOURCE_MAX_AXON_WORKERS="policy_default"
    fi
    if [[ -z "${AXON_QUEUE_MEMORY_BUDGET_BYTES:-}" ]]; then
        export AXON_QUEUE_MEMORY_BUDGET_BYTES="$AXON_EFFECTIVE_QUEUE_MEMORY_BUDGET_BYTES"
        export AXON_POLICY_SOURCE_AXON_QUEUE_MEMORY_BUDGET_BYTES="policy_default"
    fi
    if [[ -z "${AXON_WATCHER_SUBTREE_HINT_BUDGET:-}" ]]; then
        export AXON_WATCHER_SUBTREE_HINT_BUDGET="$AXON_EFFECTIVE_WATCHER_SUBTREE_HINT_BUDGET"
        export AXON_POLICY_SOURCE_AXON_WATCHER_SUBTREE_HINT_BUDGET="policy_default"
    fi

    if [[ "$AXON_GPU_ACCESS_POLICY" == "avoid" && -z "${AXON_EMBEDDING_PROVIDER:-}" ]]; then
        export AXON_EMBEDDING_PROVIDER="cpu"
        export AXON_POLICY_SOURCE_AXON_EMBEDDING_PROVIDER="policy_default"
    fi

    export AXON_RESOURCE_POLICY_COMPUTED_INSTANCE="$instance_kind"
}
