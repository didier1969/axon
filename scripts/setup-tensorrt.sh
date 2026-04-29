#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

cuda_package_set="${AXON_CUDA_PACKAGE_SET:-cudaPackages}"
cuda_package_label="${cuda_package_set//_/-}"
default_tarball="$PROJECT_ROOT/.axon/downloads/TensorRT-10.14.1.48.Linux.x86_64-gnu.cuda-12.9.tar.gz"
tarball_path="${TENSORRT_LOCAL_TARBALL:-$default_tarball}"
manifest_path="${AXON_ORT_ARTIFACT_MANIFEST:-$PROJECT_ROOT/.axon/ort-artifacts/onnxruntime-tensorrt-${cuda_package_label}/current.json}"
mode="build"
duration="120"
interval="5"
label="tensorrt-install-qualification"
qualify_args=()

usage() {
    cat <<'EOF'
Usage: bash scripts/setup-tensorrt.sh [options] [qualification options...]

Install the reproducible local TensorRT runtime artifact for Axon:
1. validate the NVIDIA TensorRT tarball already present on disk
2. build ONNX Runtime with CUDA + TensorRT providers through Nix
3. write and validate the Axon ORT TensorRT manifest
4. optionally run a bounded cold TensorRT qualification

Options:
  --tarball PATH       Local NVIDIA TensorRT tarball
  --manifest PATH      Target ORT TensorRT manifest path
  --precheck-only      Validate the local TensorRT package layout only
  --build-only         Build and validate the artifact, then stop (default)
  --qualify            Build, validate, then run cold TensorRT qualification
  --duration N         Qualification duration in seconds (default: 120)
  --interval N         Qualification sample interval in seconds (default: 5)
  --label NAME         Qualification label (default: tensorrt-install-qualification)

Common bounded-VRAM qualification options, passed through with --qualify:
  --max-vram-used-mb N
  --gpu-admission-vram-used-mb N
  --tensorrt-workspace-mb N

Required local asset:
  .axon/downloads/TensorRT-10.14.1.48.Linux.x86_64-gnu.cuda-12.9.tar.gz

Reason:
  NVIDIA TensorRT distribution requires an accepted NVIDIA download/license flow.
  Axon never hides a network fetch in setup; client installs must provide the
  approved tarball, and Axon validates filename, version and sha256 before use.
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
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
        --precheck-only)
            mode="precheck"
            shift
            ;;
        --build-only)
            mode="build"
            shift
            ;;
        --qualify)
            mode="qualify"
            shift
            ;;
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
        --max-vram-used-mb|--gpu-admission-vram-used-mb|--tensorrt-workspace-mb)
            qualify_args+=("$1" "$2")
            shift 2
            ;;
        --max-vram-used-mb=*|--gpu-admission-vram-used-mb=*|--tensorrt-workspace-mb=*)
            qualify_args+=("$1")
            shift
            ;;
        --help|-h)
            usage
            exit 0
            ;;
        *)
            echo "Unknown option: $1" >&2
            usage >&2
            exit 1
            ;;
    esac
done

tarball_path="$(realpath -m "$tarball_path")"
manifest_path="$(realpath -m "$manifest_path")"

if [[ ! -f "$tarball_path" ]]; then
    cat >&2 <<EOF
❌ TensorRT tarball not found: $tarball_path

Place the NVIDIA-approved TensorRT tarball at:
  $default_tarball

or pass:
  --tarball /path/to/TensorRT-10.14.1.48.Linux.x86_64-gnu.cuda-12.9.tar.gz
EOF
    exit 1
fi

echo "🧩 Axon TensorRT setup"
echo "   mode          : $mode"
echo "   tarball       : $tarball_path"
echo "   manifest      : $manifest_path"
echo "   build profile : ${AXON_ORT_TENSORRT_BUILD_PROFILE:-axon_embedding}"

if [[ "$mode" == "qualify" ]]; then
    exec bash "$SCRIPT_DIR/build-and-qualify-tensorrt-cold.sh" \
        --tarball "$tarball_path" \
        --manifest "$manifest_path" \
        --duration "$duration" \
        --interval "$interval" \
        --label "$label" \
        "${qualify_args[@]}"
fi

(
    export AXON_ORT_ARTIFACT_MANIFEST="$manifest_path"
    export TENSORRT_LOCAL_TARBALL="$tarball_path"
    if [[ "$mode" == "precheck" ]]; then
        export AXON_TENSORRT_PRECHECK_ONLY=1
    fi
    bash "$SCRIPT_DIR/build_ort_tensorrt_artifact.sh"
)

if [[ "$mode" == "precheck" ]]; then
    echo "✅ TensorRT local package precheck complete."
    exit 0
fi

python3 "$SCRIPT_DIR/lib/validate_ort_manifest.py" "$manifest_path"
echo "✅ TensorRT artifact installed and manifest validated: $manifest_path"
