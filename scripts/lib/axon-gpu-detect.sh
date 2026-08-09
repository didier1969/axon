#!/usr/bin/env bash
# REQ-AXO-902163 (slice S2 of DEC-AXO-901666) — GPU presence detection that CANNOT
# wedge the runtime start.
#
# The old `nvidia-smi -L` CLI probe (and, equally, any NVML ioctl) goes into
# uninterruptible D-state when the WSL2 GPU driver hangs — no `timeout` and no SIGKILL
# frees it, so a blocking probe hangs `start.sh` FOREVER (the session-94 incident:
# every start/stop/promote wedged on `nvidia-smi`). The fix is NOT merely "use NVML
# instead of the CLI" (NVML talks to the same driver and can wedge too) — it is:
# NEVER wait unboundedly on the probe.
#
# This runs the canonical NVML helper (scripts/lib/gpu_nvml.py, REQ-AXO-902085) as a
# BACKGROUND job and polls a completion marker until a hard deadline. On timeout it
# assumes NO GPU (CPU fallback) and ABANDONS the probe (orphaned, reparented to init,
# dies when the driver recovers) rather than blocking startup.
#
# Extracted from start.sh (was inline) so it is unit-testable AND so the coming Rust
# reconciler (DEC-AXO-901666) can absorb one well-defined unit.
#
# NB: sleep-based polling is used DELIBERATELY here (against the usual "wait on the
# pid" rule) precisely because we must NOT wait on a probe that may be in D-state.

# detect_gpu — return 0 if a GPU is present per NVML, 1 otherwise (incl. timeout).
# Requires PROJECT_ROOT. Env:
#   AXON_GPU_PROBE_TIMEOUT_S  hard deadline in seconds (default 4).
#   AXON_GPU_PROBE_CMD        override the probe command (tests only) — must print
#                             JSON containing `"available": true|false`. Default:
#                             `python3 <PROJECT_ROOT>/scripts/lib/gpu_nvml.py`.
detect_gpu() {
    local probe_cmd="${AXON_GPU_PROBE_CMD:-}"
    if [[ -z "$probe_cmd" ]]; then
        local helper="${PROJECT_ROOT:-.}/scripts/lib/gpu_nvml.py"
        [[ -f "$helper" ]] || return 1
        probe_cmd="python3 $helper"
    fi

    local out
    out="$(mktemp)"
    # Background probe. The DONE marker is appended only after the probe returns, so a
    # partial/never-written file is never mistaken for a completed probe.
    ( eval "$probe_cmd" >"$out" 2>/dev/null; printf '\n__AXON_GPU_PROBE_DONE__\n' >>"$out" ) &

    local deadline=$(( SECONDS + ${AXON_GPU_PROBE_TIMEOUT_S:-4} ))
    while (( SECONDS < deadline )); do
        if grep -q '__AXON_GPU_PROBE_DONE__' "$out" 2>/dev/null; then
            if grep -q '"available": true' "$out" 2>/dev/null; then
                rm -f "$out"
                return 0
            fi
            rm -f "$out"
            return 1
        fi
        sleep 0.2
    done

    # Deadline hit → probe slow/wedged → do NOT wait; assume no GPU (CPU fallback).
    rm -f "$out" 2>/dev/null
    return 1
}

# gpu_probe_json [timeout_s] — same BOUNDED probe as `detect_gpu`, but prints the NVML
# JSON payload on stdout (empty string when unavailable, wedged, or past the deadline).
#
# Exists because callers needed a FIELD (`memory_used_mb`, `compute_cap`), not a
# yes/no, and were reaching for `nvidia-smi` or a bare synchronous `gpu_nvml.py` to get
# it. Both are wrong here for the same reason, stated at the top of this file: NVML talks
# to the same driver as the CLI and wedges just as hard, so the rule is not "use NVML" —
# it is NEVER WAIT UNBOUNDEDLY.
#
# Measured on 2026-07-28: a synchronous `python3 gpu_nvml.py` blocked past 120 s while
# four `nvidia-smi` sat in D-state on the WSL2 channel. A sampling loop calling that
# synchronously would add one stuck process per iteration.
#
# Callers must treat an empty result as "unknown", never as zero.
gpu_probe_json() {
    local timeout_s="${1:-${AXON_GPU_PROBE_TIMEOUT_S:-4}}"
    local probe_cmd="${AXON_GPU_PROBE_CMD:-}"
    if [[ -z "$probe_cmd" ]]; then
        local helper="${PROJECT_ROOT:-.}/scripts/lib/gpu_nvml.py"
        [[ -f "$helper" ]] || return 1
        probe_cmd="python3 $helper"
    fi

    local out
    out="$(mktemp)"
    # `>/dev/null 2>&1` on the SUBSHELL is load-bearing, not tidiness. This function is
    # called inside `$(...)`, and command substitution waits for EOF on its pipe — which
    # only arrives when every holder of that descriptor closes it, background jobs
    # included. Without this, the caller blocks on the wedged probe for as long as it
    # takes and the deadline below does nothing at all. MEASURED before the fix: a
    # `gpu_probe_json 3` returned after 90 s. The probe's real output already goes to
    # "$out"; the inherited descriptor carries nothing and must simply be let go.
    ( eval "$probe_cmd" >"$out" 2>/dev/null; printf '\n__AXON_GPU_PROBE_DONE__\n' >>"$out" ) \
        >/dev/null 2>&1 &

    local deadline=$(( SECONDS + timeout_s ))
    while (( SECONDS < deadline )); do
        if grep -q '__AXON_GPU_PROBE_DONE__' "$out" 2>/dev/null; then
            grep -v '__AXON_GPU_PROBE_DONE__' "$out" 2>/dev/null
            rm -f "$out"
            return 0
        fi
        sleep 0.2
    done
    # Wedged: abandon the background job (it is reparented to init and dies when the
    # driver recovers) and report nothing rather than blocking the caller.
    rm -f "$out" 2>/dev/null
    return 1
}

# gpu_wedged_pids [ps_output] — REQ-AXO-902285 / REQ-AXO-902271. Space-separated pids of
# processes in uninterruptible D-state whose command touches the GPU (nvidia-smi /
# axon-indexer). A NON-EMPTY result means the WSL2 GPU virtualisation channel is WEDGED:
# any new GPU teardown/init through that channel — an indexer restart, a promote cutover —
# hangs the same way, so a promote must REFUSE rather than pay a full MCP outage and roll
# back (the 2026-08-09 incident). Unlike `detect_gpu`/`gpu_probe_json` this is a PURE `ps`
# text scan: it NEVER touches the GPU, so it cannot itself wedge (there is nothing to time
# out — the whole point of this file). Pass ps output as $1 for unit tests; defaults to a
# live `ps -eo pid,stat,args`. NB: a SINGLE sample is intentionally noisy — a healthy GPU
# worker samples D transiently; callers that gate on it (promote) confirm across two
# samples (see require_gpu_channel_free), the test-only caller tolerates the noise.
gpu_wedged_pids() {
    local ps_out="${1:-}"
    [[ -n "$ps_out" ]] || ps_out="$(ps -eo pid,stat,args 2>/dev/null)"
    printf '%s\n' "$ps_out" \
        | awk '$2 ~ /^D/ && /nvidia-smi|axon-indexer/ {print $1}' \
        | tr '\n' ' ' \
        | sed 's/  */ /g; s/^ //; s/ *$//'
}
