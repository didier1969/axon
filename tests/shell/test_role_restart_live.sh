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

INSTANCE="${AXON_TEST_INSTANCE:-live}"
PROC="${AXON_TEST_ROLE:-axon-indexer}"
BUDGET_S="${AXON_TEST_RESTART_BUDGET_S:-180}"

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
        printf '  ⚠️  cleanup: %s is "%s" and not serving — sending start so live is not left without it\n' "$PROC" "${st:-unreachable}"
        curl -s -m 30 -o /dev/null -X POST \
            "http://127.0.0.1:${PC_PORT}/process/start/${PROC}" >/dev/null 2>&1 || true
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

if [[ "$POST_STATUS" == "Running" && "$POST_READY" == "Ready" ]]; then
    pass "final observed state: Running + Ready"
else
    fail "final observed state: status='${POST_STATUS:-?}' ready='${POST_READY:-?}'"
fi

if (( ELAPSED <= BUDGET_S )); then
    pass "role unavailable ${ELAPSED}s (budget ${BUDGET_S}s — operator allows seconds to 2-3 min)"
else
    fail "role unavailable ${ELAPSED}s, over the ${BUDGET_S}s budget"
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

# Keep the samples when the invariant failed — the file IS the evidence, and deleting it
# would leave the failure message pointing at a path that no longer exists.
(( FAIL == 0 )) && rm -f "$BRAIN_SAMPLES" 2>/dev/null
printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
