#!/usr/bin/env bash
set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$PROJECT_ROOT"

CudaPackageSet="${AXON_CUDA_PACKAGE_SET:-cudaPackages}"
CUDA_PACKAGE_LABEL="${CudaPackageSet//_/-}"
ARTIFACT_DIR="${AXON_ORT_ARTIFACT_DIR:-$PROJECT_ROOT/.axon/ort-artifacts/onnxruntime-${CUDA_PACKAGE_LABEL}}"
MANIFEST_PATH="${AXON_ORT_ARTIFACT_MANIFEST:-$ARTIFACT_DIR/current.json}"
LOG_DIR="$ARTIFACT_DIR/logs"
mkdir -p "$LOG_DIR"

BUILD_LOG="$(mktemp "$LOG_DIR/build-XXXXXX.log")"
TARGET_EXPR="let
  pkgs = import (builtins.getFlake \"nixpkgs\").outPath {
    system = builtins.currentSystem;
    config = {
      cudaSupport = true;
      allowUnfreePredicate = _: true;
    };
  };
  cudaPkgs = pkgs.${CudaPackageSet};
in pkgs.onnxruntime.override {
  cudaPackages = cudaPkgs;
}"

echo "🔧 Building external CUDA-enabled ONNX Runtime artifact..."
echo "   CUDA set: $CudaPackageSet"
echo "   Manifest: $MANIFEST_PATH"
echo "   Log     : $BUILD_LOG"

OUT_PATH="$(nix build --impure --no-link --print-out-paths --expr "$TARGET_EXPR" 2>&1 | tee "$BUILD_LOG" | tail -n 1)"

if [[ -z "${OUT_PATH:-}" ]]; then
  echo "❌ nix build did not return an output path"
  exit 1
fi

CORE_LIB="$OUT_PATH/lib/libonnxruntime.so"
CUDA_PROVIDER_LIB="$OUT_PATH/lib/libonnxruntime_providers_cuda.so"

if [[ ! -f "$CORE_LIB" ]]; then
  echo "❌ Missing core ORT shared library: $CORE_LIB"
  exit 1
fi

if [[ ! -f "$CUDA_PROVIDER_LIB" ]]; then
  echo "❌ Missing CUDA provider shared library: $CUDA_PROVIDER_LIB"
  exit 1
fi

mkdir -p "$(dirname "$MANIFEST_PATH")"
cat > "$MANIFEST_PATH" <<EOF
{
  "artifact_kind": "onnxruntime_cuda_system",
  "built_at": "$(date -u +"%Y-%m-%dT%H:%M:%SZ")",
  "cuda_package_set": "$CudaPackageSet",
  "out_path": "$OUT_PATH",
  "core_lib": "$CORE_LIB",
  "cuda_provider_lib": "$CUDA_PROVIDER_LIB",
  "provider": "cuda",
  "integration_status": "external_unwired",
  "log_path": "$BUILD_LOG"
}
EOF

echo "✅ External ORT CUDA artifact ready"
echo "   out_path          : $OUT_PATH"
echo "   core lib          : $CORE_LIB"
echo "   cuda provider lib : $CUDA_PROVIDER_LIB"
echo "   manifest          : $MANIFEST_PATH"
