#!/usr/bin/env bash
set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$PROJECT_ROOT"
# shellcheck source=scripts/lib/axon-ort-artifact.sh
source "$PROJECT_ROOT/scripts/lib/axon-ort-artifact.sh"

CudaPackageSet="${AXON_CUDA_PACKAGE_SET:-cudaPackages}"
CUDA_PACKAGE_LABEL="${CudaPackageSet//_/-}"
ARTIFACT_DIR="${AXON_ORT_ARTIFACT_DIR:-$PROJECT_ROOT/.axon/ort-artifacts/onnxruntime-tensorrt-${CUDA_PACKAGE_LABEL}}"
MANIFEST_PATH="${AXON_ORT_ARTIFACT_MANIFEST:-$ARTIFACT_DIR/current.json}"
DEFAULT_TENSORRT_LOCAL_TARBALL="$PROJECT_ROOT/.axon/downloads/TensorRT-10.14.1.48.Linux.x86_64-gnu.cuda-12.9.tar.gz"
EXPECTED_TENSORRT_BASENAME="${AXON_EXPECTED_TENSORRT_BASENAME:-TensorRT-10.14.1.48.Linux.x86_64-gnu.cuda-12.9.tar.gz}"
EXPECTED_TENSORRT_VERSION="${AXON_EXPECTED_TENSORRT_VERSION:-10.14.1.48}"
EXPECTED_TENSORRT_SHA256="${AXON_EXPECTED_TENSORRT_SHA256:-0daa7d5929c78edfbe86b474064d0f82d2064c475cc6be747c5101f1ccc37105}"
TENSORRT_LOCAL_TARBALL="${TENSORRT_LOCAL_TARBALL:-$DEFAULT_TENSORRT_LOCAL_TARBALL}"
ORT_BUILD_CORES="${AXON_ORT_BUILD_CORES:-2}"
ORT_TENSORRT_BUILD_PROFILE="${AXON_ORT_TENSORRT_BUILD_PROFILE:-axon_embedding}"
# TensorRT hard-cut depends on the current ORT/CUDA pair, which the repo pin may
# intentionally lag. Keep the source explicit and fail fast if versions drift.
NIXPKGS_SOURCE="${AXON_NIXPKGS_SOURCE:-global}"
EXPECTED_ORT_VERSION="${AXON_EXPECTED_ORT_VERSION:-1.24.4}"
EXPECTED_CUDA_VERSION="${AXON_EXPECTED_CUDA_VERSION:-12.9}"
CUDA_ARCHITECTURES="${AXON_CUDA_ARCHITECTURES:-}"
TENSORRT_PRECHECK_ONLY="${AXON_TENSORRT_PRECHECK_ONLY:-0}"
axon_ort_artifact_prepare "$ARTIFACT_DIR"

LOCK_PATH="$ARTIFACT_DIR/build.lock"
BUILD_PID_PATH="$ARTIFACT_DIR/build.pid"
BUILD_FAILURE_PATH="$ARTIFACT_DIR/last_failure.json"
exec {BUILD_LOCK_FD}>"$LOCK_PATH"
if ! flock -n "$BUILD_LOCK_FD"; then
  echo "❌ TensorRT build already in progress: $LOCK_PATH"
  exit 1
fi
printf '%s\n' "$$" > "$BUILD_PID_PATH"

require_cmd() {
  local cmd="$1"
  command -v "$cmd" >/dev/null 2>&1 || {
    echo "❌ Missing required command: $cmd"
    exit 1
  }
}

cleanup() {
  rm -f "$BUILD_PID_PATH"
}

report_failure() {
  local exit_code="$1"
  cat >&2 <<EOF
❌ TensorRT ORT artifact build failed
   exit_code       : $exit_code
   manifest target : $MANIFEST_PATH
   log path        : ${BUILD_LOG:-<uninitialized>}
   nixpkgs source  : ${NIXPKGS_LABEL:-<uninitialized>}
   cuda set        : ${CudaPackageSet}
   tarball         : ${TENSORRT_LOCAL_TARBALL:-<uninitialized>}
EOF
  if [[ -n "${BUILD_FAILURE_PATH:-}" ]]; then
    cat > "$BUILD_FAILURE_PATH" <<EOF
{
  "failed_at": "$(date -u +"%Y-%m-%dT%H:%M:%SZ")",
  "exit_code": $exit_code,
  "manifest_path": "$MANIFEST_PATH",
  "log_path": "${BUILD_LOG:-}",
  "nixpkgs_source": "${NIXPKGS_LABEL:-}",
  "cuda_package_set": "${CudaPackageSet}",
  "tarball_path": "${TENSORRT_LOCAL_TARBALL:-}"
}
EOF
  fi
}

trap 'exit_code=$?; cleanup; report_failure "$exit_code"; exit "$exit_code"' ERR
trap cleanup EXIT

require_cmd flock
require_cmd mktemp
require_cmd mv
require_cmd nix
require_cmd python3
require_cmd realpath
require_cmd rg
require_cmd sha256sum
require_cmd tee

BUILD_LOG="$(axon_ort_artifact_new_build_log "$AXON_ORT_ARTIFACT_LOG_DIR")"
TENSORRT_LOCAL_TARBALL="$(realpath -m "$TENSORRT_LOCAL_TARBALL")"
if [[ ! -f "$TENSORRT_LOCAL_TARBALL" ]]; then
  echo "❌ TensorRT local tarball not found: $TENSORRT_LOCAL_TARBALL"
  echo "   This build is local-tarball-only by contract and will not fall back to a network fetch."
  exit 1
fi

if [[ "$(basename "$TENSORRT_LOCAL_TARBALL")" != "$EXPECTED_TENSORRT_BASENAME" ]]; then
  echo "❌ Unexpected TensorRT tarball name: $(basename "$TENSORRT_LOCAL_TARBALL")"
  echo "   Expected: $EXPECTED_TENSORRT_BASENAME"
  exit 1
fi

TENSORRT_LOCAL_TARBALL_SHA256="$(sha256sum "$TENSORRT_LOCAL_TARBALL" | awk '{print $1}')"
if [[ "$TENSORRT_LOCAL_TARBALL_SHA256" != "$EXPECTED_TENSORRT_SHA256" ]]; then
  echo "❌ TensorRT tarball sha256 mismatch: $TENSORRT_LOCAL_TARBALL_SHA256"
  echo "   Expected: $EXPECTED_TENSORRT_SHA256"
  exit 1
fi

if ! [[ "$ORT_BUILD_CORES" =~ ^[0-9]+$ ]] || [[ "$ORT_BUILD_CORES" -lt 1 ]]; then
  echo "❌ AXON_ORT_BUILD_CORES must be a positive integer, got: $ORT_BUILD_CORES"
  exit 1
fi

case "$ORT_TENSORRT_BUILD_PROFILE" in
  axon_embedding|full)
    ;;
  *)
    echo "❌ AXON_ORT_TENSORRT_BUILD_PROFILE must be 'axon_embedding' or 'full', got: $ORT_TENSORRT_BUILD_PROFILE"
    exit 1
    ;;
esac

if [[ -z "$CUDA_ARCHITECTURES" ]] && command -v /usr/lib/wsl/lib/nvidia-smi >/dev/null 2>&1; then
  detected_compute_cap="$(/usr/lib/wsl/lib/nvidia-smi --query-gpu=compute_cap --format=csv,noheader 2>/dev/null | head -n 1 | tr -d '[:space:]' || true)"
  if [[ "$detected_compute_cap" =~ ^[0-9]+\.[0-9]+$ ]]; then
    CUDA_ARCHITECTURES="${detected_compute_cap/.}"
  fi
fi

if [[ -z "$CUDA_ARCHITECTURES" ]]; then
  CUDA_ARCHITECTURES="86"
fi

if ! [[ "$CUDA_ARCHITECTURES" =~ ^[0-9]+([;,][0-9]+)*$ ]]; then
  echo "❌ AXON_CUDA_ARCHITECTURES must be a semicolon/comma separated numeric list, got: $CUDA_ARCHITECTURES"
  exit 1
fi

CUDA_ARCHITECTURES="${CUDA_ARCHITECTURES//,/;}"

ORT_TENSORRT_EXTRA_CMAKE_FLAGS=""
ORT_TENSORRT_DISABLED_FEATURES_JSON="[]"
if [[ "$ORT_TENSORRT_BUILD_PROFILE" == "axon_embedding" ]]; then
  # Axon vectorization needs TensorRT/CUDA execution providers, not generative
  # attention kernels or multi-GPU collectives. Disabling these avoids compiling
  # many expensive nvcc FlashAttention/NCCL sources while preserving TensorRT EP.
  ORT_TENSORRT_EXTRA_CMAKE_FLAGS='
      (lib.cmakeBool "onnxruntime_BUILD_UNIT_TESTS" false)
      (lib.cmakeBool "onnxruntime_ENABLE_CUDA_EP_INTERNAL_TESTS" false)
      (lib.cmakeBool "onnxruntime_USE_FLASH_ATTENTION" false)
      (lib.cmakeBool "onnxruntime_USE_MEMORY_EFFICIENT_ATTENTION" false)
      (lib.cmakeBool "onnxruntime_USE_NCCL" false)
      (lib.cmakeBool "onnxruntime_USE_CUDA_NHWC_OPS" false)
      (lib.cmakeBool "onnxruntime_DISABLE_ML_OPS" true)'
  ORT_TENSORRT_DISABLED_FEATURES_JSON='["unit_tests","cuda_ep_internal_tests","flash_attention","memory_efficient_attention","nccl","cuda_nhwc_ops","traditional_ml_ops"]'
fi

case "$NIXPKGS_SOURCE" in
  global)
    NIXPKGS_EXPR='(builtins.getFlake "nixpkgs").outPath'
    NIXPKGS_LABEL='global:nixpkgs'
    ;;
  repo)
    NIXPKGS_EXPR='(let repoFlake = builtins.getFlake "'"path:${PROJECT_ROOT}"'"; in repoFlake.inputs.nixpkgs.outPath)'
    NIXPKGS_LABEL="repo:path:${PROJECT_ROOT}"
    ;;
  *)
    echo "❌ AXON_NIXPKGS_SOURCE must be 'global' or 'repo', got: $NIXPKGS_SOURCE"
    exit 1
    ;;
esac

COMMON_PKG_BINDINGS="let
  pkgs = import ${NIXPKGS_EXPR} {
    system = builtins.currentSystem;
    config = {
      cudaSupport = true;
      allowUnfreePredicate = _: true;
    };
  };
  lib = pkgs.lib;
  cudaPkgs = pkgs.${CudaPackageSet};
  tensorrtPkg = pkgs.stdenvNoCC.mkDerivation {
    pname = \"tensorrt-local\";
    version = \"${EXPECTED_TENSORRT_VERSION}\";
    src = /. + \"${TENSORRT_LOCAL_TARBALL}\";
    dontConfigure = true;
    dontBuild = true;
    sourceRoot = \".\";
    installPhase = ''
      runHook preInstall
      mkdir -p \"\$out\"
      if [ ! -d \"TensorRT-${EXPECTED_TENSORRT_VERSION}\" ]; then
        echo \"Expected TensorRT-${EXPECTED_TENSORRT_VERSION} directory in local tarball\" >&2
        find . -maxdepth 2 -type d | sort >&2
        exit 1
      fi
      cp -a \"TensorRT-${EXPECTED_TENSORRT_VERSION}\"/. \"\$out\"/
      runHook postInstall
    '';
  };
  base = pkgs.onnxruntime.override {
    cudaPackages = cudaPkgs;
  };
  overriddenCmakeFlagPrefixes = [
    \"-DCMAKE_CUDA_ARCHITECTURES\"
    \"-Donnxruntime_BUILD_UNIT_TESTS\"
    \"-Donnxruntime_ENABLE_CUDA_EP_INTERNAL_TESTS\"
    \"-Donnxruntime_USE_FLASH_ATTENTION\"
    \"-Donnxruntime_USE_MEMORY_EFFICIENT_ATTENTION\"
    \"-Donnxruntime_USE_NCCL\"
    \"-Donnxruntime_USE_CUDA_NHWC_OPS\"
    \"-Donnxruntime_DISABLE_ML_OPS\"
    \"-Donnxruntime_USE_TENSORRT_BUILTIN_PARSER\"
  ];
  keepOldCmakeFlag = flag:
    !(lib.any (prefix: lib.hasPrefix prefix flag) overriddenCmakeFlagPrefixes);
  ortPkg = base.overrideAttrs (old: {
    buildInputs = (old.buildInputs or []) ++ [ tensorrtPkg ];
    enableParallelBuilding = false;
    makeFlags = (old.makeFlags or []) ++ [ \"-j${ORT_BUILD_CORES}\" ];
    env = (old.env or {}) // {
      CMAKE_BUILD_PARALLEL_LEVEL = \"${ORT_BUILD_CORES}\";
    };
    cmakeFlags = (lib.filter keepOldCmakeFlag (old.cmakeFlags or [])) ++ [
      (lib.cmakeFeature \"CMAKE_CUDA_ARCHITECTURES\" \"${CUDA_ARCHITECTURES}\")
      (lib.cmakeBool \"onnxruntime_USE_TENSORRT\" true)
      (lib.cmakeFeature \"onnxruntime_TENSORRT_HOME\" \"\${tensorrtPkg}\")
      (lib.cmakeBool \"onnxruntime_USE_TENSORRT_BUILTIN_PARSER\" true)
${ORT_TENSORRT_EXTRA_CMAKE_FLAGS}
    ];
  });
in"

BUILD_PLAN_EXPR="${COMMON_PKG_BINDINGS} {
  tensorrtDrvPath = tensorrtPkg.drvPath;
  tensorrtOutPath = tensorrtPkg.outPath;
  ortDrvPath = ortPkg.drvPath;
  ortOutPath = ortPkg.outPath;
}"

TARGET_EXPR="${COMMON_PKG_BINDINGS} ortPkg"

RESOLVED_ORT_VERSION="$(nix eval --impure --raw --expr "let pkgs = import ${NIXPKGS_EXPR} { system = builtins.currentSystem; config = { cudaSupport = true; allowUnfreePredicate = _: true; }; }; in pkgs.onnxruntime.version")"
RESOLVED_CUDA_VERSION="$(nix eval --impure --raw --expr "let pkgs = import ${NIXPKGS_EXPR} { system = builtins.currentSystem; config = { cudaSupport = true; allowUnfreePredicate = _: true; }; }; in pkgs.${CudaPackageSet}.cudaMajorMinorVersion")"

if [[ "$RESOLVED_ORT_VERSION" != "$EXPECTED_ORT_VERSION" ]]; then
  echo "❌ Refusing build: resolved onnxruntime version is $RESOLVED_ORT_VERSION, expected $EXPECTED_ORT_VERSION"
  echo "   nixpkgs source: $NIXPKGS_LABEL"
  echo "   Either update the repo-pinned nixpkgs input or rerun explicitly with AXON_NIXPKGS_SOURCE=global."
  exit 1
fi

if [[ "$RESOLVED_CUDA_VERSION" != "$EXPECTED_CUDA_VERSION" ]]; then
  echo "❌ Refusing build: resolved CUDA version is $RESOLVED_CUDA_VERSION, expected $EXPECTED_CUDA_VERSION"
  echo "   nixpkgs source: $NIXPKGS_LABEL"
  echo "   Either update the repo-pinned nixpkgs input or rerun explicitly with AXON_NIXPKGS_SOURCE=global."
  exit 1
fi

BUILD_PLAN_JSON="$(nix eval --impure --json --expr "$BUILD_PLAN_EXPR")"
TENSORRT_DRV_PATH="$(python3 -c 'import json,sys; print(json.load(sys.stdin)["tensorrtDrvPath"])' <<<"$BUILD_PLAN_JSON")"
TENSORRT_OUT_PATH="$(python3 -c 'import json,sys; print(json.load(sys.stdin)["tensorrtOutPath"])' <<<"$BUILD_PLAN_JSON")"
ORT_DRV_PATH="$(python3 -c 'import json,sys; print(json.load(sys.stdin)["ortDrvPath"])' <<<"$BUILD_PLAN_JSON")"
OUT_PATH="$(python3 -c 'import json,sys; print(json.load(sys.stdin)["ortOutPath"])' <<<"$BUILD_PLAN_JSON")"

PREBUILD_DRV_JSON="$(mktemp "$AXON_ORT_ARTIFACT_LOG_DIR/prebuild-derivation-XXXXXX.json")"
nix derivation show "$TENSORRT_DRV_PATH" > "$PREBUILD_DRV_JSON"
if rg -q 'developer\.download\.nvidia\.com|TensorRT-10\.14\.1\.48\.[^"]*tar\.gz\.drv' "$PREBUILD_DRV_JSON"; then
  echo "❌ Refusing TensorRT build: the derived TensorRT package still references an upstream network fetch."
  echo "   Derivation evidence: $PREBUILD_DRV_JSON"
  exit 1
fi

echo "🔧 Building external TensorRT-enabled ONNX Runtime artifact..."
echo "   CUDA set: $CudaPackageSet"
echo "   nixpkgs source : $NIXPKGS_LABEL"
echo "   ORT version    : $RESOLVED_ORT_VERSION"
echo "   CUDA version   : $RESOLVED_CUDA_VERSION"
echo "   CUDA archs     : $CUDA_ARCHITECTURES"
echo "   build profile  : $ORT_TENSORRT_BUILD_PROFILE"
echo "   Manifest: $MANIFEST_PATH"
echo "   Log     : $BUILD_LOG"
echo "   TensorRT tarball: $TENSORRT_LOCAL_TARBALL"
echo "   TensorRT sha256 : $TENSORRT_LOCAL_TARBALL_SHA256"
echo "   TensorRT drv    : $TENSORRT_DRV_PATH"
echo "   TensorRT out    : $TENSORRT_OUT_PATH"
echo "   ORT drv         : $ORT_DRV_PATH"
echo "   ORT build cores   : $ORT_BUILD_CORES"

echo "🔎 Validating local TensorRT package layout before the long ORT build..."
nix build --impure --no-link --expr "${COMMON_PKG_BINDINGS} tensorrtPkg" 2>&1 | tee -a "$BUILD_LOG" >/dev/null

if [[ ! -f "$TENSORRT_OUT_PATH/include/NvInferVersion.h" ]]; then
  echo "❌ TensorRT package layout invalid: missing $TENSORRT_OUT_PATH/include/NvInferVersion.h"
  exit 1
fi

if [[ ! -f "$TENSORRT_OUT_PATH/include/NvInfer.h" ]]; then
  echo "❌ TensorRT package layout invalid: missing $TENSORRT_OUT_PATH/include/NvInfer.h"
  exit 1
fi

if [[ ! -f "$TENSORRT_OUT_PATH/include/NvOnnxParser.h" ]]; then
  echo "❌ TensorRT package layout invalid: missing $TENSORRT_OUT_PATH/include/NvOnnxParser.h"
  exit 1
fi

if [[ ! -f "$TENSORRT_OUT_PATH/lib/libnvinfer.so" ]]; then
  echo "❌ TensorRT package layout invalid: missing $TENSORRT_OUT_PATH/lib/libnvinfer.so"
  exit 1
fi

if [[ ! -f "$TENSORRT_OUT_PATH/lib/libnvonnxparser.so" ]]; then
  echo "❌ TensorRT package layout invalid: missing $TENSORRT_OUT_PATH/lib/libnvonnxparser.so"
  exit 1
fi

if [[ ! -f "$TENSORRT_OUT_PATH/lib/libnvinfer_plugin.so" ]]; then
  echo "❌ TensorRT package layout invalid: missing $TENSORRT_OUT_PATH/lib/libnvinfer_plugin.so"
  exit 1
fi

if [[ "$TENSORRT_PRECHECK_ONLY" =~ ^(1|true|yes|on)$ ]]; then
  echo "✅ TensorRT precheck passed; skipping ORT build because AXON_TENSORRT_PRECHECK_ONLY=$TENSORRT_PRECHECK_ONLY"
  exit 0
fi

NIX_BUILD_CORES="$ORT_BUILD_CORES" nix build --impure --no-link --expr "$TARGET_EXPR" 2>&1 | tee -a "$BUILD_LOG" >/dev/null

CORE_LIB="$OUT_PATH/lib/libonnxruntime.so"
CUDA_PROVIDER_LIB="$OUT_PATH/lib/libonnxruntime_providers_cuda.so"
TENSORRT_PROVIDER_LIB="$OUT_PATH/lib/libonnxruntime_providers_tensorrt.so"
TENSORRT_LIB_DIR="$TENSORRT_OUT_PATH/lib"

if [[ ! -f "$CORE_LIB" ]]; then
  echo "❌ Missing core ORT shared library: $CORE_LIB"
  exit 1
fi

if [[ ! -f "$CUDA_PROVIDER_LIB" ]]; then
  echo "❌ Missing CUDA provider shared library: $CUDA_PROVIDER_LIB"
  exit 1
fi

if [[ ! -f "$TENSORRT_PROVIDER_LIB" ]]; then
  echo "❌ Missing TensorRT provider shared library: $TENSORRT_PROVIDER_LIB"
  exit 1
fi

axon_ort_artifact_write_manifest "$MANIFEST_PATH" <<EOF
{
  "artifact_kind": "onnxruntime_tensorrt_system",
  "built_at": "$(date -u +"%Y-%m-%dT%H:%M:%SZ")",
  "cuda_package_set": "$CudaPackageSet",
  "out_path": "$OUT_PATH",
  "core_lib": "$CORE_LIB",
  "cuda_provider_lib": "$CUDA_PROVIDER_LIB",
  "tensorrt_provider_lib": "$TENSORRT_PROVIDER_LIB",
  "tensorrt_lib_dir": "$TENSORRT_LIB_DIR",
  "provider": "tensorrt",
  "build_profile": "$ORT_TENSORRT_BUILD_PROFILE",
  "disabled_features": $ORT_TENSORRT_DISABLED_FEATURES_JSON,
  "integration_status": "external_unwired",
  "log_path": "$BUILD_LOG"
}
EOF

echo "✅ External ORT TensorRT artifact ready"
echo "   out_path               : $OUT_PATH"
echo "   core lib               : $CORE_LIB"
echo "   cuda provider lib      : $CUDA_PROVIDER_LIB"
echo "   tensorrt provider lib  : $TENSORRT_PROVIDER_LIB"
echo "   manifest               : $MANIFEST_PATH"
