#!/usr/bin/env bash
# tests/shell/test_role_restart_cleanup.sh — REQ-AXO-902293
#
# The SAFETY NET of the live lifecycle gate: `cleanup()` in test_role_restart_live.sh.
#
# Why this deserves a file
# -----------------------
# On 2026-08-15 the gate correctly detected that the live indexer had not come back
# within its budget — a TRUE finding, the indexer really took 198s. What failed was
# what happened next: the cleanup fired `POST /process/start` into a role whose status
# was `Terminating`, printed "sending start so live is not left without it", and
# exited without ever checking. REQ-AXO-902271 had already established that
# process-compose IGNORES a start in that state, so the message asserted an outcome it
# had not verified, and the live indexer stayed down until a human noticed.
#
# A safety net that reports success it never measured is worse than no safety net: it
# is the reason nobody looks. These cases pin the ORDER, because the order is what was
# missing — settle, then start, then VERIFY, and be explicit when the answer is no.
#
# No runtime, no network: `_axon_role_field`, `_axon_role_serving`, `curl` and `sleep`
# are all stubbed, so every case is a scripted sequence of observed states.
#
# Run: bash tests/shell/test_role_restart_cleanup.sh
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
GATE="$ROOT_DIR/tests/shell/test_role_restart_live.sh"

PASS=0
FAIL=0
pass() { printf '  PASS  %s\n' "$1"; PASS=$(( PASS + 1 )); }
fail() { printf '  FAIL  %s\n' "$1"; FAIL=$(( FAIL + 1 )); }

[[ -f "$GATE" ]] || { echo "missing $GATE" >&2; exit 1; }

# Extract the cleanup function verbatim from the gate, so this test can never drift
# from the code it protects.
cleanup_src="$(awk '/^cleanup\(\) \{/,/^\}/' "$GATE")"
[[ -n "$cleanup_src" ]] || { echo "could not extract cleanup() from $GATE" >&2; exit 1; }

# `run_case <name> <states…>` — the stub returns one status per call, in order, then
# repeats the last. `SERVING_AFTER` = number of _axon_role_serving calls that answer
# "no" before it starts answering "yes" (999 = never recovers).
run_case() {
  local states="$1" serving_after="$2"
  local harness out journal
  harness="$(mktemp)"
  journal="$(mktemp)"
  {
    echo '#!/usr/bin/env bash'
    echo 'set -uo pipefail'
    echo "STATES=($states)"
    echo "SERVING_AFTER=$serving_after"
    # State lives in FILES, not variables. `cleanup` reads the status through
    # `st="$(_axon_role_field …)"` — a command substitution, i.e. a SUBSHELL — so a
    # shell-variable counter increments in a child and is lost to the parent. A first
    # version of this harness did exactly that, which made the "no start while
    # Terminating" assertion unable to observe the real state and therefore unable to
    # fail. Caught by its own negative control: neutralising the fix left it green.
    echo 'STATE_IDX="$(mktemp)"; printf 0 > "$STATE_IDX"'
    echo 'SERVE_IDX="$(mktemp)"; printf 0 > "$SERVE_IDX"'
    echo 'LAST_STATE="$(mktemp)"'
    echo "START_JOURNAL=$journal"
    echo '_axon_role_field() { local i s; i="$(cat "$STATE_IDX")"; s="${STATES[$i]:-${STATES[${#STATES[@]}-1]}}"; printf "%s" $(( i + 1 )) > "$STATE_IDX"; printf "%s" "$s" > "$LAST_STATE"; printf "%s" "$s"; }'
    echo '_axon_role_serving() { local i; i="$(cat "$SERVE_IDX")"; printf "%s" $(( i + 1 )) > "$SERVE_IDX"; [[ $(( i + 1 )) -gt "$SERVING_AFTER" ]]; }'
    # Every start attempt is journalled to a FILE with the status observed at that
    # instant — never to stdout. The real call is `curl … >/dev/null 2>&1`, so a stub
    # that printed would be silenced by the caller and the assertion could never see a
    # start. A first version did print, and its negative control passed while the fix
    # was neutralised: a second vacuous assertion, caught the same way as the first.
    echo 'curl() { printf "START_SENT state=%s\n" "$(cat "$LAST_STATE" 2>/dev/null || echo ?)" >> "$START_JOURNAL"; return 0; }'
    echo 'sleep() { :; }'
    echo 'SAMPLER_PID=""; PC_PORT=8080; PROC=axon-indexer; INSTANCE=live'
    echo 'CLEANUP_SETTLE_S=20; CLEANUP_VERIFY_S=20'
    echo "$cleanup_src"
    echo 'cleanup'
  } > "$harness"
  # The transcript the assertions read = what cleanup SAID + what it actually DID.
  out="$(bash "$harness" 2>&1)"$'\n'"$(cat "$journal" 2>/dev/null)"
  rm -f "$harness" "$journal"
  printf '%s' "$out"
}

# --- 1. already serving: never send a start -----------------------------------------
# The pre-existing guard (a Completed-but-serving duplicate). Kept under test so the
# new waiting logic cannot resurrect the "manufacture the mess it prevents" bug.
out="$(run_case '"Completed" "Completed"' 0)"
if ! grep -q 'START_SENT' <<<"$out"; then
  pass "a role that IS serving gets no start"
else
  fail "start sent to a serving role: $out"
fi

# --- 2. Terminating: wait for it to settle BEFORE starting --------------------------
# The 2026-08-15 case. The start must not be fired while the status is Terminating,
# because process-compose ignores it there.
out="$(run_case '"Terminating" "Terminating" "Completed" "Completed" "Running"' 2)"
if grep -q 'a start is INERT in this state' <<<"$out"; then
  pass "Terminating is recognised as a state where a start is inert"
else
  fail "no wait announced on Terminating: $out"
fi
if grep -q 'START_SENT state=Terminating' <<<"$out"; then
  fail "start fired INTO Terminating — the exact inert call of 2026-08-15: $out"
else
  pass "no start fired while the status is Terminating"
fi

# --- 3. recovery is VERIFIED, not asserted ------------------------------------------
if grep -q '✅ cleanup: axon-indexer is serving again' <<<"$out"; then
  pass "a successful recovery is confirmed by observation, not by having sent a start"
else
  fail "no verified-recovery line: $out"
fi

# --- 4. a failed recovery says so, loudly, with the command -------------------------
# The load-bearing case: before this REQ the transcript was indistinguishable from a
# success, so the operator had no reason to look.
out="$(run_case '"Completed" "Completed" "Completed"' 999)"
if grep -q 'STILL not serving' <<<"$out" && grep -q 'LIVE IS DEGRADED' <<<"$out"; then
  pass "a failed recovery is reported as degraded, not as an attempt made"
else
  fail "a failed recovery does not announce itself: $out"
fi
if grep -q 'process/start/axon-indexer' <<<"$out" && grep -q 'REQ-AXO-902271' <<<"$out"; then
  pass "the failure hands over the exact recovery command and the wedge diagnosis"
else
  fail "no actionable recovery path on failure: $out"
fi

# --- 5. no path claims restoration it did not observe -------------------------------
# The original wording predicted the outcome instead of measuring it. Nothing may
# promise a result before checking it.
#
# Comment lines are excluded, and that exclusion is the point: the first version of
# this assertion fired on the very comment that documents the defect it forbids — the
# THIRD self-matching guard of this session (after REQ-AXO-902260's anti-recidive test
# and REQ-AXO-902327's backtick guard). A guard that reddens on its own prose gets
# deleted, so the rule is now explicit: a source-scanning guard reads CODE, never
# documentation.
offenders="$(grep -n 'so live is not left without it' "$GATE" | grep -vE '^[0-9]+:[[:space:]]*#' || true)"
if [[ -n "$offenders" ]]; then
  fail "the cleanup still promises an outcome it has not verified: $offenders"
else
  pass "no claim of restoration is made before it is observed (comments excluded)"
fi

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
