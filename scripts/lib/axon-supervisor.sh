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

# axon_role_supervision_verdict <status> <is_ready> <has_ready_probe> <exit_code> \
#                               <restarts> <max_restarts> <serving> [proc_state]
#
# PURE decision function (no I/O — unit-tested in axon-supervisor.test.sh). Turns one
# process-compose row plus the role's own ground truth into ONE token:
#
#   ok        — Running, and either Ready or nothing to be ready about
#   no_budget — Running, but the restart budget is already spent: no safety net left
#   not_ready — Running but its own readiness probe says no
#   drift     — the supervisor gave up on it, yet the role IS serving its health port
#   disabled  — not selected for this runtime mode (brain_only, dashboard toggle)
#   oneshot   — a task, not a service: no readiness probe and it exited 0
#   exhausted — down AND the restart budget is spent: the supervisor will NEVER retry
#   wedged    — stuck mid-teardown behind an unreapable zombie: self-healing never STARTS
#   down      — down with retries left (the supervisor may still bring it back)
#
# REQ-AXO-902271 — `wedged` names a failure mode `exhausted` does NOT cover, and the
# difference matters because the two need OPPOSITE recovery. `exhausted` is the supervisor
# having tried its 3 restarts and given up. `wedged` is `restarts=0`: it never tried, and
# never will, because from its point of view the stop has not FINISHED. The role's process
# is dead but `<defunct>` — a multi-threaded zombie (`Zl`) with one thread stuck in
# uninterruptible D-state on the WSL2 GPU virtualisation channel
# (`dxgglobal_acquire_process_adapter`), which SIGKILL does not clear. So the role sits at
# `Terminating` indefinitely with its whole restart budget intact — dead with a full tank,
# which no counter reports.
#
# Measured on 2026-07-28 (three promote gate failures in one day, host verifiably idle at
# `0 concurrent rustc`). The `down` verdict this used to yield is not merely imprecise: its
# recovery command is WRONG. `POST /process/start` is ignored while the supervisor believes
# the role is still terminating, and `PATCH stop` answers
# `{"error":"process axon-indexer is not running"}`. Printing a command that cannot work is
# the same class of defect as printing HEALTHY for a dead role.
#
# The discriminator is the ZOMBIE, not elapsed time: an ordinary teardown passes through
# `Terminating` too, and crying wolf on every clean stop would train people to skip this
# section — which is the blindness this whole surface exists to remove. `proc_state`
# defaults to unknown, so a caller that cannot look at the pid degrades to `down` rather
# than inventing a verdict.
#
# REQ-AXO-902264 — `exhausted` is the whole reason this exists. `max_restarts: 3` with
# `restart: on_failure` means self-healing GIVES UP after the third failure and then does
# nothing forever, in silence: the only trace is a log line nobody reads. Meanwhile
# `axon status` reported on ONE role (the one derived from the pid files) and could print
# HEALTHY while another role had been dead for hours. Giving up must not be
# indistinguishable from working.
#
# `serving` outranks the supervisor deliberately, for the same reason as in
# `axon_restart_role_verified`: process-compose tracks the last process IT launched, which
# may be a duplicate the writer guard refused while the real instance keeps answering.
axon_role_supervision_verdict() {
    local status="${1:-}" is_ready="${2:-}" has_probe="${3:-}" exit_code="${4:-0}"
    local restarts="${5:-0}" max_restarts="${6:-0}" serving="${7:-unknown}"
    local proc_state="${8:-unknown}"

    [[ "$restarts" =~ ^[0-9]+$ ]] || restarts=0
    [[ "$max_restarts" =~ ^[0-9]+$ ]] || max_restarts=0

    if [[ "$status" == "Running" ]]; then
        if [[ "$is_ready" != "Ready" && "$has_probe" == "true" ]]; then
            printf 'not_ready\n'
        elif (( max_restarts > 0 && restarts >= max_restarts )); then
            # MEASURED (isolated process-compose 1.94.0 probe, REQ-AXO-902264): the restart
            # counter NEVER goes back down — not after a healthy period, and not after the
            # explicit `POST /process/start` this very tool recommends as the recovery. The
            # role comes back Running with `restarts` still at the ceiling, so the NEXT
            # failure is terminal and nothing says so. A green line here would be the same
            # class of lie the whole REQ is about, one step further down the timeline.
            printf 'no_budget\n'
        else
            printf 'ok\n'
        fi
        return 0
    fi

    # Ground truth first: a role that answers /readyz is not down, whatever the
    # supervisor's bookkeeping says.
    if [[ "$serving" == "yes" ]]; then printf 'drift\n'; return 0; fi
    # REQ-AXO-902271 — before the budget arithmetic, because the budget is IRRELEVANT here:
    # a wedged role has consumed none of it and will consume none of it. Ordering this
    # after the `exhausted`/`down` branches would have hidden the case behind a count that
    # reads perfectly healthy.
    if [[ "$status" == "Terminating" && "$proc_state" == "zombie" ]]; then
        printf 'wedged\n'; return 0
    fi
    # `Disabled` is configuration, not failure: process-compose marks every process the
    # launcher did not select (brain_only omits the indexer; AXON_DASHBOARD_DISABLED omits
    # the dashboard). Surfaced as a warning with its recovery command, never as a failure.
    if [[ "$status" == "Disabled" ]]; then printf 'disabled\n'; return 0; fi
    # A process with no readiness probe that exited 0 is a completed task (postgres-check),
    # not a dead service. This is the only discriminator process-compose offers.
    if [[ "$has_probe" != "true" && "$exit_code" == "0" ]]; then printf 'oneshot\n'; return 0; fi
    if (( max_restarts > 0 && restarts >= max_restarts )); then printf 'exhausted\n'; return 0; fi
    printf 'down\n'
}

# axon_role_survey <project_root> <instance_kind>
#
# One line per supervised role, pipe-separated, for the caller to render:
#   name|status|is_ready|restarts|max_restarts|serving|verdict
#
# Returns 1 (no output) when there is no supervisor to ask — the caller decides whether
# that is expected. `max_restarts` comes from the process-compose YAML because the REST
# API exposes the CONSUMED count and not the budget, so the exhaustion boundary is
# unknowable from the API alone.
axon_role_survey() {
    local project_root="${1:?project root required}" instance_kind="${2:?instance kind required}"
    local pc_port cfg rows name status ready probe code restarts maxr serving verdict
    local pid proc_state

    pc_port="$(axon_pc_port_for_instance "$instance_kind")"
    axon_supervisor_healthy "$pc_port" || return 1
    cfg="$(axon_pc_config_path "$project_root" "$instance_kind")"

    local body
    body="$(curl -s -m 8 "http://127.0.0.1:${pc_port}/processes" 2>/dev/null)" || return 1
    [[ -n "$body" ]] || return 1

    # The payload travels in the ENVIRONMENT, not on stdin: `python3 -` already takes its
    # program from stdin, so a heredoc script and a piped body cannot coexist — the
    # heredoc silently wins and the parse sees an empty document. Same idiom as
    # `AXONCTL_JSON` in scripts/status.sh.
    rows="$(AXON_PC_PROCESSES_JSON="$body" python3 - "$cfg" <<'PY'
import json, os, sys

try:
    body = json.loads(os.environ.get("AXON_PC_PROCESSES_JSON", ""))
except Exception:
    sys.exit(1)
procs = body.get("data", body) if isinstance(body, dict) else body
if not isinstance(procs, list):
    sys.exit(1)

# The restart BUDGET lives only in the YAML; the API reports the consumed count.
budgets = {}
try:
    import yaml
    with open(sys.argv[1], "r", encoding="utf-8") as fh:
        # The YAML carries ${VAR:-default} interpolations; they parse as plain strings,
        # and none of them appear in the availability block we read here.
        conf = yaml.safe_load(fh) or {}
    for pname, pconf in (conf.get("processes") or {}).items():
        avail = (pconf or {}).get("availability") or {}
        budgets[pname] = avail.get("max_restarts", 0)
except Exception:
    budgets = {}

# Sorted by name: the API returns roles in an order that varies between calls, and a
# status surface people read (and diff) twice in a row must not reshuffle itself.
for p in sorted(procs, key=lambda r: str(r.get("name", ""))):
    name = str(p.get("name", "")).strip()
    if not name:
        continue
    print("|".join([
        name,
        str(p.get("status", "")).strip() or "?",
        str(p.get("is_ready", "")).strip() or "-",
        "true" if p.get("has_ready_probe") else "false",
        str(p.get("exit_code", 0)),
        str(p.get("restarts", 0)),
        str(budgets.get(name, 0)),
        # REQ-AXO-902271 — the pid is what tells a teardown in progress apart from one
        # wedged behind an unreapable zombie. The API has it; nothing read it before.
        str(p.get("pid", 0)),
    ]))
PY
)" || return 1
    [[ -n "$rows" ]] || return 1

    while IFS='|' read -r name status ready probe code restarts maxr pid; do
        [[ -n "$name" ]] || continue
        # Probe the role's own health port ONLY when the supervisor claims it is not
        # Running: on a healthy runtime `status` must stay cheap.
        serving="-"
        if [[ "$status" != "Running" ]] && [[ -n "$(_axon_role_health_port "$instance_kind" "$name")" ]]; then
            if _axon_role_serving "$instance_kind" "$name"; then serving="yes"; else serving="no"; fi
        fi
        # REQ-AXO-902271 — only for `Terminating`, and only then: `ps` on every role of a
        # healthy runtime would be paid on every `axon status` for a question that cannot
        # arise. `Zl` is the multi-threaded zombie observed in production; matching on the
        # leading `Z` covers `Z` and `Zl` alike.
        proc_state="unknown"
        if [[ "$status" == "Terminating" ]] && [[ "$pid" =~ ^[0-9]+$ ]] && (( pid > 0 )); then
            case "$(ps -o stat= -p "$pid" 2>/dev/null | tr -d '[:space:]')" in
                Z*) proc_state="zombie" ;;
                "") proc_state="gone" ;;
                *)  proc_state="alive" ;;
            esac
        fi
        verdict="$(axon_role_supervision_verdict "$status" "$ready" "$probe" "$code" "$restarts" "$maxr" "$serving" "$proc_state")"
        printf '%s|%s|%s|%s|%s|%s|%s\n' "$name" "$status" "$ready" "$restarts" "$maxr" "$serving" "$verdict"
    done <<< "$rows"
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
    local pc_port original_pid status observed_pid ready action started elapsed start_sent=0 down_ticks=0

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
        # REQ-AXO-902263 — GROUND TRUTH, checked on EVERY tick the supervisor claims the
        # role is down. process-compose can report `Completed` while a perfectly healthy
        # instance serves: its status then tracks a REFUSED DUPLICATE (the IST writer guard
        # rejects a second writer with "ownership is already held … owner=…;pid=…"), not the
        # live process.
        #
        # This check was FIRST written inside the `start_sent -eq 0` branch, i.e. evaluated
        # ONCE before sending the start and never again. That is exactly wrong: the
        # duplicate-tracking state arises AFTER the start, so the loop then polled a status
        # that could never become Running and burned the whole budget. It failed a real
        # promote at step 2d while `/readyz` answered 200 throughout. Correctness here is
        # "ask the role, every time", not "ask the role once".
        if [[ "$action" == "start" ]] && _axon_role_serving "$instance_kind" "$proc"; then
            _axon_sup_log "[restart-verified] ${proc} reports '${status}' after ${elapsed}s but IS SERVING its own health endpoint — the supervisor is tracking a refused duplicate, not the live process. Treating as recovered."
            return 0
        fi
        if [[ "$action" == "start" ]]; then
            down_ticks=$(( down_ticks + 1 ))
        else
            down_ticks=0
        fi
        # Require the role to look down for SEVERAL consecutive ticks before spawning.
        # Sending `start` on the FIRST `Completed` races a role that is already coming back
        # up: the spawned duplicate is refused by the IST writer guard, and the supervisor
        # then tracks THAT dead duplicate instead of the live process — poisoning its own
        # bookkeeping (observed: `Completed` reported while /readyz answered 200, which then
        # made the functional test skip). Patience costs ~9 s; eagerness costs a desynced
        # supervisor until the next clean stop/start cycle.
        if [[ "$action" == "start" && "$start_sent" -eq 0 && "$down_ticks" -ge 3 ]]; then
            # The observed process-compose defect: a REQUESTED stop is not a "failure", so
            # `availability.restart: on_failure` never fires. Send the missing half ONCE —
            # resending on every tick would spawn more refused duplicates.
            _axon_sup_log "[restart-verified] ${proc} reported '${status}' for ${down_ticks} consecutive checks — supervisor will not relaunch a requested stop; sending explicit start"
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

# ---------------------------------------------------------------------------
# REQ-AXO-902277 — autonomous indexer recovery after max_restarts exhaustion.
#
# process-compose 1.94.0's `max_restarts` budget is a HARD, TERMINAL ceiling
# that NEVER regenerates (MEASURED, REQ-AXO-902264): after 3 TensorRT-hang exits
# the supervisor abandons the indexer (`Completed`, verdict `exhausted`) and it
# stays dead — ~2 days in the 2026-08-06 incident — until a human runs a full
# stop/start. 902264 delivered the DETECTION; this delivers the CURE: a restart
# budget that REGENERATES with time, owned by Axon, out-of-band of the very
# supervisor that abandoned the role. `axon_restart_role_verified` already sends
# the explicit `POST /process/start` that recovers a `Completed` role and
# verifies it via /readyz, so recovery REUSES it; the new logic here is WHEN to
# fire it: only on the abandoned class, and no more than N times per rolling
# window. The window IS the temporal budget regeneration — after T min of health
# the old restarts age out of the window and the budget is whole again.
# ---------------------------------------------------------------------------

# Rolling-window budget (env-overridable, single source). A poison chunk is
# quarantined by the REQ-AXO-902277 B2 fix after 3 hangs, and transient GPU blips
# clear in a few restarts; a role restarting MORE than this in the window is
# crash-looping on something the healer cannot fix (bad build, host GPU wedge —
# REQ-AXO-902285), so the healer stops hammering and leaves it abandoned +
# VISIBLE (verdict `exhausted`) rather than masking a hard fault behind flaps.
axon_self_heal_window_s()   { printf '%s\n' "${AXON_SELF_HEAL_WINDOW_S:-1800}"; }  # 30 min
axon_self_heal_max()        { printf '%s\n' "${AXON_SELF_HEAL_MAX:-3}"; }
axon_self_heal_state_file() { printf '%s\n' "${1:?project root required}/.axon/self-heal-indexer-restarts.log"; }

# axon_self_heal_should_act <verdict>
# Return 0 iff <verdict> is the ABANDONED class the healer OWNS. Deliberately
# NARROW (advisor guidance — never fight process-compose's fast path):
#   * `exhausted` → not Running, budget spent, supervisor gave up: HEALER'S JOB.
#   * `down`      → not Running, budget LEFT: process-compose is already
#                   restarting it; racing spawns writer-guard-refused duplicates.
#   * `no_budget` → still Running: a restart cannot reset the counter (measured),
#                   so acting would be a pure outage for nothing.
#   * `wedged`    → needs a zombie reap (axon_reap_supervisor_tree), not a plain
#                   restart — a different remediation.
axon_self_heal_should_act() {
    [[ "${1:-}" == "exhausted" ]]
}

# axon_self_heal_window_ok <state_file> <now_epoch> <window_s> <max_in_window>
# Prune restart timestamps older than the window, REWRITE the pruned file (so it
# cannot grow unbounded), and return 0 iff FEWER than <max_in_window> remain (a
# fresh restart is permitted). The prune IS the temporal budget regeneration:
# entries age out on their own, so a role healthy for the whole window starts the
# next one with a full budget. Missing/empty state file = full budget.
axon_self_heal_window_ok() {
    local state_file="${1:?state file required}" now="${2:?now required}"
    local window_s="${3:?window required}" max="${4:?max required}"
    local floor=$(( now - window_s )) line
    local -a kept=()
    if [[ -f "$state_file" ]]; then
        while IFS= read -r line; do
            [[ "$line" =~ ^[0-9]+$ ]] || continue
            (( line >= floor )) && kept+=("$line")
        done < "$state_file"
    fi
    mkdir -p "$(dirname "$state_file")" 2>/dev/null || true
    if (( ${#kept[@]} )); then
        printf '%s\n' "${kept[@]}" > "$state_file" 2>/dev/null || true
    else
        : > "$state_file" 2>/dev/null || true
    fi
    (( ${#kept[@]} < max ))
}

# axon_self_heal_record <state_file> <now_epoch> — append one restart timestamp.
axon_self_heal_record() {
    local state_file="${1:?state file required}" now="${2:?now required}"
    mkdir -p "$(dirname "$state_file")" 2>/dev/null || true
    printf '%s\n' "$now" >> "$state_file"
}

# axon_self_heal_indexer <project_root> <instance_kind> [now_epoch] [restart_budget_s]
#
# The single entry point wired into ensure-axon-running.sh (event-driven: WSL
# boot + MCP-triggered) and its --watch loop (periodic, for the operator-away
# idle case the 2026-08-06 outage happened in). Surveys the indexer role; if it
# is in the abandoned class AND the rolling window permits, restarts it (verified
# via /readyz) and records the restart. A saturated window is a crash-loop the
# healer refuses to mask. The restart is RECORDED before it is attempted, so a
# failing restart still counts against the window (no infinite retry on a broken
# build). Returns: 0 = healthy or recovered, 1 = acted-and-failed / crash-loop
# guard tripped, 2 = no supervisor / no indexer row (caller decides).
axon_self_heal_indexer() {
    local project_root="${1:?project root required}" instance_kind="${2:?instance kind required}"
    local now="${3:-}" budget_s="${4:-180}"
    local survey name v verdict="" state_file window_s max
    [[ -n "$now" ]] || now="$(date +%s)"

    survey="$(axon_role_survey "$project_root" "$instance_kind")" || return 2
    while IFS='|' read -r name _ _ _ _ _ v; do
        [[ "$name" == "axon-indexer" ]] && verdict="$v"
    done <<< "$survey"
    [[ -n "$verdict" ]] || return 2

    axon_self_heal_should_act "$verdict" || return 0   # ok / not our class → nothing to do

    state_file="$(axon_self_heal_state_file "$project_root")"
    window_s="$(axon_self_heal_window_s)"
    max="$(axon_self_heal_max)"

    if ! axon_self_heal_window_ok "$state_file" "$now" "$window_s" "$max"; then
        _axon_sup_warn "[self-heal] axon-indexer '${verdict}' but ${max} restart(s) already in the last $((window_s / 60))min — crash-loop guard: NOT restarting (leaving abandoned + visible; check build / host GPU wedge REQ-AXO-902285)"
        return 1
    fi

    _axon_sup_log "[self-heal] axon-indexer verdict='${verdict}' — supervisor abandoned it (max_restarts spent); autonomous restart within the rolling budget (REQ-AXO-902277)"
    axon_self_heal_record "$state_file" "$now"
    if axon_restart_role_verified "$instance_kind" "axon-indexer" "$budget_s"; then
        _axon_sup_log "[self-heal] axon-indexer recovered"
        return 0
    fi
    _axon_sup_warn "[self-heal] axon-indexer restart did not verify within ${budget_s}s — still abandoned"
    return 1
}

# REQ-AXO-902348/902332 — human interpretation of a process-compose exit code.
# process-compose reports exit_code; -1 is its marker for death by signal. The
# specific signal (SIGSEGV vs SIGKILL) is NOT in the API — it lives in the kernel
# log — so we name the CLASS honestly rather than guess the signal.
axon_exit_code_reason() {
    case "${1:-}" in
        -1)  printf 'death by signal (external kill — OOM killer or a native SIGSEGV, e.g. libnvinfer; the signal itself is only in dmesg)\n' ;;
        75)  printf 'self-exit for supervisor restart (TensorRT B2 hang, REQ-AXO-902033)\n' ;;
        137) printf 'SIGKILL (128+9 — OOM killer or forced kill)\n' ;;
        143) printf 'SIGTERM (128+15 — shutdown signal not handled cleanly)\n' ;;
        *)   printf 'exit code %s\n' "${1:-?}" ;;
    esac
}

# axon_persist_role_exit_events <project_root> <instance_kind>
#
# REQ-AXO-902348/902332 — record WHY each supervised role exited, from the layer
# that SURVIVES the role's death (this runs in the self-heal watcher; the dying
# role cannot write its own SIGKILL). One row per DISTINCT exit — deduped on
# (exit_code, restarts) vs the role's last recorded event — so a role that stays
# Completed does not spam a row every tick. A CLEAN exit (exit_code 0) records
# NOTHING: that is the negative control (a graceful stop must not read as a fault).
# Best-effort and non-fatal: any failure (no supervisor, no psql, PG down) is a
# silent no-op — this is observability, it must never break the heal loop.
# KNOWN BOUND: polling means a crash that restarts between two ticks can be missed.
axon_persist_role_exit_events() {
    local project_root="${1:?project root required}" instance_kind="${2:?instance kind required}"
    local pc_port body now psql dbname

    # AXON_PC_PROCESSES_BODY_OVERRIDE lets a test substitute the /processes body
    # (the function's real input) so the persist+dedup path is falsifiable without
    # a running supervisor — the "a guard whose input isn't substitutable can't be
    # falsified" rule. Unset in production → the real curl poll runs.
    if [[ -n "${AXON_PC_PROCESSES_BODY_OVERRIDE:-}" ]]; then
        body="$AXON_PC_PROCESSES_BODY_OVERRIDE"
    else
        pc_port="$(axon_pc_port_for_instance "$instance_kind")"
        axon_supervisor_healthy "$pc_port" || return 0
        body="$(curl -s -m 8 "http://127.0.0.1:${pc_port}/processes" 2>/dev/null)" || return 0
    fi
    [[ -n "$body" ]] || return 0

    # The detached watcher may not carry the devenv PATH; resolve psql explicitly.
    psql="$(command -v psql 2>/dev/null || true)"
    [[ -z "$psql" && -x "$project_root/.devenv/profile/bin/psql" ]] && psql="$project_root/.devenv/profile/bin/psql"
    [[ -n "$psql" ]] || return 0
    case "$instance_kind" in
        live) dbname="axon_live" ;;
        dev)  dbname="axon_dev" ;;
        *)    dbname="axon_${instance_kind}" ;;
    esac
    local pgport="${PGPORT:-44144}"

    # Wall-clock ms. NOT `date +%s%3N`: on this host's coreutils `%3N` does not
    # truncate to 3 digits, it appends all 9 nanosecond digits (a 19-digit value
    # that overflows to_timestamp downstream). Take the ns epoch and divide.
    now="$(( $(date +%s%N) / 1000000 ))"
    local rows
    rows="$(AXON_PC_JSON="$body" python3 - <<'PY'
import json, os, sys
try:
    body = json.loads(os.environ.get("AXON_PC_JSON", ""))
except Exception:
    sys.exit(0)
procs = body.get("data", body) if isinstance(body, dict) else body
if not isinstance(procs, list):
    sys.exit(0)
for p in procs:
    name = str(p.get("name", "")).strip()
    if not name:
        continue
    try:
        code = int(p.get("exit_code", 0))
    except Exception:
        code = 0
    if code == 0:   # a clean / running role is not an exit event
        continue
    status = str(p.get("status", "")).strip() or "?"
    try:
        restarts = int(p.get("restarts", 0))
    except Exception:
        restarts = 0
    print("|".join([name, str(code), status, str(restarts)]))
PY
)"
    [[ -n "$rows" ]] || return 0

    local role code status restarts last reason esc_role
    while IFS='|' read -r role code status restarts; do
        [[ -n "$role" ]] || continue
        esc_role="${role//\'/\'\'}"
        # Only a NEW exit (differs from the last recorded (code,restarts)) is written.
        last="$("$psql" -h 127.0.0.1 -p "$pgport" -U axon -d "$dbname" -tAXc \
            "SELECT exit_code || '|' || restarts FROM axon.role_exit_event \
             WHERE role='${esc_role}' AND instance_kind='${instance_kind}' \
             ORDER BY observed_ms DESC LIMIT 1" 2>/dev/null | tr -d '[:space:]')"
        [[ "$last" == "${code}|${restarts}" ]] && continue
        reason="$(axon_exit_code_reason "$code")"
        reason="${reason//\'/\'\'}"
        "$psql" -h 127.0.0.1 -p "$pgport" -U axon -d "$dbname" -tAXc \
            "INSERT INTO axon.role_exit_event (role, instance_kind, observed_ms, exit_code, pc_status, restarts, reason) \
             VALUES ('${esc_role}', '${instance_kind}', ${now}, ${code}, '${status//\'/\'\'}', ${restarts}, '${reason}')" \
            >/dev/null 2>&1 || true
        _axon_sup_log "[exit-ledger] ${role} exit_code=${code} restarts=${restarts} — ${reason}"
    done <<< "$rows"
    return 0
}

# axon_recent_role_exits <project_root> <instance_kind> [window_hours]
#
# REQ-AXO-902348/902332 — the READ side: the most recent recorded exit per role
# within the window, for `axon status` to answer WHY a role fell (not merely
# THAT it fell). One line per role, pipe-separated:
#   role|iso_utc|exit_code|reason
# Empty output (no rows / no psql / PG down) is normal and expected on a clean
# runtime — the caller renders nothing. Best-effort, never fails the caller.
axon_recent_role_exits() {
    local project_root="${1:?project root required}" instance_kind="${2:?instance kind required}"
    local window_h="${3:-24}" psql dbname
    psql="$(command -v psql 2>/dev/null || true)"
    [[ -z "$psql" && -x "$project_root/.devenv/profile/bin/psql" ]] && psql="$project_root/.devenv/profile/bin/psql"
    [[ -n "$psql" ]] || return 0
    case "$instance_kind" in
        live) dbname="axon_live" ;;
        dev)  dbname="axon_dev" ;;
        *)    dbname="axon_${instance_kind}" ;;
    esac
    local pgport="${PGPORT:-44144}"
    local floor_ms=$(( ( $(date +%s) - window_h * 3600 ) * 1000 ))
    "$psql" -h 127.0.0.1 -p "$pgport" -U axon -d "$dbname" -tAF'|' -c \
        "SELECT DISTINCT ON (role) role, \
                to_char(to_timestamp(observed_ms/1000.0) AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"'), \
                exit_code, reason \
         FROM axon.role_exit_event \
         WHERE instance_kind='${instance_kind//\'/\'\'}' AND observed_ms >= ${floor_ms} \
         ORDER BY role, observed_ms DESC" 2>/dev/null || true
}
