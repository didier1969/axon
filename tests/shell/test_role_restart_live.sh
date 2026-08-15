#!/usr/bin/env bash
# REQ-AXO-902263 — FUNCTIONAL test of the per-role restart, against the REAL live
# indexer. Run: bash tests/shell/test_role_restart_live.sh
#
# Why this test exists, and why it is functional rather than pure
# ----------------------------------------------------------------
# The lifecycle scripts are ~2 758 lines with, before this file, ZERO functional
# coverage — the two existing shell tests assert on JSON fixtures and on `grep`
# over source text; neither starts a runtime. Every lifecycle defect found in
# session 104 was therefore found by breaking production:
#
#   * `POST /process/restart/axon-indexer` → HTTP 200, then Terminating ~4 min,
#     then Completed with NO new process. The promote's step-6c TIER-1 trusted
#     that 200 and reported a recovery it had not performed.
#   * `axon_resume_live_indexer_after_dev` → `process start … || true`: a silent
#     failure leaves live without an indexer after a dev session.
#   * `shutdown.timeout_seconds: 15` on a TensorRT-holding process → SIGKILL mid
#     CUDA teardown.
#
# None of the three would have survived this file. A pure test could not have
# caught any of them: the logic was fine, the OBSERVED EFFECT was not.
#
# Operator constraint, encoded as assertions (not as caution)
# ----------------------------------------------------------
# "Il est parfaitement autorisé de déposer l'indexeur live durant quelques
#  secondes, voire 2-3 minutes. C'est plus sensible au niveau du brain car tous
#  les LLM l'utilisent potentiellement."
#
# So: dropping the live INDEXER is the test's method, and "the BRAIN never
# flinches" is its hard invariant — sampled throughout, one non-200 fails the run.
# The 180 s budget is the operator's number, not an estimate.
#
# This test NEVER touches the brain: no /mcp call, no embed_provider flip, no
# `stop --hard` (which would reap the live indexer via axonctl).

# Exit codes: 0 = assertions ran and passed · 1 = a real failure · 77 = SKIPPED, nothing
# was measured (autotools convention). 77 matters: a skip reported as success is the same
# lying signal as everything else this REQ removes — `qualify --profile lifecycle` printed
# `pass` on a skipped run before this distinction existed.

set -uo pipefail   # NOT -e: the whole point is to observe failures, not abort on them

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
# shellcheck source=../../scripts/lib/axon-supervisor.sh
source "$ROOT_DIR/scripts/lib/axon-supervisor.sh"
# REQ-AXO-902285 — shared GPU-wedge detector (also gates the promote); DRY, one detector.
source "$ROOT_DIR/scripts/lib/axon-gpu-detect.sh"
# REQ-AXO-902273 — host readiness comes from the shared policy, not from a private
# re-read of /proc. This gate had its own `pgrep -c rustc` + swap snapshot, taken ONLY on
# failure; that combination is what let session 107 time a suite on a host at load 76
# with full swap and read the result as a code regression.
# shellcheck source=../../scripts/lib/axon-resource-policy.sh
source "$ROOT_DIR/scripts/lib/axon-resource-policy.sh"

INSTANCE="${AXON_TEST_INSTANCE:-live}"
PROC="${AXON_TEST_ROLE:-axon-indexer}"
BUDGET_S="${AXON_TEST_RESTART_BUDGET_S:-180}"

# REQ-AXO-902273 — captured BEFORE the restart, and reported on BOTH outcomes. A PASS on
# a saturated host is no more trustworthy than a FAIL: it just happens to be the one
# nobody questions. Advisory only — this gate must never refuse to run.
HOST_AT_START="$(axon_host_measurement_verdict || true)"

PC_PORT="$(axon_pc_port_for_instance "$INSTANCE")"
BRAIN_PORT="$(axon_brain_port_for_instance "$INSTANCE")"

PASS=0; FAIL=0
BRAIN_SAMPLES=""
SAMPLER_PID=""

pass() { printf '  PASS  %s\n' "$1"; PASS=$(( PASS + 1 )); }
fail() { printf '  FAIL  %s\n' "$1"; FAIL=$(( FAIL + 1 )); }
skip() { printf '  SKIP  %s\n' "$1"; }

# --- Safety net: never leave the instance without this role -------------------
# If the test dies anywhere (assertion, ^C, budget), still try to bring the role
# back. A test that can leave live degraded is worse than no test.
cleanup() {
    local rc=$?
    [[ -n "$SAMPLER_PID" ]] && { kill "$SAMPLER_PID" 2>/dev/null; wait "$SAMPLER_PID" 2>/dev/null; }
    local st
    st="$(_axon_role_field "$PC_PORT" "$PROC" status)"
    # REQ-AXO-902263 — ask the ROLE, not only the supervisor. process-compose can report
    # Completed while a healthy instance serves (its status then tracks a duplicate the IST
    # writer guard refused). Firing a start there spawns another doomed process: the cleanup
    # would manufacture the mess it exists to prevent. Observed for real during this test's
    # own development.
    if _axon_role_serving "$INSTANCE" "$PROC"; then
        [[ "$st" != "Running" ]] && printf '  ℹ️  cleanup: %s reports "%s" but IS SERVING — supervisor tracks a refused duplicate; no start sent\n' "$PROC" "$st"
    elif [[ "$st" != "Running" ]]; then
        # REQ-AXO-902293 — this branch used to fire `start` immediately and announce
        # "so live is not left without it", then exit WITHOUT ever checking. On
        # 2026-08-15 it printed exactly that while leaving the live indexer down: the
        # role was "Terminating", and REQ-AXO-902271 already established that
        # process-compose IGNORES a start in that state. The message asserted an
        # outcome it never verified — the same defect class the gate itself exists to
        # catch, sitting in the gate's own safety net.
        #
        # Three things, in order, because the order is what was missing:
        #   1. WAIT for `Terminating` to settle. A start sent into it is inert by
        #      contract, so sending one there is not a recovery attempt, it is noise.
        #   2. start.
        #   3. VERIFY it is serving again, and say so — or say plainly that it is NOT
        #      and hand over the exact command, instead of implying a restoration.
        local settle_budget="${CLEANUP_SETTLE_S:-120}" waited=0
        while [[ "$st" == "Terminating" ]] && (( waited < settle_budget )); do
            [[ "$waited" -eq 0 ]] && printf '  ⏳ cleanup: %s is "Terminating" — a start is INERT in this state (REQ-AXO-902271); waiting up to %ss for it to settle\n' "$PROC" "$settle_budget"
            sleep 5
            waited=$(( waited + 5 ))
            st="$(_axon_role_field "$PC_PORT" "$PROC" status)"
        done

        printf '  ⚠️  cleanup: %s is "%s" and not serving — sending start\n' "$PROC" "${st:-unreachable}"
        curl -s -m 30 -o /dev/null -X POST \
            "http://127.0.0.1:${PC_PORT}/process/start/${PROC}" >/dev/null 2>&1 || true

        local verify_budget="${CLEANUP_VERIFY_S:-60}" elapsed=0 restored=0
        while (( elapsed < verify_budget )); do
            if _axon_role_serving "$INSTANCE" "$PROC"; then restored=1; break; fi
            sleep 5
            elapsed=$(( elapsed + 5 ))
        done
        if (( restored == 1 )); then
            printf '  ✅ cleanup: %s is serving again after %ss — live was NOT left degraded\n' "$PROC" "$elapsed"
        else
            st="$(_axon_role_field "$PC_PORT" "$PROC" status)"
            printf '  ❌ cleanup: %s STILL not serving after %ss (status="%s") — LIVE IS DEGRADED, this gate did it\n' "$PROC" "$elapsed" "${st:-unreachable}"
            printf '     recover with: curl -X POST http://127.0.0.1:%s/process/start/%s\n' "$PC_PORT" "$PROC"
            printf '     if that is inert, the role is wedged: ps -eLo stat,tid,wchan | grep -E "^D" (REQ-AXO-902271)\n'
        fi
    fi
    exit "$rc"
}
trap cleanup EXIT INT TERM

# --- Brain availability sampler ----------------------------------------------
# NOTE the `|| true` placement. A first version of this idiom (in
# promote_live_safe.sh) wrote `code="$(curl … -w '%{http_code}' || echo 000)"`.
# `-w` ALREADY prints 000 on a refused connection, so the fallback appended a
# SECOND one → "000000", which never equalled "000" and classified every sample
# as up: it reported "0 s outage" across a promote that demonstrably restarted the
# brain. Dropping the fallback then exposed curl's non-zero exit to `set -e`,
# killing the sampler at the FIRST outage. `|| true` after the assignment is what
# both needed. (REQ-AXO-902258)
# The timeout is 10 s, NOT 2 s, and that distinction is the point. A first version used
# `-m 2` and reported "3 unreachable samples" during a run where the host was under a
# TensorRT crash-loop with memory pressure — while `/readyz` measured 5-17 ms once calm.
# At 2 s the instrument could not tell "the brain is DOWN" from "the brain is slow under
# load", and collapsing both into `down` is the same failure as every other lying signal
# this REQ is about. 10 s is well beyond what an MCP client tolerates, so a `down` sample
# now means genuinely unserved, and `slow` is recorded separately as information rather
# than as a verdict.
start_brain_sampler() {
    BRAIN_SAMPLES="$(mktemp)"
    (
        while :; do
            out="$(curl -s -m 10 -o /dev/null -w '%{http_code} %{time_total}' \
                "http://127.0.0.1:${BRAIN_PORT}/readyz" 2>/dev/null)" || true
            code="${out%% *}"; elapsed="${out##* }"
            if [[ -z "$code" || "$code" == "000" ]]; then
                printf '%s down -\n' "$(date -u +%s)"
            else
                printf '%s %s %s\n' "$(date -u +%s)" "$code" "$elapsed"
            fi
            sleep 1
        done
    ) >> "$BRAIN_SAMPLES" 2>/dev/null &
    SAMPLER_PID=$!
}

printf 'test_role_restart_live — instance=%s role=%s budget=%ss (REQ-AXO-902263)\n' \
    "$INSTANCE" "$PROC" "$BUDGET_S"

# --- Pre-conditions: SKIP, never fail, when the fixture is absent ------------
if ! axon_supervisor_healthy "$PC_PORT"; then
    skip "no supervisor on :$PC_PORT — instance '$INSTANCE' is not running"
    printf '\n%d passed, %d failed (SKIPPED — nothing was measured)\n' "$PASS" "$FAIL"; exit 77
fi
PRE_STATUS="$(_axon_role_field "$PC_PORT" "$PROC" status)"
PRE_READY="$(_axon_role_field "$PC_PORT" "$PROC" is_ready)"
PRE_PID="$(_axon_role_field "$PC_PORT" "$PROC" pid)"
if [[ "$PRE_STATUS" != "Running" || "$PRE_READY" != "Ready" ]]; then
    skip "$PROC is status='$PRE_STATUS' ready='$PRE_READY' — needs Running+Ready to be a meaningful test"
    printf '\n%d passed, %d failed (SKIPPED — nothing was measured)\n' "$PASS" "$FAIL"; exit 77
fi
if ! axon_brain_healthy "$BRAIN_PORT"; then
    skip "brain not serving on :$BRAIN_PORT — refusing to add load to an already-degraded instance"
    printf '\n%d passed, %d failed (SKIPPED — nothing was measured)\n' "$PASS" "$FAIL"; exit 77
fi
# REQ-AXO-902271 — the GPU virtualisation channel must be free BEFORE we ask the indexer
# to stop. Its teardown releases a TensorRT/CUDA session through that channel; when the
# channel is wedged, the process cannot finish exiting, becomes an unreapable zombie, and
# process-compose reports `Terminating` forever.
#
# Measured, three times on 2026-07-28: this gate failed at ~196 s with the pid UNCHANGED,
# each failure leaving the live indexer down. The cause was never the restart logic — it
# was an `nvidia-smi --query-gpu` from a sibling tool (agent-deck) sitting in uninterruptible
# D-state on `dxgvmb_send_sync_msg`. Running the test then does not measure the restart, it
# measures the wedge, and it manufactures the outage it exists to prevent.
#
# This is a SKIP, not a failure: the release is not broken, the host is momentarily unable
# to answer the question. Loud, because a silent skip is the vacuous green this gate exists
# to remove.
_gpu_wedged_pids="$(gpu_wedged_pids)"  # REQ-AXO-902285 — shared detector (scripts/lib/axon-gpu-detect.sh)
if [[ -n "${_gpu_wedged_pids// /}" ]]; then
    skip "GPU channel WEDGED (pids in uninterruptible D-state: ${_gpu_wedged_pids}) — the indexer cannot complete a TensorRT teardown through a stuck dxg channel. Testing now would measure the wedge and strand the live indexer (REQ-AXO-902271). Re-run when \`ps -eo stat | grep '^D'\` is empty."
    printf '\n%d passed, %d failed (SKIPPED — nothing was measured)\n' "$PASS" "$FAIL"; exit 77
fi
printf '  pre-condition: %s Running+Ready pid=%s · brain :%s serving\n' "$PROC" "$PRE_PID" "$BRAIN_PORT"

start_brain_sampler

# --- The test ----------------------------------------------------------------
T0="$SECONDS"
if axon_restart_role_verified "$INSTANCE" "$PROC" "$BUDGET_S"; then
    pass "axon_restart_role_verified returned 0 within budget"
else
    fail "axon_restart_role_verified did NOT recover $PROC within ${BUDGET_S}s (this is the defect it exists to catch)"
fi
ELAPSED=$(( SECONDS - T0 ))

POST_PID="$(_axon_role_field "$PC_PORT" "$PROC" pid)"
POST_STATUS="$(_axon_role_field "$PC_PORT" "$PROC" status)"
POST_READY="$(_axon_role_field "$PC_PORT" "$PROC" is_ready)"

# THE assertion the HTTP 200 could not make. A same-pid "success" is the exact
# false positive that made TIER-1 inoperative.
if [[ -n "$POST_PID" && "$POST_PID" != "0" && "$POST_PID" != "$PRE_PID" ]]; then
    pass "role runs under a NEW pid ($PRE_PID → $POST_PID) — a real restart, not a reported one"
else
    fail "pid did not change ($PRE_PID → ${POST_PID:-?}): the role was never actually restarted"
fi

# Ground truth first, supervisor second — the same rule the primitive follows. Two ways to
# be genuinely up:
#   * the supervisor agrees (Running + Ready), or
#   * the ROLE answers its own /readyz, which outranks the supervisor's bookkeeping.
# The second case is not a loophole: process-compose's readiness probe has a 5 s initial
# delay and a 5 s period, so right after a restart it legitimately still reads `-` while the
# role already serves. Asserting only on the supervisor failed a run where the role was
# demonstrably healthy — measuring the bookkeeping instead of the service.
if [[ "$POST_STATUS" == "Running" && "$POST_READY" == "Ready" ]]; then
    pass "final observed state: Running + Ready (supervisor agrees)"
elif _axon_role_serving "$INSTANCE" "$PROC"; then
    pass "final state: role SERVES its own /readyz (supervisor still says status='${POST_STATUS:-?}' ready='${POST_READY:-?}' — probe lag or duplicate-tracking)"
else
    fail "final observed state: status='${POST_STATUS:-?}' ready='${POST_READY:-?}' AND not serving"
fi

if (( ELAPSED <= BUDGET_S )); then
    pass "role unavailable ${ELAPSED}s (budget ${BUDGET_S}s — operator allows seconds to 2-3 min) — host at start: ${HOST_AT_START}"
else
    # Report the HOST STATE with the overshoot. This host is shared: a sibling project's
    # pre-push gate (`llmlang/scripts/gate.sh` → `cargo test`) spawns one rustc per test
    # case and reached 29 concurrent during a real promote, dragging this restart to 193 s
    # against the same 180 s budget it met at 37 s and 118 s when the host was idle.
    # Without these two numbers the failure reads as an Axon regression, and the next
    # reader re-derives the provenance from `/tmp/lll-test-*` paths — which is how a
    # contention symptom gets "fixed" by raising the budget.
    # Capture THEN validate — never chain a fallback on the exit status. Both spellings
    # are wrong in opposite directions, and both only misfire in the message that matters
    # most (an overshoot on an otherwise-quiet host):
    #   * `pgrep -c rustc || echo '?'` prints "0" AND exits 1 when nothing matches, so the
    #     fallback fires ON TOP of a correct answer → "0\n?" with an embedded newline.
    #     Same shape as the `84\n0` from axon_count_inotify_instances and the sampler's
    #     `|| echo 000` → "000000" (REQ-AXO-902263 / REQ-AXO-902256).
    #   * `awk … || echo '?'` never fires: awk exits 0 on no match, so the value is EMPTY
    #     rather than '?'.
    _num_or_unknown() { [[ "$1" =~ ^[0-9]+$ ]] && printf '%s' "$1" || printf '?'; }
    _rustc_now="$(_num_or_unknown "$(pgrep -c rustc 2>/dev/null)")"
    _swap_free="$(_num_or_unknown "$(awk '/^SwapFree:/ {print $2}' /proc/meminfo 2>/dev/null)")"
    _swap_total="$(_num_or_unknown "$(awk '/^SwapTotal:/ {print $2}' /proc/meminfo 2>/dev/null)")"
    fail "role unavailable ${ELAPSED}s, over the ${BUDGET_S}s budget — host at start: ${HOST_AT_START}; at that moment: ${_rustc_now} concurrent rustc, swap ${_swap_free}/${_swap_total} kB free. A busy host inflates the GPU teardown; re-run on an idle host before treating this as a regression (and never raise the budget to make it pass — it encodes the operator's 2-3 min constraint)."
fi

# --- The hard invariant: the brain never flinched ----------------------------
kill "$SAMPLER_PID" 2>/dev/null; wait "$SAMPLER_PID" 2>/dev/null; SAMPLER_PID=""
N_SAMPLES="$(wc -l < "$BRAIN_SAMPLES" 2>/dev/null | tr -d ' ')"; N_SAMPLES="${N_SAMPLES:-0}"
N_DOWN="$(awk '$2 == "down"' "$BRAIN_SAMPLES" 2>/dev/null | wc -l | tr -d ' ')"; N_DOWN="${N_DOWN:-0}"
N_BAD="$(awk '$2 != "200" && $2 != "down"' "$BRAIN_SAMPLES" 2>/dev/null | wc -l | tr -d ' ')"; N_BAD="${N_BAD:-0}"
SLOWEST="$(awk '$3 != "-" {if ($3+0 > m) m = $3+0} END {printf "%.2f", m+0}' "$BRAIN_SAMPLES" 2>/dev/null)"

# A silent instrument reads exactly like a green result: refuse to certify the
# invariant on too few samples rather than report a vacuous pass.
if (( N_SAMPLES < 5 )); then
    fail "brain invariant NOT MEASURED — only ${N_SAMPLES} sample(s); treat as UNKNOWN, not as pass"
elif (( N_DOWN == 0 && N_BAD == 0 )); then
    # Latency is reported, never asserted on: a slow brain under load is information for
    # the reader, not a failure. Only "did not answer at all within 10 s" fails.
    pass "brain served all ${N_SAMPLES} samples with 200 (slowest ${SLOWEST}s) — no LLM was interrupted"
else
    fail "brain UNSERVED during the test: ${N_DOWN} no-answer(>10s) + ${N_BAD} non-200 out of ${N_SAMPLES} samples (slowest answered ${SLOWEST}s ; samples kept: $BRAIN_SAMPLES)"
fi

# --- REQ-AXO-902264: every role accounted for, not just the one we restarted ---
# A restart that succeeds while ANOTHER role was silently abandoned is not a green
# lifecycle. The survey is the surface an operator reads; assert on it here so a
# regression in the verdicts is caught by the test rather than in production.
SURVEY="$(axon_role_survey "$ROOT_DIR" "$INSTANCE" 2>/dev/null || true)"
if [[ -z "$SURVEY" ]]; then
    fail "role survey returned nothing while the supervisor is up — the observability surface is blind"
else
    BAD_ROLES="$(printf '%s\n' "$SURVEY" | awk -F'|' '$7 == "exhausted" || $7 == "down" {print $1"("$7")"}' | tr '\n' ' ')"
    if [[ -z "${BAD_ROLES// /}" ]]; then
        pass "role survey: $(printf '%s\n' "$SURVEY" | wc -l | tr -d ' ') role(s), none abandoned"
    else
        fail "role survey reports abandoned role(s): ${BAD_ROLES}"
    fi
fi

# Keep the samples when the invariant failed — the file IS the evidence, and deleting it
# would leave the failure message pointing at a path that no longer exists.
(( FAIL == 0 )) && rm -f "$BRAIN_SAMPLES" 2>/dev/null
printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
