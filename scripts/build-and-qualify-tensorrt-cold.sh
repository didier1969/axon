#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

duration="120"
interval="5"
label="tensorrt-hard-cut"
cuda_package_set="${AXON_CUDA_PACKAGE_SET:-cudaPackages}"
cuda_package_label="${cuda_package_set//_/-}"
default_tarball="$PROJECT_ROOT/.axon/downloads/TensorRT-10.14.1.48.Linux.x86_64-gnu.cuda-12.9.tar.gz"
tarball_path="${TENSORRT_LOCAL_TARBALL:-$default_tarball}"
manifest_path="${AXON_ORT_ARTIFACT_MANIFEST:-$PROJECT_ROOT/.axon/ort-artifacts/onnxruntime-tensorrt-${cuda_package_label}/current.json}"
failure_path="$PROJECT_ROOT/.axon/ort-artifacts/onnxruntime-tensorrt-${cuda_package_label}/last_failure.json"
qualify_extra_args=()

while [[ $# -gt 0 ]]; do
    case "$1" in
        --duration)
            duration="$2"
            shift 2
            ;;
        --duration=*)
            duration="${1#*=}"
            shift
            ;;
        --interval)
            interval="$2"
            shift 2
            ;;
        --interval=*)
            interval="${1#*=}"
            shift
            ;;
        --label)
            label="$2"
            shift 2
            ;;
        --label=*)
            label="${1#*=}"
            shift
            ;;
        --tarball)
            tarball_path="$2"
            shift 2
            ;;
        --tarball=*)
            tarball_path="${1#*=}"
            shift
            ;;
        --manifest)
            manifest_path="$2"
            shift 2
            ;;
        --manifest=*)
            manifest_path="${1#*=}"
            shift
            ;;
        --help|-h)
            cat <<'EOF'
Usage: bash scripts/build-and-qualify-tensorrt-cold.sh [options] [extra qualify args...]

Build a TensorRT-enabled ORT artifact from a local TensorRT tarball, validate the manifest,
then launch the cold indexer-only TensorRT qualification.

Options:
  --duration N      Qualification duration in seconds (default: 120)
  --interval N      Qualification sample interval in seconds (default: 5)
  --label NAME      Qualification label (default: tensorrt-hard-cut)
  --tarball PATH    Local TensorRT tarball path
  --manifest PATH   Target TensorRT ORT manifest path

Extra arguments are passed to qualify-dev-indexer-tensorrt-cold.sh.
Useful VRAM controls:
  --max-vram-used-mb N
  --gpu-admission-vram-used-mb N
  --tensorrt-workspace-mb N

Build profile:
  AXON_ORT_TENSORRT_BUILD_PROFILE=axon_embedding  # default, skips FlashAttention/NCCL
  AXON_ORT_TENSORRT_BUILD_PROFILE=full            # full ORT CUDA+TensorRT build
EOF
            exit 0
            ;;
        *)
            qualify_extra_args+=("$1")
            shift
            ;;
    esac
done

tarball_path="$(realpath -m "$tarball_path")"
manifest_path="$(realpath -m "$manifest_path")"
if [[ ! -f "$tarball_path" ]]; then
    echo "❌ TensorRT tarball not found: $tarball_path" >&2
    exit 1
fi

echo "🔧 Building TensorRT ORT artifact from local tarball"
echo "   tarball : $tarball_path"
echo "   manifest: $manifest_path"
echo "   nixpkgs : ${AXON_NIXPKGS_SOURCE:-global}"

(
    export AXON_ORT_ARTIFACT_MANIFEST="$manifest_path"
    export TENSORRT_LOCAL_TARBALL="$tarball_path"
    bash "$SCRIPT_DIR/build_ort_tensorrt_artifact.sh"
) || {
    echo "❌ TensorRT ORT artifact build failed before qualification." >&2
    if [[ -f "$failure_path" ]]; then
        echo "   Failure summary: $failure_path" >&2
    fi
    exit 1
}

if [[ ! -f "$manifest_path" ]]; then
    echo "❌ TensorRT manifest missing after build: $manifest_path" >&2
    exit 1
fi

python3 "$SCRIPT_DIR/lib/validate_ort_manifest.py" "$manifest_path"

echo "✅ TensorRT manifest validated"

exec bash "$SCRIPT_DIR/qualify-dev-indexer-tensorrt-cold.sh" \
    --duration "$duration" \
    --interval "$interval" \
    --label "$label" \
    --manifest "$manifest_path" \
    "${qualify_extra_args[@]}"
