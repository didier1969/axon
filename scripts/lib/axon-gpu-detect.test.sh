#!/usr/bin/env bash
# REQ-AXO-902163 — tests for detect_gpu: NVML-based, and above all NEVER wedges the
# start (the session-94 incident: a D-state nvidia-smi hung every start/stop/promote).
#
# Run: bash scripts/lib/axon-gpu-detect.test.sh
# Exit 0 on pass, 1 on any failed assertion.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=axon-gpu-detect.sh
source "$SCRIPT_DIR/axon-gpu-detect.sh"

PASS=0
FAIL=0

assert() {
    local desc="$1"
    local cond="$2"
    if eval "$cond"; then
        printf '  PASS  %s\n' "$desc"
        PASS=$(( PASS + 1 ))
    else
        printf '  FAIL  %s  (cond: %s)\n' "$desc" "$cond"
        FAIL=$(( FAIL + 1 ))
    fi
}

# T1 — probe reports available -> detect_gpu returns 0.
export AXON_GPU_PROBE_CMD='printf "{\n  \"available\": true\n}\n"'
if detect_gpu; then r=0; else r=1; fi
assert "available=true -> detect_gpu returns 0" "[ $r -eq 0 ]"

# T2 — probe reports unavailable -> detect_gpu returns 1.
export AXON_GPU_PROBE_CMD='printf "{\n  \"available\": false\n}\n"'
if detect_gpu; then r=0; else r=1; fi
assert "available=false -> detect_gpu returns 1" "[ $r -eq 1 ]"

# T3 — probe HANGS (simulates a wedged GPU driver) -> detect_gpu must return 1 WITHIN
# the deadline instead of blocking. THE core regression guard for the s94 incident.
export AXON_GPU_PROBE_CMD='sleep 30'
export AXON_GPU_PROBE_TIMEOUT_S=1
_start=$SECONDS
if detect_gpu; then r=0; else r=1; fi
_elapsed=$(( SECONDS - _start ))
assert "hung probe -> detect_gpu returns 1 (no GPU, CPU fallback)" "[ $r -eq 1 ]"
assert "hung probe -> returns within ~2s (NON-BLOCKING, never wedges)" "[ $_elapsed -le 3 ]"
unset AXON_GPU_PROBE_CMD AXON_GPU_PROBE_TIMEOUT_S

# T4 — no probe available (no helper, no override) -> returns 1, no error.
r=0
( export PROJECT_ROOT="$(mktemp -d)"; unset AXON_GPU_PROBE_CMD; detect_gpu ) || r=1
assert "no helper -> detect_gpu returns 1" "[ $r -eq 1 ]"

# --- gpu_probe_json: same bounded contract, but it returns the PAYLOAD -------------
# Callers needed a field (memory_used_mb, compute_cap) and were reaching for
# `nvidia-smi` or a bare synchronous `gpu_nvml.py` to get it. Operator rule: NVML only,
# never the CLI — and this file's own doctrine adds the part that matters more: NVML
# talks to the same driver and wedges just as hard, so never wait unboundedly.

# T5 — payload is returned verbatim when the probe answers.
export AXON_GPU_PROBE_CMD='printf "{\"available\":true,\"memory_used_mb\":1234,\"compute_cap\":\"8.6\"}\n"'
_out="$(gpu_probe_json 3 2>/dev/null || true)"
assert "gpu_probe_json returns the payload" "[[ '$_out' == *'\"memory_used_mb\":1234'* ]]"
assert "gpu_probe_json carries compute_cap" "[[ '$_out' == *'8.6'* ]]"
unset AXON_GPU_PROBE_CMD

# T6 — THE regression guard for the bug this function shipped with. `gpu_probe_json` is
# called inside `$(...)`, and command substitution waits for EOF on its pipe — which only
# arrives when EVERY holder closes it, background jobs included. Without `>/dev/null 2>&1`
# on the background subshell, the deadline below is decorative: MEASURED at 90 s for a
# 3 s budget against a genuinely wedged WSL2 GPU channel. The assertion must therefore be
# made through a command substitution, exactly as real callers do — testing the function
# bare would pass while the caller hangs.
export AXON_GPU_PROBE_CMD='sleep 30'
_start=$SECONDS
_out="$(gpu_probe_json 2 2>/dev/null || true)"
_elapsed=$(( SECONDS - _start ))
assert "hung probe -> gpu_probe_json returns within its deadline THROUGH \$( )" "[ $_elapsed -le 4 ]"
assert "hung probe -> gpu_probe_json yields empty (unknown), never a fabricated 0" "[ -z '$_out' ]"
unset AXON_GPU_PROBE_CMD

printf '\ndetect_gpu tests: %d passed, %d failed\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
