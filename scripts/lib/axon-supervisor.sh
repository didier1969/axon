#!/usr/bin/env bash
# axon-supervisor.sh — process-compose supervisor lifecycle helpers shared by
# scripts/start.sh and scripts/stop.sh.
#
# Reason of existence (REQ-AXO-901735 hardening):
#   1. The process-compose management API port (live=8080 / dev=8081) was
#      derived independently in start.sh AND stop.sh (and again in two python
#      tools). Duplicated magic numbers drift; this lib is the single source
#      of truth for `axon_pc_port_for_instance`.
#   2. A `stop` that didn't fully reap left ORPHAN process-compose supervisors
#      holding the instance port AND the canonical brain port, so the next
#      `start` failed "address already in use". These helpers reap the Axon
#      process tree (supervisor + its axon-brain/axon-indexer/dashboard
#      children) scoped PRECISELY to this repo's bin paths / instance config /
#      canonical port — never a broad pkill, never a process outside this repo.
#
# All reaping is PID-anchored: PIDs are resolved from `ss -ltnp` on the
# instance's ports and from `pgrep -f` scoped to "${PROJECT_ROOT}" and the
# process-compose config file, then signalled by explicit PID.

# Idempotent sourcing guard.
if [[ -n "${_AXON_SUPERVISOR_LIB_LOADED:-}" ]]; then
    return 0 2>/dev/null || exit 0
fi
_AXON_SUPERVISOR_LIB_LOADED=1

# axon_pc_port_for_instance <instance_kind> — canonical process-compose
# management API port. SINGLE SOURCE OF TRUTH (consumed by start.sh + stop.sh).
axon_pc_port_for_instance() {
    case "${1:-live}" in
        live) printf '8080\n' ;;
        dev)  printf '8081\n' ;;
        *)    printf '8080\n' ;;
    esac
}

# axon_pc_config_path <project_root> <instance_kind> — absolute path to the
# process-compose config for this instance. Used to scope pgrep matches so we
# only ever touch a supervisor launched against THIS repo's config.
axon_pc_config_path() {
    local project_root="${1:?project root required}"
    local instance_kind="${2:?instance kind required}"
    printf '%s/process-compose.%s.yaml\n' "$project_root" "$instance_kind"
}

# axon_port_listener_pids <port> — PIDs LISTENing on <port>, one per line.
# Anchored on the LISTEN state and an exact port match (suffix of $4). Returns
# 0 with empty stdout when nothing listens (no `set -e` trip).
axon_port_listener_pids() {
    local port="${1:?port required}"
    ss -ltnp 2>/dev/null | awk -v p="$port" '
        $1 == "LISTEN" {
            n = split($4, addr_parts, ":")
            if (addr_parts[n] != p) next
            while (match($0, /pid=([0-9]+)/)) {
                pid = substr($0, RSTART + 4, RLENGTH - 4)
                print pid
                $0 = substr($0, RSTART + RLENGTH)
            }
        }' 2>/dev/null | awk 'NF' | sort -u
}

# axon_pc_supervisor_pids <project_root> <instance_kind> — PIDs of any
# process-compose supervisor launched against THIS repo's instance config.
# Scoped by BOTH the process-compose binary name AND the config path, so an
# unrelated process-compose for another project is never matched. Empty stdout
# + rc 0 when none.
axon_pc_supervisor_pids() {
    local project_root="${1:?project root required}"
    local instance_kind="${2:?instance kind required}"
    local cfg
    cfg="$(axon_pc_config_path "$project_root" "$instance_kind")"
    # pgrep -f matches the full cmdline; require both "process-compose" and the
    # exact instance config path. pgrep returns 1 on no-match → swallow it.
    pgrep -f "process-compose.*${cfg}" 2>/dev/null | awk 'NF' | sort -u || true
}

# axon_repo_runtime_child_pids <project_root> — PIDs of axon-brain / axon-indexer
# / dashboard BEAM children that belong to THIS repo. Scoped to "${project_root}"
# so other clones / other projects are never touched. Empty stdout + rc 0 when
# none. Used as a belt-and-suspenders sweep after the supervisor is down.
axon_repo_runtime_child_pids() {
    local project_root="${1:?project root required}"
    local node_name="${2:-}"
    local out=""
    local add
    add="$(pgrep -f "${project_root}/bin/axon-brain( |\$)|${project_root}/bin/axon-indexer( |\$)|${project_root}/.axon[^ ]*/cargo-target/[^ ]*/axon-brain( |\$)|${project_root}/.axon[^ ]*/cargo-target/[^ ]*/axon-indexer( |\$)" 2>/dev/null || true)"
    [[ -n "$add" ]] && out="$out
$add"
    # Dashboard BEAM: matched by Erlang node name (cmdline loses project_root).
    if [[ -n "$node_name" ]]; then
        add="$(pgrep -f "beam.smp.*${node_name}" 2>/dev/null || true)"
        [[ -n "$add" ]] && out="$out
$add"
    fi
    printf '%s\n' "$out" | awk 'NF' | sort -u
}

# axon_kill_pids_graceful <signal-escalation> <pid...> — send SIGTERM, wait up
# to ~5s, then SIGKILL any survivor. Each PID is validated to still exist
# before signalling. Best-effort; returns 0 always.
axon_kill_pids_graceful() {
    local pid
    local -a pids=("$@")
    (( ${#pids[@]} > 0 )) || return 0
    for pid in "${pids[@]}"; do
        [[ "$pid" =~ ^[0-9]+$ ]] || continue
        kill -0 "$pid" 2>/dev/null && kill -TERM "$pid" 2>/dev/null || true
    done
    local w
    for ((w = 0; w < 25; w++)); do
        local alive=0
        for pid in "${pids[@]}"; do
            [[ "$pid" =~ ^[0-9]+$ ]] || continue
            if kill -0 "$pid" 2>/dev/null; then alive=1; break; fi
        done
        (( alive == 0 )) && return 0
        sleep 0.2
    done
    for pid in "${pids[@]}"; do
        [[ "$pid" =~ ^[0-9]+$ ]] || continue
        kill -0 "$pid" 2>/dev/null && kill -KILL "$pid" 2>/dev/null || true
    done
    return 0
}

# axon_port_is_free <port> — 0 if NOTHING listens on <port>, 1 otherwise.
axon_port_is_free() {
    local port="${1:?port required}"
    local pids
    pids="$(axon_port_listener_pids "$port")"
    [[ -z "$pids" ]]
}

# axon_supervisor_healthy <pc_port> — 0 if a process-compose daemon answers its
# /live management endpoint on <pc_port> (i.e. a real supervisor is up), 1
# otherwise. Used by start.sh to distinguish a HEALTHY instance (abort) from a
# stale orphan holding the port (reclaim).
axon_supervisor_healthy() {
    local pc_port="${1:?pc port required}"
    curl -sf --connect-timeout 3 "http://127.0.0.1:${pc_port}/live" >/dev/null 2>&1
}

# axon_brain_healthy <brain_port> — 0 if the brain answers /readyz, 1 otherwise.
axon_brain_healthy() {
    local brain_port="${1:?brain port required}"
    curl -sf --connect-timeout 3 "http://127.0.0.1:${brain_port}/readyz" >/dev/null 2>&1
}

# axon_reap_supervisor_tree — reap the process-compose supervisor for this
# instance + its repo-scoped runtime children, then verify the canonical brain
# port is freed (retry/escalate to SIGKILL if still bound). Best-effort but
# returns 1 if the brain port is STILL held after escalation, so callers can
# surface a hard stop failure.
#
# Args (all required, passed explicitly to avoid re-deriving):
#   $1 project_root   $2 instance_kind   $3 brain_port   $4 pc_bin (may be "")
#   $5 node_name (Elixir node, may be "")
axon_reap_supervisor_tree() {
    local project_root="${1:?project root required}"
    local instance_kind="${2:?instance kind required}"
    local brain_port="${3:?brain port required}"
    local pc_bin="${4:-}"
    local node_name="${5:-}"
    local pc_port
    pc_port="$(axon_pc_port_for_instance "$instance_kind")"

    # 1. Graceful supervisor shutdown via the PC management API (kills children
    #    too, honouring their shutdown signals). Only if a daemon answers.
    if axon_supervisor_healthy "$pc_port" && [[ -x "${pc_bin:-}" ]]; then
        _axon_sup_log "Stopping process-compose supervisor on :${pc_port}..."
        "$pc_bin" down -p "$pc_port" 2>/dev/null || true
        local w
        for ((w = 0; w < 20; w++)); do
            axon_supervisor_healthy "$pc_port" || break
            sleep 0.25
        done
    fi

    # 2. Reap any orphan supervisor still bound to the PC port (config-scoped
    #    PIDs ∪ PIDs LISTENing on the PC port that match this repo's config).
    local sup_pids
    sup_pids="$(axon_pc_supervisor_pids "$project_root" "$instance_kind")"
    if [[ -n "$sup_pids" ]]; then
        _axon_sup_log "Reaping orphan supervisor PID(s): ${sup_pids//$'\n'/ }"
        # shellcheck disable=SC2086
        axon_kill_pids_graceful $sup_pids
    fi

    # 3. Belt-and-suspenders: reap repo-scoped runtime children that may have
    #    detached from a dead supervisor (e.g. dev release brain under
    #    .axon/cargo-target, invisible to bin/-anchored matchers).
    local child_pids
    child_pids="$(axon_repo_runtime_child_pids "$project_root" "$node_name")"
    if [[ -n "$child_pids" ]]; then
        _axon_sup_log "Reaping repo runtime child PID(s): ${child_pids//$'\n'/ }"
        # shellcheck disable=SC2086
        axon_kill_pids_graceful $child_pids
    fi

    # 4. Verify the canonical brain port is freed; escalate to SIGKILL by PID.
    if axon_port_is_free "$brain_port"; then
        return 0
    fi
    local port_pids
    port_pids="$(axon_port_listener_pids "$brain_port")"
    if [[ -n "$port_pids" ]]; then
        _axon_sup_warn "Brain port :${brain_port} still bound after SIGTERM (pids: ${port_pids//$'\n'/ }) — escalating to SIGKILL."
        local pid
        for pid in $port_pids; do
            [[ "$pid" =~ ^[0-9]+$ ]] || continue
            kill -KILL "$pid" 2>/dev/null || true
        done
        local w
        for ((w = 0; w < 20; w++)); do
            axon_port_is_free "$brain_port" && return 0
            sleep 0.25
        done
    fi
    axon_port_is_free "$brain_port"
}

# Minimal logging shims (reuse axon-log.sh markers when available).
_axon_sup_log() {
    if declare -F axon_log_step >/dev/null 2>&1; then
        axon_log_step "$*"
    else
        printf '👉 %s\n' "$*"
    fi
}
_axon_sup_warn() {
    if declare -F axon_log_warn >/dev/null 2>&1; then
        axon_log_warn "$*"
    else
        printf '⚠️  %s\n' "$*" >&2
    fi
}
