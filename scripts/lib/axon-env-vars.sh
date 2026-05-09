#!/usr/bin/env bash

# REQ-AXO-241 — Single source of truth for AXON_* / HYDRA_* env var
# lifecycle. Replaces two parallel hardcoded allowlists:
#
#   1. axon-instance.sh:axon_clear_inherited_env preserve-list
#   2. start.sh:PASS_THROUGH_EXPORTS propagate-list
#
# Default policy: any var matching AXON_* / HYDRA_* (and a small set of
# explicit foreign prefixes like OMP_*) is PRESERVED across the lifecycle
# clear step and PROPAGATED into the tmux-spawned devenv shell. Adding a
# new tunable knob now requires changes in zero places.
#
# Exception: derived per-instance / per-run vars below MUST be rebuilt
# per-run (PID files, sockets, runtime identities, lifecycle script
# internals) so they never leak between live/dev or between consecutive
# runs in the same shell. They are listed here once and consumed by both
# axon_clear_inherited_env and the PASS_THROUGH iterator.
#
# How to add a new env var (the goal of this REQ):
#
#   - Tunable knob (operator-provided)        → just `export AXON_FOO=bar`
#                                                and run a lifecycle. No
#                                                code change required.
#   - Per-instance derived var (script sets)  → add it to the list below.
#                                                Required exactly once.

# Print one denied (per-instance / derived) env var name per line.
# Consumed by axon_env_var_is_derived (predicate) and the start.sh
# PASS_THROUGH iterator (skip set).
axon_derived_env_var_names() {
    cat <<'EOF'
AXON_PROJECTS_ROOT
AXON_WATCH_DIR
AXON_PROJECT_ROOT
AXON_RUNTIME_MODE
AXON_RUNTIME_SHADOW_ROLE
AXON_SPLIT_SHADOW_ONLY
AXON_MCP_MUTATION_JOBS
AXON_INSTANCE_KIND
AXON_RUNTIME_IDENTITY
AXON_DB_ROOT
AXON_RUN_ROOT
AXON_PID_FILE
AXON_TELEMETRY_SOCK
AXON_MCP_SOCK
AXON_SQL_URL
AXON_MCP_URL
AXON_DASHBOARD_URL
AXON_MUTATION_POLICY
AXON_ENABLE_AUTONOMOUS_INGESTOR
AXON_RUNTIME_PROFILE
AXON_RUNTIME_BOOT_ROLE
AXON_RESOURCE_POLICY_COMPUTED_INSTANCE
AXON_RESOURCE_POLICY_CPU_CORES
AXON_RESOURCE_POLICY_RAM_GB
AXON_EFFECTIVE_MAX_AXON_WORKERS
AXON_EFFECTIVE_QUEUE_MEMORY_BUDGET_BYTES
AXON_EFFECTIVE_WATCHER_SUBTREE_HINT_BUDGET
AXON_WORKTREE_ENV_LOADED
PHX_PORT
HYDRA_TCP_PORT
HYDRA_HTTP_PORT
HYDRA_ODATA_PORT
HYDRA_HTTP2_PORT
HYDRA_MCP_PORT
EOF
}

# Predicate: does this env var name belong to the derived per-run set?
# Returns 0 if derived (denylist hit), 1 otherwise. Also catches the
# AXON_POLICY_SOURCE_* / AXON_RESOURCE_POLICY_* / AXON_EFFECTIVE_*
# wildcard families written by axon-resource-policy.sh.
axon_env_var_is_derived() {
    local name="$1"
    case "$name" in
        AXON_POLICY_SOURCE_*|AXON_RESOURCE_POLICY_*|AXON_EFFECTIVE_*)
            return 0 ;;
    esac
    while IFS= read -r derived; do
        [[ -z "$derived" ]] && continue
        [[ "$name" == "$derived" ]] && return 0
    done < <(axon_derived_env_var_names)
    return 1
}

# Predicate: is this env var name a candidate for the prefix-allowlist
# policy? AXON_*/HYDRA_* are the canonical Axon namespace; OMP_* is the
# narrow set of OpenMP-specific knobs we propagate (OMP_NUM_THREADS,
# OMP_WAIT_POLICY) so the supervised process inherits the same threading
# discipline as the parent shell.
axon_env_var_in_prefix_allowlist() {
    case "$1" in
        AXON_*|HYDRA_*|OMP_NUM_THREADS|OMP_WAIT_POLICY) return 0 ;;
        *) return 1 ;;
    esac
}
