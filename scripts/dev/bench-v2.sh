#!/usr/bin/env bash
# REQ-AXO-289 S6a iter 7 — Wrapper for axon-bench-pipeline-v2.
#
# Sets up the ORT / TensorRT + CUDA env so the GpuB2Embedder can dlopen
# its provider libs (same path the embed-bench.sh wrapper uses), then
# invokes the v2 pipeline bench with sustained-throughput defaults aimed
# at the operator north-star ≥250 ch/s.
#
# Usage:
#   scripts/dev/bench-v2.sh [--source PATH] [--max-files N] \
#                           [--duration-secs N] [--warmup-secs N] \
#                           [--gpu|--cpu|--noop] [--csv|--human] [--rebuild]
#
# Wrapper-only flags:
#   --rebuild    force cargo build --release even if the binary is newer
#                than the .rs sources
#
# Defaults (when no flag overrides):
#   --source         = ./src
#   --max-files      = 3000
#   --duration-secs  = 60
#   --warmup-secs    = 10
#   embedder         = --gpu (TensorRT preferred)
#   --human          (use --csv for machine output)
#
# Operator env knobs picked up automatically (export before invoking):
#   AXON_B2_BATCH_SIZE=128       (default 64 — bump for higher GPU saturation)
#   AXON_B2_BATCH_TIMEOUT_MS=200 (default 200)
#   AXON_A1_WORKERS / A2 / A3    (defaults 4/8/2 — env honored verbatim)
#   AXON_B1_WORKERS / B2 / B3    (defaults 4/1/2)
#   AXON_DEV_DATABASE_URL        (required for --gpu / --cpu — bench skips PG under --noop)
#
# Output: CSV mode prints a header + one row to stdout. Human mode prints
# wall + sustained throughput + per-stage backpressure for visual scan.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

# Default forwarded args (overridable by CLI args)
HAVE_SOURCE=0
HAVE_MAX_FILES=0
HAVE_DURATION=0
HAVE_WARMUP=0
HAVE_EMBEDDER=0
HAVE_OUTPUT=0
REBUILD=0
FWD=()

for arg in "$@"; do
    case "$arg" in
        --rebuild)    REBUILD=1 ;;
        --source)     HAVE_SOURCE=1 ; FWD+=("$arg") ;;
        --max-files)  HAVE_MAX_FILES=1 ; FWD+=("$arg") ;;
        --duration-secs) HAVE_DURATION=1 ; FWD+=("$arg") ;;
        --warmup-secs)   HAVE_WARMUP=1 ; FWD+=("$arg") ;;
        --gpu|--cpu|--noop) HAVE_EMBEDDER=1 ; FWD+=("$arg") ;;
        --csv|--human)      HAVE_OUTPUT=1 ; FWD+=("$arg") ;;
        *) FWD+=("$arg") ;;
    esac
done

[[ "$HAVE_SOURCE" == "0" ]] && FWD+=("--source" "$ROOT/src")
[[ "$HAVE_MAX_FILES" == "0" ]] && FWD+=("--max-files" "3000")
[[ "$HAVE_DURATION" == "0" ]] && FWD+=("--duration-secs" "60")
[[ "$HAVE_WARMUP" == "0" ]] && FWD+=("--warmup-secs" "10")
[[ "$HAVE_EMBEDDER" == "0" ]] && FWD+=("--gpu")
[[ "$HAVE_OUTPUT" == "0" ]] && FWD+=("--human")

# Resolve embedder mode for ORT env setup
EMBEDDER_MODE="gpu"
for arg in "${FWD[@]}"; do
    case "$arg" in
        --cpu)  EMBEDDER_MODE="cpu" ;;
        --noop) EMBEDDER_MODE="noop" ;;
        --gpu)  EMBEDDER_MODE="gpu" ;;
    esac
done

# Provider env wiring (mirrors scripts/dev/embed-bench.sh)
if [[ "$EMBEDDER_MODE" != "noop" ]]; then
    if [[ "$EMBEDDER_MODE" == "cpu" ]]; then
        MANIFEST="$ROOT/.axon/ort-artifacts/onnxruntime-cuda/current.json"
        export AXON_GPU_EMBED_SERVICE_TENSORRT=0
    else
        MANIFEST="$ROOT/.axon/ort-artifacts/onnxruntime-tensorrt-cudaPackages/current.json"
        export AXON_GPU_EMBED_SERVICE_TENSORRT=1
    fi
    if [[ ! -f "$MANIFEST" ]]; then
        echo "❌ ORT manifest missing: $MANIFEST" >&2
        echo "   Build it via: bash scripts/build_ort_tensorrt_artifact.sh" >&2
        exit 1
    fi
    CORE_LIB="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get("core_lib",""))' "$MANIFEST")"
    TENSORRT_DIR="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get("tensorrt_lib_dir",""))' "$MANIFEST")"
    if [[ ! -f "$CORE_LIB" ]]; then
        echo "❌ core_lib missing: $CORE_LIB (from $MANIFEST)" >&2
        exit 1
    fi
    ORT_LIB_DIR="$(dirname "$CORE_LIB")"
    NIX_GCC_LIB="$(find /nix/store -maxdepth 1 -name '*-gcc-*-lib' -type d 2>/dev/null | head -1)/lib"
    LDP="$ORT_LIB_DIR"
    [[ -n "$TENSORRT_DIR" && -d "$TENSORRT_DIR" ]] && LDP="$LDP:$TENSORRT_DIR"
    [[ -d /usr/lib/wsl/lib ]] && LDP="$LDP:/usr/lib/wsl/lib"
    [[ -d "$NIX_GCC_LIB" ]] && LDP="$LDP:$NIX_GCC_LIB"
    [[ -n "${LD_LIBRARY_PATH:-}" ]] && LDP="$LDP:$LD_LIBRARY_PATH"
    export ORT_STRATEGY=system
    export ORT_DYLIB_PATH="$CORE_LIB"
    export LD_LIBRARY_PATH="$LDP"
    export AXON_GPU_EMBED_SERVICE_ENABLED="${AXON_GPU_EMBED_SERVICE_ENABLED:-1}"
    export AXON_GPU_TELEMETRY_BACKEND="${AXON_GPU_TELEMETRY_BACKEND:-nvml}"
    export AXON_NVML_LIBRARY_PATH="${AXON_NVML_LIBRARY_PATH:-/usr/lib/wsl/lib/libnvidia-ml.so.1}"

    if [[ -z "${AXON_DEV_DATABASE_URL:-${DATABASE_URL:-}}" ]]; then
        echo "❌ AXON_DEV_DATABASE_URL or DATABASE_URL required for --gpu / --cpu mode." >&2
        echo "   For embedded-backend smoke run: scripts/dev/bench-v2.sh --noop" >&2
        exit 1
    fi
fi

# Auto-rebuild on .rs change
BIN=".axon/cargo-target/release/axon-bench-pipeline-v2"
NEEDS=0
[[ ! -f "$BIN" ]] && NEEDS=1
[[ "$REBUILD" == "1" ]] && NEEDS=1
if [[ "$NEEDS" == "0" ]]; then
    if find src/axon-core/src/pipeline_v2 src/axon-core/src/bin/axon-bench-pipeline-v2.rs \
        -name '*.rs' -newer "$BIN" -print -quit 2>/dev/null | grep -q .; then
        NEEDS=1
    fi
fi
if [[ "$NEEDS" == "1" ]]; then
    echo "🔨 Rebuilding axon-bench-pipeline-v2 (release)..." >&2
    CARGO_TARGET_DIR=.axon/cargo-target cargo build --release \
        --manifest-path src/axon-core/Cargo.toml --bin axon-bench-pipeline-v2 >&2
fi

echo "▶ axon-bench-pipeline-v2 ${FWD[*]}" >&2
echo "  AXON_B2_BATCH_SIZE=${AXON_B2_BATCH_SIZE:-64} AXON_B2_BATCH_TIMEOUT_MS=${AXON_B2_BATCH_TIMEOUT_MS:-200}" >&2
echo "  AXON_A1_WORKERS=${AXON_A1_WORKERS:-4} AXON_A2_WORKERS=${AXON_A2_WORKERS:-8} AXON_A3_WORKERS=${AXON_A3_WORKERS:-2}" >&2
echo "  AXON_B1_WORKERS=${AXON_B1_WORKERS:-4} AXON_B2_WORKERS=${AXON_B2_WORKERS:-1} AXON_B3_WORKERS=${AXON_B3_WORKERS:-2}" >&2
exec "$BIN" "${FWD[@]}"
