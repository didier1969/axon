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

# axon_brain_port_for_instance <instance_kind> — canonical axon-brain MCP/SQL
# HTTP port. Mirrors AXON_BRAIN_PORT in axon-instance.sh. SINGLE SOURCE OF TRUTH
# for the embed-provider auto-release (REQ-AXO-234 layer B).
axon_brain_port_for_instance() {
    case "${1:-live}" in
        live) printf '44129\n' ;;
        dev)  printf '44139\n' ;;
        *)    printf '44129\n' ;;
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

# axon_repo_runtime_child_pids <project_root> <instance_kind> [node_name] — PIDs
# of axon-brain / axon-indexer / dashboard BEAM children that belong to THIS repo
# AND THIS instance. Scoped to "${project_root}" so other clones / projects are
# never touched, AND to the instance's binary location so a `dev` stop NEVER
# reaps `live` and vice-versa (the bug: a repo-wide bin/+cargo-target match
# killed the live brain/indexer during a dev stop). Canonical invariant
# (CLAUDE.md deployment): live runs the promoted RELEASE binaries under bin/
# ONLY ; dev runs cargo-target builds (debug or release) ONLY. Empty stdout +
# rc 0 when none. Belt-and-suspenders sweep after the supervisor is down.
axon_repo_runtime_child_pids() {
    local project_root="${1:?project root required}"
    local instance_kind="${2:?instance kind required}"
    local node_name="${3:-}"
    local out=""
    local add
    local bin_pat
    case "$instance_kind" in
        live)
            # Live = promoted release binaries under bin/ ONLY (never cargo-target).
            bin_pat="${project_root}/bin/axon-brain( |\$)|${project_root}/bin/axon-indexer( |\$)"
            ;;
        *)
            # Dev = cargo-target builds (debug or release) ONLY (never bin/).
            bin_pat="${project_root}/.axon[^ ]*/cargo-target/[^ ]*/axon-brain( |\$)|${project_root}/.axon[^ ]*/cargo-target/[^ ]*/axon-indexer( |\$)"
            ;;
    esac
    add="$(pgrep -f "$bin_pat" 2>/dev/null || true)"
    [[ -n "$add" ]] && out="$out
$add"
    # Dashboard BEAM: matched by Erlang node name (cmdline loses project_root).
    # The node name is already instance-specific, so this stays scoped.
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

# ---------------------------------------------------------------------------
# REQ-AXO-902263 — VERIFIED per-role restart.
#
# Why this exists: `POST /process/restart/<name>` returns HTTP 200 and then does
# NOT necessarily restart the process. Observed on the live indexer (session
# 104): 200 + {"name":"axon-indexer"}, then status=Terminating for ~4 minutes
# (a tokio worker stuck in state D on wchan `dxgvmb_send_sync_msg` — the WSL2
# GPU VM-bus channel, unkillable even by SIGKILL), then status=Completed with NO
# new process. Because the stop was REQUESTED, `availability.restart:
# on_failure` never fires, so process-compose leaves the role down.
#
# Every caller that trusted the HTTP code was therefore reporting a recovery it
# had not performed — the promote's step-6c TIER-1 among them. The fix is not a
# longer timeout: it is to stop believing the return code and poll the OBSERVED
# state instead. Same lesson as REQ-AXO-902258 (`promoted: true` on a wrong
# binary) and REQ-AXO-902262 (`status: ok` on destroyed data).
# ---------------------------------------------------------------------------

# axon_role_recovery_action <observed_status> <observed_pid> <original_pid>
#
# PURE decision function (no I/O, no process side effect — unit-tested in
# axon-supervisor.test.sh). Echoes the next action for the polling loop:
#   done  — the role is back on a genuinely NEW process
#   start — process-compose considers the role finished; it needs an explicit start
#   wait  — still converging, keep polling
#
# The `pid != original` clause is the crux: right after a restart request the OLD
# process can still be reported Running/Ready for several seconds. Accepting that
# as success is exactly the false positive this whole REQ is about.
axon_role_recovery_action() {
    local status="${1:-}" observed_pid="${2:-}" original_pid="${3:-}"
    case "$status" in
        Running)
            # Ready-ness is checked by the caller (it needs a second field); here
            # only the identity question is decided.
            if [[ -n "$observed_pid" && "$observed_pid" != "0" && "$observed_pid" != "$original_pid" ]]; then
                printf 'done\n'
            else
                printf 'wait\n'
            fi
            ;;
        Completed|Stopped|Skipped|Disabled)
            # The observed defect: a requested stop is not a "failure", so the
            # supervisor will never restart it on its own.
            printf 'start\n'
            ;;
        *)
            # Terminating / Restarting / Launching / Pending / unreachable ("").
            printf 'wait\n'
            ;;
    esac
}

# _axon_role_field <pc_port> <process> <json_key> — read one field of the
# supervisor's view of a process. Empty on any failure (daemon down, unknown
# process, malformed body) so callers treat "unknown" as "keep waiting" rather
# than as a verdict.
_axon_role_field() {
    local pc_port="${1:?pc port required}" proc="${2:?process required}" key="${3:?key required}"
    curl -s -m 8 "http://127.0.0.1:${pc_port}/process/${proc}" 2>/dev/null \
        | python3 -c "
import json, sys
try:
    print(json.load(sys.stdin).get(sys.argv[1], '') or '')
except Exception:
    print('')
" "$key" 2>/dev/null || printf ''
}

# _axon_role_health_port <instance_kind> <process> — the role's own health port, or empty
# when the role has none (only the two runtime roles expose one).
_axon_role_health_port() {
    local instance_kind="${1:?}" proc="${2:?}"
    case "$proc" in
        axon-indexer) [[ "$instance_kind" == "live" ]] && printf '44130' || printf '44149' ;;
        axon-brain)   axon_brain_port_for_instance "$instance_kind" ;;
        *)            printf '' ;;
    esac
}

# _axon_role_serving <instance_kind> <process> — 0 when the role answers its OWN health
# endpoint, whatever the supervisor believes.
#
# This is the ground truth that outranks process-compose's bookkeeping: the supervisor
# tracks the last process IT launched, which may be a duplicate the writer guard refused,
# while the real instance keeps serving. Roles without a health port return 1 (unknown ≠
# serving) so callers fall back to the supervisor's view rather than assume health.
_axon_role_serving() {
    local port
    port="$(_axon_role_health_port "$1" "$2")"
    [[ -n "$port" ]] || return 1
    curl -sf --connect-timeout 3 -m 5 "http://127.0.0.1:${port}/readyz" >/dev/null 2>&1
}

# axon_restart_role_verified <instance_kind> <process> [budget_s]
#
# Restart ONE process-compose role and return 0 only once the OBSERVED state is
# Running AND is_ready=Ready AND running under a pid different from the one seen
# before the request. Returns 1 on budget exhaustion, naming the terminal state
# so the caller can escalate knowingly (the promote escalates to a full restart).
#
# NEVER touches the brain: no /mcp call, no embed_provider flip. The operator's
# constraint is explicit — dropping the live INDEXER for seconds to 2-3 minutes is
# fine, the BRAIN is what every LLM depends on. Hence the 180 s default budget.
axon_restart_role_verified() {
    local instance_kind="${1:?instance kind required}"
    local proc="${2:?process name required}"
    local budget_s="${3:-180}"
    local pc_port original_pid status observed_pid ready action started elapsed start_sent=0

    pc_port="$(axon_pc_port_for_instance "$instance_kind")"
    if ! axon_supervisor_healthy "$pc_port"; then
        _axon_sup_warn "[restart-verified] no supervisor on :${pc_port} for instance=${instance_kind} — cannot restart ${proc}"
        return 1
    fi

    original_pid="$(_axon_role_field "$pc_port" "$proc" pid)"
    [[ -n "$original_pid" ]] || original_pid="0"
    _axon_sup_log "[restart-verified] ${proc} (instance=${instance_kind}, pid=${original_pid}, budget=${budget_s}s)"

    # Fire and FORGET the HTTP result on purpose: the response code carries no
    # information about whether the role actually came back (that is the defect).
    curl -s -m 15 -o /dev/null -X POST \
        "http://127.0.0.1:${pc_port}/process/restart/${proc}" >/dev/null 2>&1 || true

    started="$SECONDS"
    while :; do
        elapsed=$(( SECONDS - started ))
        status="$(_axon_role_field "$pc_port" "$proc" status)"
        observed_pid="$(_axon_role_field "$pc_port" "$proc" pid)"
        ready="$(_axon_role_field "$pc_port" "$proc" is_ready)"
        action="$(axon_role_recovery_action "$status" "$observed_pid" "$original_pid")"

        if [[ "$action" == "done" && "$ready" == "Ready" ]]; then
            _axon_sup_log "[restart-verified] ${proc} back after ${elapsed}s (pid ${original_pid} → ${observed_pid}, ready)"
            return 0
        fi
        if [[ "$action" == "start" && "$start_sent" -eq 0 ]]; then
            # REQ-AXO-902263 — before spawning, check whether the ROLE IS ALREADY SERVING.
            # process-compose can report `Completed` while a perfectly healthy instance runs:
            # its status then tracks a REFUSED DUPLICATE, not the live process. Observed for
            # real — the indexer answered /readyz and /livez with a 3.7 s-fresh heartbeat
            # while the supervisor said Completed, because earlier `start` calls had spawned
            # duplicates that the IST writer guard correctly refused ("ownership is already
            # held ... owner=...;pid=..."). Firing another start there manufactures another
            # doomed process and inflates the restart counter — the caller creating the mess
            # it is trying to clean up.
            if _axon_role_serving "$instance_kind" "$proc"; then
                _axon_sup_log "[restart-verified] ${proc} reports '${status}' but IS SERVING its health endpoint — supervisor is tracking a refused duplicate, not the live process. No start sent."
                return 0
            fi
            # The observed process-compose defect. Send the missing half ONCE —
            # resending on every tick would fight a slow launch.
            _axon_sup_log "[restart-verified] ${proc} reported '${status}' — supervisor will not relaunch a requested stop; sending explicit start"
            curl -s -m 30 -o /dev/null -X POST \
                "http://127.0.0.1:${pc_port}/process/start/${proc}" >/dev/null 2>&1 || true
            start_sent=1
        fi
        if (( elapsed >= budget_s )); then
            _axon_sup_warn "[restart-verified] ${proc} NOT recovered within ${budget_s}s — terminal observed state: status='${status:-unreachable}' ready='${ready:-?}' pid='${observed_pid:-?}' (was ${original_pid})"
            return 1
        fi
        sleep 3
    done
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
        # REQ-AXO-901929 — `process-compose down --ordered-shutdown` hangs
        # FOREVER when a managed child is <defunct> (zombie): it waits on a
        # process that will never reap. The bare `|| true` catches a non-zero
        # exit but NOT a hang, so a single zombie indexer wedged the whole
        # promote (step 5 restart) and every stop --hard. Bound it: on hang,
        # timeout kills the client and we fall through to the SIGKILL-by-PID
        # reap (steps 2-4 below), which tears the supervisor down regardless.
        timeout -k 5 25 "$pc_bin" down -p "$pc_port" 2>/dev/null || true
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
    child_pids="$(axon_repo_runtime_child_pids "$project_root" "$instance_kind" "$node_name")"
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

# ---------------------------------------------------------------------------
# REQ-AXO-234 layer B — auto-release of the live brain's query-embed lane.
#
# Pausing the live indexer (layer A) stops the BATCH GPU lane, but the brain
# keeps the CUDA EP warm for punctual query-embeds (`query`/`why`/
# `retrieve_context`). On a single-GPU host that residual lane still contends
# with a dev bench / dev-GPU session. These best-effort helpers flip the live
# brain's query-embed provider to `cpu` on a dev GPU start (releasing the GPU)
# and restore the previous override on the dev stop. The brain rebuilds its
# query model lazily on the next request — no restart. Both calls are
# best-effort: a DOWN brain only logs a warning, never aborts the caller.
# ---------------------------------------------------------------------------

# _axon_live_embed_provider_get <brain_port> — echo the brain's current
# query-embed override (unset|cpu|gpu|auto). Empty stdout on any failure.
_axon_live_embed_provider_get() {
    local brain_port="${1:?brain port required}"
    local resp
    resp="$(curl -fsS --max-time 5 -X POST "http://127.0.0.1:${brain_port}/mcp" \
        -H "Content-Type: application/json" \
        -d '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"embed_provider","arguments":{"action":"get"}}}' \
        2>/dev/null)" || return 0
    printf '%s' "$resp" | python3 -c '
import json, sys
try:
    doc = json.load(sys.stdin)
    ov = doc.get("result", {}).get("data", {}).get("override")
    if isinstance(ov, str) and ov:
        print(ov)
except Exception:
    pass
' 2>/dev/null || true
}

# _axon_live_embed_provider_set <brain_port> <cpu|gpu|auto> — best-effort flip
# of the brain's query-embed provider override. rc 0 even on transport failure.
_axon_live_embed_provider_set() {
    local brain_port="${1:?brain port required}"
    local provider="${2:?provider required}"
    curl -fsS --max-time 5 -X POST "http://127.0.0.1:${brain_port}/mcp" \
        -H "Content-Type: application/json" \
        -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"embed_provider\",\"arguments\":{\"action\":\"set\",\"provider\":\"${provider}\"}}}" \
        >/dev/null 2>&1
}

# ---------------------------------------------------------------------------
# REQ-AXO-234 — single-GPU exclusion automation (DEC-AXO-067, PIL-AXO-004).
#
# On a single-GPU host the live indexer and a dev `--indexer-full`/`-vector`
# session cannot share the device. These helpers automate the previously MANUAL
# contract ("stop the live indexer before a dev GPU session"): a dev GPU start
# pauses the live indexer and drops a marker; the dev stop resumes it and clears
# the marker. Both are idempotent and best-effort — they never abort the caller.
# ---------------------------------------------------------------------------

# Marker recording that a dev GPU session paused the live indexer. Lives under
# the LIVE state root so it is discoverable regardless of which dev invocation
# clears it. Single source of truth for the path (both pause + resume call it).
axon_live_pause_marker_path() {
    local project_root="${1:?project root required}"
    printf '%s\n' "$project_root/.axon/live-paused-by-dev"
}

# axon_auto_pause_live_indexer_for_dev <project_root> <pc_bin> <runtime_mode>
# Pause the live indexer when a dev GPU session starts. No-op unless the current
# instance is dev, the mode uses the GPU, and a live supervisor is up.
axon_auto_pause_live_indexer_for_dev() {
    local project_root="${1:?project root required}"
    local pc_bin="${2:-}"
    local runtime_mode="${3:-}"
    [[ "${AXON_INSTANCE_KIND:-}" == "dev" ]] || return 0
    [[ "$runtime_mode" == "indexer_full" || "$runtime_mode" == "indexer_vector" ]] || return 0

    local live_pc_port live_brain_port marker prev_provider
    live_pc_port="$(axon_pc_port_for_instance live)"
    live_brain_port="$(axon_brain_port_for_instance live)"
    marker="$(axon_live_pause_marker_path "$project_root")"

    if axon_supervisor_healthy "$live_pc_port" && [[ -x "$pc_bin" ]]; then
        _axon_sup_log "[auto-pause] dev GPU start → pausing live indexer (single-GPU exclusion DEC-AXO-067, REQ-AXO-234)"
        "$pc_bin" process stop axon-indexer -p "$live_pc_port" 2>/dev/null || true

        # REQ-AXO-234 layer B — also release the brain's punctual query-embed
        # lane so it stops contending for the GPU. Record the PREVIOUS override
        # for an exact restore on resume; flip to cpu (best-effort, brain rebuilds
        # its query model lazily on the next request — no restart).
        prev_provider="$(_axon_live_embed_provider_get "$live_brain_port")"
        [[ -n "$prev_provider" ]] || prev_provider="unset"
        if _axon_live_embed_provider_set "$live_brain_port" cpu; then
            _axon_sup_log "[auto-pause] live query-embed lane → cpu (was \`$prev_provider\`), GPU released for dev (REQ-AXO-234 layer B)"
        else
            _axon_sup_warn "[auto-pause] could not flip live query-embed provider to cpu (brain unreachable) — indexer pause still in effect"
        fi

        mkdir -p "$(dirname "$marker")"
        printf 'paused_by=dev\npaused_at=%s\ndev_pid=%s\nlive_pc_port=%s\nlive_brain_port=%s\nprev_embed_provider=%s\n' \
            "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$$" "$live_pc_port" "$live_brain_port" "$prev_provider" >"$marker"
    else
        # No live supervisor (or no pc binary) — nothing to pause. Drop any stale
        # marker so a later dev stop does not spuriously resume a non-paused live.
        rm -f "$marker" 2>/dev/null || true
    fi
}

# axon_resume_live_indexer_after_dev <project_root> <pc_bin>
# Resume the live indexer a prior dev GPU session paused. No-op unless the
# current instance is dev and the marker is present. Idempotent: always clears
# the marker, even when the live supervisor is down (it starts its own indexer
# fresh next time).
axon_resume_live_indexer_after_dev() {
    local project_root="${1:?project root required}"
    local pc_bin="${2:-}"
    [[ "${AXON_INSTANCE_KIND:-}" == "dev" ]] || return 0

    local marker live_pc_port live_brain_port prev_provider restore_provider
    marker="$(axon_live_pause_marker_path "$project_root")"
    [[ -f "$marker" ]] || return 0
    live_pc_port="$(axon_pc_port_for_instance live)"

    # Recover the brain port + previous query-embed override recorded at pause
    # time. Fall back to canonical defaults for markers written before layer B.
    live_brain_port="$(sed -n 's/^live_brain_port=//p' "$marker" 2>/dev/null | head -n1)"
    [[ -n "$live_brain_port" ]] || live_brain_port="$(axon_brain_port_for_instance live)"
    prev_provider="$(sed -n 's/^prev_embed_provider=//p' "$marker" 2>/dev/null | head -n1)"

    if axon_supervisor_healthy "$live_pc_port" && [[ -x "$pc_bin" ]]; then
        _axon_sup_log "[auto-resume] dev stop → resuming live indexer paused for the GPU session (REQ-AXO-234)"
        # REQ-AXO-902263 — was `"$pc_bin" process start … || true`: a silent failure left
        # LIVE without an indexer after every dev session, and nothing reported it. Same
        # class as the promote's TIER-1 (trusting a request instead of checking the effect).
        # Verify, and be LOUD on failure — the operator can then restore it deliberately
        # instead of discovering a degraded live hours later.
        if ! axon_restart_role_verified live axon-indexer 180; then
            _axon_sup_warn "[auto-resume] live indexer did NOT come back — LIVE IS WITHOUT AN INDEXER. Restore with: curl -X POST :${live_pc_port}/process/start/axon-indexer"
        fi

        # REQ-AXO-234 layer B — restore the brain's query-embed provider. `set`
        # only accepts cpu|gpu|auto, so an `unset`/missing prior maps to `auto`
        # (GPU-when-free), the closest restore of the no-override default.
        if [[ -n "$prev_provider" && "$prev_provider" != "unset" ]]; then
            restore_provider="$prev_provider"
        else
            restore_provider="auto"
        fi
        if _axon_live_embed_provider_set "$live_brain_port" "$restore_provider"; then
            _axon_sup_log "[auto-resume] live query-embed lane restored → \`$restore_provider\` (REQ-AXO-234 layer B)"
        else
            _axon_sup_warn "[auto-resume] could not restore live query-embed provider to \`$restore_provider\` (brain unreachable)"
        fi
    else
        _axon_sup_warn "[auto-resume] live supervisor down — clearing pause marker without resume (live starts its own indexer)"
    fi
    rm -f "$marker" 2>/dev/null || true
}
