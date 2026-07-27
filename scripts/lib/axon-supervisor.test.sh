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

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
