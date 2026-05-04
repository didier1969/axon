#!/usr/bin/env bash
# REQ-AXO-176 — Wrapper for the embedder-bench binary.
#
# Resolves the active ORT artifact manifest (TensorRT preferred per
# operator decision 2026-05-03 over CUDA EP), exports the env so
# libonnxruntime + provider libs are dlopen-able, then invokes the
# release binary.
#
# Usage:
#   scripts/dev/embed-bench.sh [--n N] [--source PATH] [--no-force-gpu]
#                              [--label L] [--csv|--human] [--cpu]
#                              [--rebuild]
#
# Flags forwarded to the binary unchanged unless listed below.
# Wrapper-only flags:
#   --cpu        sets AXON_GPU_EMBED_SERVICE_TENSORRT=0 + --no-force-gpu
#                (CPU EP via ORT — useful as a baseline)
#   --rebuild    force `cargo build --release --bin embedder-bench` even
#                if the binary exists and is newer than the .rs sources
#
# Cycle target: ~5-10s per run after first release build.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

CPU_MODE=0
REBUILD=0
FWD=()
for arg in "$@"; do
    case "$arg" in
        --cpu) CPU_MODE=1 ;;
        --rebuild) REBUILD=1 ;;
        *) FWD+=("$arg") ;;
    esac
done

# Pick manifest: TensorRT (default) OR --cpu
if [[ "$CPU_MODE" == "1" ]]; then
    MANIFEST="$ROOT/.axon/ort-artifacts/onnxruntime-cuda/current.json"
    export AXON_GPU_EMBED_SERVICE_TENSORRT=0
    FWD+=("--no-force-gpu")
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

# Build LD_LIBRARY_PATH: ORT lib dir + TensorRT lib dir + WSL CUDA + nix gcc-cc.lib + existing
NIX_GCC_LIB="$(find /nix/store -maxdepth 1 -name '*-gcc-*-lib' -type d 2>/dev/null | head -1)/lib"
LDP="$ORT_LIB_DIR"
[[ -n "$TENSORRT_DIR" && -d "$TENSORRT_DIR" ]] && LDP="$LDP:$TENSORRT_DIR"
[[ -d /usr/lib/wsl/lib ]] && LDP="$LDP:/usr/lib/wsl/lib"
[[ -d "$NIX_GCC_LIB" ]] && LDP="$LDP:$NIX_GCC_LIB"
[[ -n "${LD_LIBRARY_PATH:-}" ]] && LDP="$LDP:$LD_LIBRARY_PATH"

export ORT_STRATEGY=system
export ORT_DYLIB_PATH="$CORE_LIB"
export LD_LIBRARY_PATH="$LDP"
# Mirror the runtime_boot defaults so the bench sees the same env the
# real subprocess would see — inflight tuning happens via these.
export AXON_GPU_EMBED_SERVICE_ENABLED="${AXON_GPU_EMBED_SERVICE_ENABLED:-1}"
# REQ-AXO-176 — operator note 2026-05-04: nvidia-smi VRAM readings are
# unreliable; switch to NVML for any VRAM-aware decision. When VRAM
# exceeds the 8 GB GPU limit (treat 7 GB as safe ceiling) the driver
# spills allocations to system RAM and throughput collapses.
export AXON_GPU_TELEMETRY_BACKEND="${AXON_GPU_TELEMETRY_BACKEND:-nvml}"
export AXON_NVML_LIBRARY_PATH="${AXON_NVML_LIBRARY_PATH:-/usr/lib/wsl/lib/libnvidia-ml.so.1}"

# Auto-rebuild if any embedder .rs is newer than the binary, or --rebuild
BIN=".axon/cargo-target/release/embedder-bench"
NEEDS=0
if [[ ! -f "$BIN" ]]; then NEEDS=1; fi
if [[ "$REBUILD" == "1" ]]; then NEEDS=1; fi
if [[ "$NEEDS" == "0" ]]; then
    if find src/axon-core/src/embedder src/axon-core/src/embedder.rs src/axon-core/src/bin/embedder-bench.rs \
        -name '*.rs' -newer "$BIN" -print -quit 2>/dev/null | grep -q .; then
        NEEDS=1
    fi
fi
if [[ "$NEEDS" == "1" ]]; then
    echo "🔨 Rebuilding embedder-bench (release)..."
    CARGO_TARGET_DIR=.axon/cargo-target cargo build --release \
        --manifest-path src/axon-core/Cargo.toml --bin embedder-bench >&2
fi

# REQ-AXO-176 — execute inside `devenv shell` so the Nix gcc-cc.lib's
# libstdc++.so.6 is on LD_LIBRARY_PATH BEFORE axon-core/ort dlopens
# libonnxruntime.so. Without this, the system /lib/x86_64-linux-gnu's
# libstdc++ (loaded by the binary itself) lacks the GLIBCXX symbol
# version that the Nix-built libonnxruntime requires → dlopen fails.
# This mirrors how scripts/start.sh:910 wraps the runtime invocation.
ARG_QUOTED=""
for a in "${FWD[@]}"; do
    ARG_QUOTED="$ARG_QUOTED $(printf '%q' "$a")"
done

exec devenv shell --no-reload --no-tui -- bash -lc \
    "export ORT_STRATEGY='$ORT_STRATEGY'; \
     export ORT_DYLIB_PATH='$ORT_DYLIB_PATH'; \
     export LD_LIBRARY_PATH=\"$LDP:\$LD_LIBRARY_PATH\"; \
     export AXON_GPU_EMBED_SERVICE_TENSORRT='$AXON_GPU_EMBED_SERVICE_TENSORRT'; \
     export AXON_GPU_EMBED_SERVICE_ENABLED='$AXON_GPU_EMBED_SERVICE_ENABLED'; \
     '$ROOT/$BIN'$ARG_QUOTED"
