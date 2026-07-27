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

# args: status is_ready has_ready_probe exit_code restarts max_restarts serving
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

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
