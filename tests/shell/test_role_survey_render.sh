#!/usr/bin/env bash
# tests/shell/test_role_survey_render.sh — REQ-AXO-902264
#
# Fixture tests for the survey renderer that `axon status` prints. No runtime, no
# supervisor, no network: every case is a synthetic survey row.
#
# What this protects, and why it is worth a file of its own
# --------------------------------------------------------
# These lines are the ones an operator READS AND OBEYS when a role has been abandoned —
# they carry the recovery commands. Two properties must hold and neither is self-evident
# from the code:
#   1. an abandoned role DEGRADES the runtime (exit 2 → `STATUS DEGRADED`, non-zero
#      `axon status`), because the whole defect being fixed is that giving up looked
#      exactly like working;
#   2. a healthy runtime stays green — a section that cries wolf on `postgres-check`
#      (a completed one-shot task) or on a booting indexer trains everyone to skip it,
#      which is the same blindness by another route.
#
# Run: bash tests/shell/test_role_survey_render.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
RENDER="$ROOT_DIR/scripts/lib/axon-role-survey-render.py"

PASS=0
FAIL=0
pass() { printf '  PASS  %s\n' "$1"; PASS=$(( PASS + 1 )); }
fail() { printf '  FAIL  %s\n' "$1"; FAIL=$(( FAIL + 1 )); }

# run <survey-rows> → sets OUT and RC
run() {
    OUT="$(printf '%s\n' "$1" | AXON_PC_PORT=8080 AXON_INSTANCE_KIND=live python3 "$RENDER")" \
        && RC=0 || RC=$?
}

# assert_rc <desc> <expected_rc> <rows>
assert_rc() {
    local desc="$1" expected="$2" rows="$3"
    run "$rows"
    if [[ "$RC" == "$expected" ]]; then pass "$desc"; else fail "$desc (expected rc=$expected, got rc=$RC)"; fi
}

# assert_contains <desc> <needle> <rows>
assert_contains() {
    local desc="$1" needle="$2" rows="$3"
    run "$rows"
    if [[ "$OUT" == *"$needle"* ]]; then pass "$desc"; else fail "$desc (missing: $needle ; got: $OUT)"; fi
}

# assert_lacks <desc> <needle> <rows>
assert_lacks() {
    local desc="$1" needle="$2" rows="$3"
    run "$rows"
    if [[ "$OUT" != *"$needle"* ]]; then pass "$desc"; else fail "$desc (unexpectedly present: $needle)"; fi
}

printf 'axon-role-survey-render — REQ-AXO-902264\n'

HEALTHY='axon-brain|Running|Ready|0|3|-|ok
axon-indexer|Running|Ready|0|3|-|ok
dashboard|Running|Ready|0|3|-|ok
postgres-check|Completed|-|0|0|-|oneshot'

# --- Property 2 first: the healthy runtime must stay green -------------------
assert_rc 'a fully healthy runtime does not degrade' 0 "$HEALTHY"
assert_lacks 'no FAIL line on a healthy runtime' 'FAIL' "$HEALTHY"
# postgres-check is a task that exits 0; flagging it would put a permanent FAIL on every
# healthy runtime — the fastest way to teach everyone to ignore this section.
assert_contains 'a completed one-shot task reads as OK' 'OK      postgres-check' "$HEALTHY"

# --- Property 1: abandonment is loud and degrades ----------------------------
EXHAUSTED='axon-brain|Running|Ready|0|3|-|ok
axon-indexer|Completed|-|3|3|no|exhausted'
assert_rc 'an abandoned role degrades the runtime (exit 2)' 2 "$EXHAUSTED"
assert_contains 'exhaustion says the supervisor will never retry' 'NEVER' "$EXHAUSTED"
assert_contains 'exhaustion carries the immediate recovery command' \
    'curl -X POST http://127.0.0.1:8080/process/start/axon-indexer' "$EXHAUSTED"
# The measured trap: that curl brings the role back but leaves the counter at the ceiling.
# An operator who runs only it believes they are covered and is not.
assert_contains 'exhaustion warns the recovery does NOT restore the budget' \
    'does NOT give the budget back' "$EXHAUSTED"

DOWN='axon-indexer|Completed|-|1|3|no|down'
assert_rc 'a role that is down (retries left) still degrades' 2 "$DOWN"
assert_contains 'down states how much budget is left' '1/3' "$DOWN"

# --- The Running-but-doomed state -------------------------------------------
# Measured: the counter never resets, so a role can serve with zero retries left. It is
# not a failure yet, which is exactly why it must be said before the next crash.
NOBUDGET='axon-indexer|Running|Ready|3|3|-|no_budget'
assert_rc 'a spent budget warns but does not degrade a working runtime' 0 "$NOBUDGET"
assert_contains 'spent budget is named as such' 'budget is SPENT' "$NOBUDGET"
assert_contains 'spent budget points at the only real fix' './scripts/axon --instance live stop' "$NOBUDGET"
assert_contains 'spent budget flags the brain cost of that fix' 'interrupts the brain' "$NOBUDGET"

# --- Wedged: dead with a FULL tank (REQ-AXO-902271) --------------------------
# The failure mode `exhausted` does not cover. Observed three times on 2026-07-28 with the
# host verifiably idle: the role's process is an unreapable zombie, so from the
# supervisor's point of view the stop never finishes and self-healing never STARTS.
# `restarts=0` — the counter that is supposed to warn us reads perfectly healthy.
WEDGED='axon-brain|Running|Ready|0|3|-|ok
axon-indexer|Terminating|-|0|3|no|wedged'
assert_rc 'a wedged role degrades the runtime (exit 2)' 2 "$WEDGED"
assert_contains 'wedged is named, not folded into "down"' 'WEDGED' "$WEDGED"
# The whole point of a separate verdict: `down` would print a start command, and that
# command is INERT here (the supervisor ignores it while it believes the role is still
# terminating). Printing an inert command is the same class of defect as printing HEALTHY
# for a dead role, so the line must say so out loud.
assert_contains 'wedged states that a start command will not work' 'will NOT work' "$WEDGED"
assert_contains 'wedged gives the diagnostic for the real blocker' 'D-state' "$WEDGED"
assert_contains 'wedged says the budget is intact, not spent' '0/3' "$WEDGED"
# `wsl --shutdown` closes every session the operator has open. It may be named as the
# forced way out; it must never read as the recommended first move.
assert_contains 'wedged marks the forced cure as an operator decision' 'operator decision' "$WEDGED"

# --- States that must NOT degrade -------------------------------------------
# The indexer spends minutes loading its GPU model at boot; failing there would make
# `axon status` red on every normal start.
assert_rc 'a booting role (not ready) does not degrade' 0 'axon-indexer|Running|Not Ready|0|3|-|not_ready'
# brain_only does not select the indexer; AXON_DASHBOARD_DISABLED omits the dashboard.
assert_rc 'a role disabled by runtime mode does not degrade' 0 'axon-indexer|Disabled|-|0|3|no|disabled'
# Ground truth outranks bookkeeping: the role answers its own /readyz, so the runtime
# works. Sending the operator to restart a healthy process would manufacture an outage.
assert_rc 'supervisor drift warns without degrading' 0 'axon-indexer|Completed|-|0|3|yes|drift'
assert_contains 'drift explains what is actually broken' 'stale bookkeeping' \
    'axon-indexer|Completed|-|0|3|yes|drift'

# --- Input robustness: status must never crash on a malformed survey --------
assert_rc 'empty input is reported as "nothing to render", not as a crash' 1 ''
assert_rc 'a truncated row is skipped, and an empty render is rc=1' 1 'axon-indexer|Running'
assert_contains 'a truncated row does not suppress the valid ones' 'OK      axon-brain' \
    'axon-indexer|Running
axon-brain|Running|Ready|0|3|-|ok'

# An unknown verdict from a future version must fail loudly rather than vanish: silence is
# the failure mode this whole REQ exists to remove.
assert_rc 'an unknown verdict degrades rather than disappearing' 2 \
    'axon-indexer|Completed|-|0|3|no|some_future_verdict'

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
