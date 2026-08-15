#!/usr/bin/env bash
# scripts/check-host-before-suite.sh — DEC-AXO-901670
#
# Say, BEFORE the full `cargo test --lib` run, whether this host can take it.
#
# Why this exists
# ---------------
# On 2026-08-15 the full suite ran while the live runtime was serving. Measured:
# host load **105**, the suite itself 570s → 1857s (×3 slower), OPV's postgres
# SIGKILLed twice, and the live indexer dead **three times** with SIGSEGV inside
# `libnvinfer` — after which a `dxgvmb` wedge took MCP down for six minutes.
#
# **No promote was involved.** The operator's model was "every promote needs a
# wsl --shutdown"; the measurement says 5 wedges / 215 promotes (2.3%), and that
# evening's wedge had zero. The suite alone did it. WSL2 exposes ONE serialized
# GPU channel, and a saturated host plus a crashing TensorRT is enough to jam it.
#
# This is ADVISORY on purpose. It prints the numbers and returns non-zero when the
# host is loaded; it never blocks a run. A gate that refuses gets bypassed, and a
# bypassed gate teaches everyone to ignore the next one — the repo has paid for
# that lesson more than once. What it buys is that nobody starts a 30-minute suite
# without having SEEN the state of the machine.
#
# Usage:  bash scripts/check-host-before-suite.sh
# Exit:   0 = clear · 1 = loaded, read the lines above before deciding
set -uo pipefail

LOAD_CEILING="${SUITE_LOAD_CEILING:-8}"
verdict=0

printf '\n== host check before the full suite (DEC-AXO-901670) ==\n\n'

# --- 1. load -------------------------------------------------------------------
load1="$(awk '{print $1}' /proc/loadavg)"
cores="$(nproc 2>/dev/null || echo 1)"
printf '  load (1m)          : %s   (ceiling %s, %s cores)\n' "$load1" "$LOAD_CEILING" "$cores"
if awk -v l="$load1" -v c="$LOAD_CEILING" 'BEGIN{exit !(l>c)}'; then
    printf '                       ^ ABOVE ceiling — the suite will be slow AND will slow everything else\n'
    verdict=1
fi

# --- 2. is the live runtime serving? -------------------------------------------
# CONTEXT, not a fault: a serving live runtime is the normal state, and it ran
# beside the suite all day without harm until the load reached 100. Failing on it
# alone would make this script permanently amber — and a check that can never go
# green is one everybody learns to skip (practice #614). What is dangerous is
# load × a live to damage, so only the load, a jammed channel or a concurrent
# build set the verdict.
live_serving=0
if curl -fsS -m 3 "http://127.0.0.1:44129/readyz" >/dev/null 2>&1; then
    live_serving=1
    printf '  live brain         : SERVING — raises the stakes of everything below\n'
else
    printf '  live brain         : not answering — nothing to damage\n'
fi

# --- 3. GPU channel ------------------------------------------------------------
# REQ-AXO-902334 — a thread in D on dxgvmb is the NORMAL state of an indexer that
# is embedding: WSL2 exposes one synchronous GPU channel, so every call spends
# time blocked in it. One sample cannot tell "the channel is jammed" from "the
# GPU is busy" — measured 2026-08-15 on a healthy host: 1 sample in 5 showed a
# D-thread, with a DIFFERENT tid each time, while the first version of this check
# printed "ALREADY jammed" and prescribed `wsl --shutdown`.
#
# What distinguishes a wedge is PERSISTENCE of the SAME tid. The real wedge that
# evening showed the same tids continuously, with the process in `Terminating`
# and /readyz timing out. So: sample N times, require a stable tid, and only then
# name a remedy that closes every Windows session the operator has open.
DXGVMB_SAMPLES="${DXGVMB_SAMPLES:-5}"
DXGVMB_INTERVAL="${DXGVMB_INTERVAL:-1}"
# The probe is behind an overridable command ON PURPOSE. A guard whose input
# cannot be substituted cannot be falsified, and an unfalsifiable guard is the
# one that ships broken — this file already shipped a wrong verdict once. Set
# `DXGVMB_PROBE_CMD` to a stub that prints a fixed tid to prove the jam branch
# still fires; the negative control lives in
# `tests/shell/test_host_check_dxgvmb.sh`.
DXGVMB_PROBE_CMD="${DXGVMB_PROBE_CMD:-}"
dxgvmb_tids() {
    if [[ -n "$DXGVMB_PROBE_CMD" ]]; then
        eval "$DXGVMB_PROBE_CMD"
        return
    fi
    ps -eLo stat,tid,wchan:24 2>/dev/null | awk '$1 ~ /^D/ && $3 ~ /dxgvmb/ {print $2}' | sort -u
}
stable_tids="$(dxgvmb_tids)"
seen_any=0
[[ -n "$stable_tids" ]] && seen_any=1
hits=$([[ -n "$stable_tids" ]] && echo 1 || echo 0)
for _ in $(seq 2 "$DXGVMB_SAMPLES"); do
    sleep "$DXGVMB_INTERVAL"
    now="$(dxgvmb_tids)"
    [[ -n "$now" ]] && { seen_any=1; hits=$((hits + 1)); }
    # Intersection: a tid present in EVERY sample so far.
    stable_tids="$(comm -12 <(printf '%s\n' "$stable_tids") <(printf '%s\n' "$now") 2>/dev/null)"
done
stable_count="$(printf '%s\n' "$stable_tids" | grep -c '[0-9]' || true)"
stable_count="${stable_count:-0}"
printf '  dxgvmb D-threads   : present in %s/%s samples · tid stable across all: %s\n' \
    "$hits" "$DXGVMB_SAMPLES" "$([[ "$stable_count" -gt 0 ]] && echo "YES ($stable_count)" || echo no)"
if [[ "$stable_count" -gt 0 ]]; then
    # Corroborate before naming a remedy this expensive: a jammed channel also
    # leaves the process unable to finish exiting, so it shows as Terminating
    # and/or stops answering /readyz. Persistence alone can still be a long,
    # legitimate GPU call.
    corroborated=0
    curl -fsS -m 3 "http://127.0.0.1:44129/readyz" >/dev/null 2>&1 || corroborated=1
    if curl -fsS -m 3 "http://127.0.0.1:8080/processes" 2>/dev/null | grep -q 'Terminating'; then
        corroborated=1
    fi
    if [[ "$corroborated" -eq 1 ]]; then
        printf '                       ^ GPU channel JAMMED (stable tid + a role Terminating or /readyz mute).\n'
        printf '                         SIGKILL cannot clear a D-state thread; only `wsl --shutdown` can —\n'
        printf '                         that closes every Windows session, so it is the OPERATOR'"'"'s call.\n'
        verdict=1
    else
        printf '                       ^ same tid throughout, but the runtime still answers — a long GPU\n'
        printf '                         call, not a wedge. Re-run if it persists across several minutes.\n'
    fi
elif [[ "$seen_any" -eq 1 ]]; then
    printf '                       (transient — the indexer is embedding; this is the normal state)\n'
fi

# --- 4. recent TensorRT crashes ------------------------------------------------
# Not a blocker by itself: it says the GPU stack is in one of its bad spells.
today="$(date +%Y-%m-%d)"
# `|| true`, NOT `|| echo 0`. `grep -c` and `pgrep -c` already PRINT 0 when they
# match nothing, and then exit non-zero — so `|| echo 0` appends a SECOND zero and
# the value becomes the two-line string "0\n0", which breaks every later `[[ -gt ]]`.
# The repo has paid for this exact idiom once before, on
# `curl -w '%{http_code}' || echo 000` yielding "000000" and classifying every
# sample as up (REQ-AXO-902258). Neutralise the exit status, never the output.
segv="$(grep -c "^${today}.*libnvinfer" /var/log/kern.log 2>/dev/null || true)"
segv="${segv:-0}"
printf '  libnvinfer segfaults today : %s\n' "$segv"
if [[ "$segv" -gt 0 ]]; then
    printf '                       ^ TensorRT is crashing today — the indexer is fragile (REQ-AXO-902332)\n'
fi

# --- 5. concurrent builds ------------------------------------------------------
rustc="$(pgrep -c rustc 2>/dev/null || true)"
rustc="${rustc:-0}"
printf '  concurrent rustc   : %s\n' "$rustc"
[[ "$rustc" -gt 0 ]] && verdict=1

printf '\n'
if [[ "$verdict" -eq 0 ]]; then
    if [[ "$live_serving" -eq 1 ]]; then
        printf '  ✅ host is clear — run the suite (live is serving: keep an eye on it)\n\n'
    else
        printf '  ✅ host is clear — run the suite\n\n'
    fi
else
    printf '  ⚠️  host is loaded. Options, in order of preference:\n'
    printf '     1. wait for it to settle (re-run this script)\n'
    printf '     2. run a SCOPED subset: cargo test --lib -- <module>\n'
    printf '     3. run anyway, knowing the live indexer may die (budget max_restarts is 3, never regenerated)\n\n'
fi
exit "$verdict"
