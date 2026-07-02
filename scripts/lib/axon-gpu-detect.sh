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
