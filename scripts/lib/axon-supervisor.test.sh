#!/usr/bin/env bash
# REQ-AXO-902263 — unit tests for the per-role recovery DECISION.
#
# Pure-function tests: no HTTP, no process control, no runtime side effect.
# Run: bash scripts/lib/axon-supervisor.test.sh
#
# Scope note (deliberate): these cover `axon_role_recovery_action` only — the
# logic. They can NOT catch the class of defect that motivated this REQ, because
# that defect lived in the I/O (a supervisor answering HTTP 200 while leaving the
# role down). Session 104 proved the point the hard way: `drive_cutover` had 18
# pure tests and the wrong-binary promote still shipped, because the gap was in
# the executor's interaction with start.sh. The companion FUNCTIONAL test
# (tests/shell/test_role_restart_live.sh) is the one that exercises the real
# supervisor; this file only pins the decision table it relies on.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=axon-supervisor.sh
source "$SCRIPT_DIR/axon-supervisor.sh"

PASS=0
FAIL=0

assert_action() {
    local desc="$1" expected="$2" status="$3" observed_pid="$4" original_pid="$5" got
    got="$(axon_role_recovery_action "$status" "$observed_pid" "$original_pid")"
    if [[ "$got" == "$expected" ]]; then
        printf '  PASS  %s\n' "$desc"; PASS=$(( PASS + 1 ))
    else
        printf '  FAIL  %s  (expected %s, got %s)\n' "$desc" "$expected" "$got"; FAIL=$(( FAIL + 1 ))
    fi
}

printf 'axon_role_recovery_action — REQ-AXO-902263\n'

# THE crux. Right after a restart request the OLD process is still reported
# Running (and often Ready) for several seconds. Treating that as success is
# exactly the false positive that made the promote's TIER-1 inoperative: it
# claimed a recovery it had not performed.
assert_action 'Running on the SAME pid is not recovery — keep waiting' \
    wait Running 111 111

assert_action 'Running on a NEW pid is recovery' \
    done Running 999 111

# The observed process-compose behaviour: a REQUESTED stop is not a "failure",
# so `availability.restart: on_failure` never fires and the role stays down
# until someone sends the missing start. Session 104: status=Completed,
# restarts=1, no new process, live running brain-only.
assert_action 'Completed needs an explicit start (supervisor will not relaunch)' \
    start Completed 0 111
assert_action 'Stopped needs an explicit start' \
    start Stopped 0 111
assert_action 'Disabled needs an explicit start' \
    start Disabled 0 111
assert_action 'Skipped needs an explicit start' \
    start Skipped 0 111

# A GPU-holding process can sit in Terminating for minutes (a tokio worker in
# state D on wchan dxgvmb_send_sync_msg is unkillable, even by SIGKILL). That is
# slow, not failed — the budget decides, not the status.
assert_action 'Terminating is slow, not failed' \
    wait Terminating 111 111
assert_action 'Launching is slow, not failed' \
    wait Launching 0 111
assert_action 'Restarting is slow, not failed' \
    wait Restarting 0 111

# An unreachable supervisor yields an empty status. It must NOT be read as a
# verdict in either direction — neither "recovered" nor "needs a start" (a
# spurious start against a flapping daemon is its own hazard).
assert_action 'unreachable supervisor (empty status) → wait, never a verdict' \
    wait '' '' 111

# Degenerate pids must not be mistaken for a fresh process.
assert_action 'pid 0 while Running is not a new process' \
    wait Running 0 111
assert_action 'empty pid while Running is not a new process' \
    wait Running '' 111

# First-ever start: nothing was running before (original pid 0), so any real pid
# IS the new process.
assert_action 'no previous process (orig 0) → a real pid is recovery' \
    done Running 999 0

printf '\n_axon_role_health_port — the role'"'"'s OWN endpoint, not the supervisor'"'"'s view\n'

assert_port() {
    local desc="$1" expected="$2" kind="$3" proc="$4" got
    got="$(_axon_role_health_port "$kind" "$proc")"
    if [[ "$got" == "$expected" ]]; then
        printf '  PASS  %s\n' "$desc"; PASS=$(( PASS + 1 ))
    else
        printf '  FAIL  %s  (expected "%s", got "%s")\n' "$desc" "$expected" "$got"; FAIL=$(( FAIL + 1 ))
    fi
}

# The ground truth that outranks process-compose bookkeeping. Observed for real: the live
# indexer answered /readyz + /livez with a 3.7 s-fresh heartbeat while the supervisor said
# `Completed`, because earlier `start` calls had spawned duplicates the IST writer guard
# refused ("ownership is already held … owner=…;pid=…"). Without this probe, a caller
# fires yet another doomed start and inflates the restart counter — manufacturing the mess
# it means to clean up.
assert_port 'live indexer health port'  44130 live axon-indexer
assert_port 'dev indexer health port'   44149 dev  axon-indexer

# A role with no health endpoint must yield EMPTY, so `_axon_role_serving` returns
# "unknown" (≠ serving) and the caller falls back to the supervisor rather than assuming
# health. Guessing a port here would be worse than admitting ignorance.
assert_port 'dashboard has no role health port' '' live dashboard
assert_port 'unknown role has no health port'   '' live zzz-nonexistent

printf '\naxon_role_supervision_verdict — REQ-AXO-902264 (giving up must not look like working)\n'

# args: status is_ready has_ready_probe exit_code restarts max_restarts serving [proc_state]
assert_verdict() {
    local desc="$1" expected="$2"; shift 2
    local got; got="$(axon_role_supervision_verdict "$@")"
    if [[ "$got" == "$expected" ]]; then
        printf '  PASS  %s\n' "$desc"; PASS=$(( PASS + 1 ))
    else
        printf '  FAIL  %s  (expected %s, got %s)\n' "$desc" "$expected" "$got"; FAIL=$(( FAIL + 1 ))
    fi
}

# THE case this whole function exists for. `restart: on_failure` + `max_restarts: 3` means
# the supervisor stops trying after the third failure and then does nothing, forever, with
# no signal beyond a log line. Observed in production: the live indexer had been dead for
# hours while `axon-live status` printed HEALTHY.
assert_verdict 'restart budget spent → exhausted, the supervisor will never retry' \
    exhausted Completed - true 1 3 3 no
assert_verdict 'restarts consumed BEYOND the budget is still exhausted' \
    exhausted Error - true 1 5 3 no

# Retries left is a materially different situation: the runtime may still recover on its
# own. Still a FAIL for the operator, but not the same sentence.
assert_verdict 'down with retries left is not exhaustion' \
    down Completed - true 1 1 3 no
assert_verdict 'no restart policy at all (max=0) can never be "exhausted"' \
    down Completed - true 1 0 0 no

# Ground truth outranks the supervisor. Observed for real: `Completed` in process-compose
# while the role answered /readyz, because a duplicate start was refused by the writer
# guard. Reporting that as a dead role sends the operator to restart a healthy process.
assert_verdict 'serving its own health port outranks a Completed verdict' \
    drift Completed - true 1 3 3 yes

# REQ-AXO-902271 — `wedged`: the supervisor never TRIED, as opposed to `exhausted` where
# it tried and gave up. The role's process is an unreapable zombie, so the stop never
# completes and self-healing never starts. Note `restarts=0`: the counter designed to warn
# about abandonment reads perfectly healthy here, which is why the verdict cannot be
# derived from it.
assert_verdict 'Terminating behind a zombie is wedged, not down' \
    wedged Terminating - true 1 0 3 no zombie
# The budget is irrelevant to this verdict and must not be allowed to mask it: a role can
# be wedged with its budget spent too, and the actionable fact is still the wedge (the
# start command that `exhausted` prints is inert while the stop has not completed).
assert_verdict 'wedged outranks exhausted — the recovery differs' \
    wedged Terminating - true 1 3 3 no zombie
# Discriminate on the ZOMBIE, not on the status: an ordinary teardown also passes through
# Terminating, and crying wolf on every clean stop would train people to skip the section.
assert_verdict 'Terminating with a live process is an ordinary teardown' \
    down Terminating - true 1 0 3 no alive
assert_verdict 'Terminating with the process already gone is not wedged' \
    down Terminating - true 1 0 3 no gone
# A caller that cannot inspect the pid must degrade to the old verdict rather than invent
# one — the parameter defaults to unknown for exactly this.
assert_verdict 'Terminating without pid information falls back to down' \
    down Terminating - true 1 0 3 no
# Ground truth still outranks everything: a role answering /readyz is not wedged.
assert_verdict 'a serving role is never reported as wedged' \
    drift Terminating - true 1 0 3 yes zombie

# Configuration, not failure: brain_only does not select the indexer, and
# AXON_DASHBOARD_DISABLED omits the dashboard. Both surface as Disabled.
assert_verdict 'Disabled is a runtime-mode choice, not a failure' \
    disabled Disabled - true 0 0 3 no

# postgres-check: a task, not a service. No readiness probe + exit 0 is the only
# discriminator process-compose offers, and mis-classifying it would print a permanent
# FAIL on a perfectly healthy runtime — the fastest way to train everyone to ignore this
# section.
assert_verdict 'a probe-less process that exited 0 is a completed task' \
    oneshot Completed - false 0 0 0 -
assert_verdict 'a probe-less process that exited NON-zero is still a failure' \
    down Completed - false 1 0 0 -

# Nominal.
assert_verdict 'Running + Ready is ok'          ok Running Ready true 0 0 3 -
assert_verdict 'Running without a probe is ok'  ok Running -     false 0 0 0 -

# MEASURED on process-compose 1.94.0 in an isolated probe: `restarts` never decreases —
# not after a healthy period, and not after the explicit `POST /process/start` used to
# recover. The role returns Running with the counter still at the ceiling, so the next
# failure is terminal and unannounced. Running-and-doomed must not print like Running.
assert_verdict 'Running with the restart budget already spent is NOT plain ok' \
    no_budget Running Ready true 3 3 3 -
assert_verdict 'Running with budget left is ok' \
    ok Running Ready true 0 2 3 -
assert_verdict 'no restart policy (max=0) never reports a spent budget' \
    ok Running Ready true 7 0 0 -
# Ordering: not-ready outranks the budget question. A role that is not serving is the more
# urgent fact, and reporting "no safety net" about a process that is already not working
# would bury it.
assert_verdict 'not_ready wins over no_budget' \
    not_ready Running 'Not Ready' true 3 3 3 -
# Running but not ready: the indexer spends minutes loading the GPU model at boot. Named
# distinctly so the renderer can warn without declaring the runtime degraded.
assert_verdict 'Running but not ready is its own verdict, not a failure' \
    not_ready Running 'Not Ready' true 0 0 3 -

# Transient supervisor states must never be read as abandonment.
assert_verdict 'Terminating is down, not exhausted, when the budget is untouched' \
    down Terminating - true 0 0 3 no

# Malformed counters must not crash an arithmetic comparison inside `axon status`.
assert_verdict 'non-numeric counters degrade to 0 instead of erroring' \
    down Completed - true 1 '' 'n/a' no

printf '\naxon_self_heal_should_act — REQ-AXO-902277 (act ONLY on the abandoned class)\n'

assert_should_act() {
    local desc="$1" expected="$2" verdict="$3" rc
    if axon_self_heal_should_act "$verdict"; then rc=0; else rc=$?; fi
    if [[ "$rc" == "$expected" ]]; then
        printf '  PASS  %s\n' "$desc"; PASS=$(( PASS + 1 ))
    else
        printf '  FAIL  %s  (expected rc %s, got %s)\n' "$desc" "$expected" "$rc"; FAIL=$(( FAIL + 1 ))
    fi
}

# `exhausted` is the ONLY verdict the healer owns: not Running, budget spent, the
# supervisor gave up. Everything else is someone else's job or a pure outage.
assert_should_act 'exhausted → the healer restarts it'                         0 exhausted
# process-compose still has budget and is already restarting a plain `down`;
# racing it spawns writer-guard-refused duplicates that poison the bookkeeping.
assert_should_act 'down (budget left) belongs to process-compose'              1 down
# Still Running — a restart cannot reset the counter (measured 902264), so acting
# would drop a serving indexer for nothing.
assert_should_act 'no_budget is Running — restart resets nothing, do not act'   1 no_budget
# A zombie behind Terminating needs a reap, not a start.
assert_should_act 'wedged needs a zombie reap, not a restart'                   1 wedged
assert_should_act 'ok never acts'                                              1 ok
assert_should_act 'not_ready never acts'                                       1 not_ready
assert_should_act 'disabled (config) never acts'                              1 disabled
assert_should_act 'drift (serving) never acts'                                1 drift
assert_should_act 'oneshot never acts'                                        1 oneshot
assert_should_act 'empty verdict never acts'                                  1 ''

printf '\naxon_self_heal_window_ok — REQ-AXO-902277 (temporal budget regeneration)\n'

SELF_HEAL_WORK="$(mktemp -d)"
trap 'rm -rf "$SELF_HEAL_WORK"' EXIT

assert_window() {
    local desc="$1" expected="$2"; shift 2
    local rc
    if axon_self_heal_window_ok "$@"; then rc=0; else rc=$?; fi
    if [[ "$rc" == "$expected" ]]; then
        printf '  PASS  %s\n' "$desc"; PASS=$(( PASS + 1 ))
    else
        printf '  FAIL  %s  (expected rc %s, got %s)\n' "$desc" "$expected" "$rc"; FAIL=$(( FAIL + 1 ))
    fi
}

# now=10000, window=1800 → floor=8200; max=3.
assert_window 'missing state file = full budget → restart permitted' \
    0 "$SELF_HEAL_WORK/absent.log" 10000 1800 3

sf="$SELF_HEAL_WORK/two.log"; printf '9900\n9950\n' > "$sf"
assert_window 'two restarts in window (< max 3) → still permitted' \
    0 "$sf" 10000 1800 3

sf="$SELF_HEAL_WORK/three.log"; printf '9800\n9900\n9950\n' > "$sf"
assert_window 'three restarts in window (= max 3) → crash-loop guard blocks' \
    1 "$sf" 10000 1800 3

sf="$SELF_HEAL_WORK/old.log"; printf '1000\n2000\n8000\n' > "$sf"
assert_window 'all restarts aged out of the window → budget regenerated' \
    0 "$sf" 10000 1800 3
if [[ ! -s "$sf" ]]; then
    printf '  PASS  %s\n' 'prune rewrites the state file (aged entries dropped)'; PASS=$(( PASS + 1 ))
else
    printf '  FAIL  prune did not clear aged entries (file: %s)\n' "$(tr '\n' ' ' < "$sf")"; FAIL=$(( FAIL + 1 ))
fi

sf="$SELF_HEAL_WORK/mixed.log"; printf '1000\n8000\n9900\n9950\n' > "$sf"
assert_window 'mixed window keeps only in-window entries (2 < max) → permitted' \
    0 "$sf" 10000 1800 3
kept="$(wc -l < "$sf" | tr -d '[:space:]')"
if [[ "$kept" == "2" ]]; then
    printf '  PASS  %s\n' 'prune keeps exactly the in-window entries (2)'; PASS=$(( PASS + 1 ))
else
    printf '  FAIL  prune kept %s entries, expected 2\n' "$kept"; FAIL=$(( FAIL + 1 ))
fi

sf="$SELF_HEAL_WORK/garbage.log"; printf 'not-a-number\n\n9950\n' > "$sf"
assert_window 'malformed lines are ignored (one valid recent entry → permitted)' \
    0 "$sf" 10000 1800 3

printf '\naxon_self_heal_record — REQ-AXO-902277\n'
sf="$SELF_HEAL_WORK/rec.log"
axon_self_heal_record "$sf" 12345
axon_self_heal_record "$sf" 12346
recn="$(wc -l < "$sf" | tr -d '[:space:]')"
if [[ "$recn" == "2" ]]; then
    printf '  PASS  %s\n' 'record appends one line per restart'; PASS=$(( PASS + 1 ))
else
    printf '  FAIL  record wrote %s lines, expected 2\n' "$recn"; FAIL=$(( FAIL + 1 ))
fi

printf '\naxon_self_heal_indexer — REQ-AXO-902277 wiring (stubbed survey + restart)\n'
# Stub the two I/O calls the orchestrator delegates to, so the
# decide → record → delegate wiring is exercised without a live supervisor. The
# companion FUNCTIONAL test (tests/shell/test_role_restart_live.sh) exercises the
# real supervisor; this pins the decision the healer relies on.
STUB_VERDICT="exhausted"
STUB_RESTART_RC=0
RESTART_CALLS=0
axon_role_survey() {
    printf 'axon-brain|Running|Ready|0|3|-|ok\naxon-indexer|Completed|-|3|3|no|%s\ndashboard|Running|Ready|0|3|-|ok\n' "$STUB_VERDICT"
}
axon_restart_role_verified() { RESTART_CALLS=$(( RESTART_CALLS + 1 )); return "$STUB_RESTART_RC"; }
export AXON_SELF_HEAL_WINDOW_S=1800 AXON_SELF_HEAL_MAX=3

assert_orch() {
    local desc="$1" want_rc="$2" want_calls="$3" want_records="$4"
    local root="$5" verdict="$6" restart_rc="$7" now="$8"
    STUB_VERDICT="$verdict"; STUB_RESTART_RC="$restart_rc"; RESTART_CALLS=0
    local rc
    if axon_self_heal_indexer "$root" live "$now" 5; then rc=0; else rc=$?; fi
    local sfile="$root/.axon/self-heal-indexer-restarts.log" records=0
    [[ -f "$sfile" ]] && records="$(grep -c . "$sfile" 2>/dev/null || echo 0)"
    if [[ "$rc" == "$want_rc" && "$RESTART_CALLS" == "$want_calls" && "$records" == "$want_records" ]]; then
        printf '  PASS  %s\n' "$desc"; PASS=$(( PASS + 1 ))
    else
        printf '  FAIL  %s  (rc=%s/%s calls=%s/%s records=%s/%s)\n' \
            "$desc" "$rc" "$want_rc" "$RESTART_CALLS" "$want_calls" "$records" "$want_records"; FAIL=$(( FAIL + 1 ))
    fi
}

R1="$SELF_HEAL_WORK/root_ok"; mkdir -p "$R1/.axon"
assert_orch 'ok indexer → no restart, no record, rc 0' 0 0 0 "$R1" ok 0 10000

R2="$SELF_HEAL_WORK/root_heal"; mkdir -p "$R2/.axon"
assert_orch 'exhausted + fresh window → restart once, record once, rc 0' 0 1 1 "$R2" exhausted 0 10000

R3="$SELF_HEAL_WORK/root_saturated"; mkdir -p "$R3/.axon"
printf '9800\n9900\n9950\n' > "$R3/.axon/self-heal-indexer-restarts.log"
assert_orch 'exhausted + saturated window → crash-loop guard, no restart, rc 1' 1 0 3 "$R3" exhausted 0 10000

R4="$SELF_HEAL_WORK/root_failrestart"; mkdir -p "$R4/.axon"
assert_orch 'exhausted + restart fails → recorded (counts vs window), rc 1' 1 1 1 "$R4" exhausted 1 10000

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
