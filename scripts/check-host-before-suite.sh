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
# A thread already stuck in D on dxgvmb means the channel is jammed RIGHT NOW.
# SIGKILL cannot clear it; only `wsl --shutdown` can (operator decision).
wedged="$(ps -eLo stat,wchan:24 2>/dev/null | awk '$1 ~ /^D/ && $2 ~ /dxgvmb/' | wc -l)"
printf '  dxgvmb threads     : %s\n' "$wedged"
if [[ "$wedged" -gt 0 ]]; then
    printf '                       ^ GPU channel ALREADY jammed — do not add load; only `wsl --shutdown` clears it\n'
    verdict=1
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
