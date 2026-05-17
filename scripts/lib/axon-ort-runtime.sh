#!/usr/bin/env bash

axon_manifest_value() {
    local manifest_path="${1:?manifest path required}"
    local key="${2:?manifest key required}"
    python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get(sys.argv[2], ""))' "$manifest_path" "$key" 2>/dev/null || true
}

axon_resolve_ort_runtime() {
    local project_root="${1:?project root required}"
    local embedding_provider_request="${2:?embedding provider required}"
    local cuda_package_set="${AXON_CUDA_PACKAGE_SET:-cudaPackages}"
    local cuda_package_label="${cuda_package_set//_/-}"
    local gpu_service_tensorrt_requested=0

    PRELAUNCH_LD_LIBRARY_PATH_EXPORT=""
    ORT_BUILD_LOG="$(mktemp /tmp/axon-ort-build.XXXXXX.log)"
    ORT_BUILD_TARGET="nixpkgs#onnxruntime"
    ORT_OUT_PATH=""
    ORT_DYLIB_PATH=""
    TENSORRT_LIB_DIR=""
    GPU_SERVICE_TENSORRT_REQUESTED=0

    if [[ "${AXON_GPU_EMBED_SERVICE_TENSORRT:-0}" =~ ^(1|true|yes|on)$ ]]; then
        gpu_service_tensorrt_requested=1
        GPU_SERVICE_TENSORRT_REQUESTED=1
    fi

    if [[ -n "${AXON_ORT_ARTIFACT_MANIFEST:-}" ]]; then
        ORT_ARTIFACT_MANIFEST="$AXON_ORT_ARTIFACT_MANIFEST"
    elif [[ "$gpu_service_tensorrt_requested" == "1" ]]; then
        ORT_ARTIFACT_MANIFEST="$project_root/.axon/ort-artifacts/onnxruntime-tensorrt-${cuda_package_label}/current.json"
    else
        ORT_ARTIFACT_MANIFEST="$project_root/.axon/ort-artifacts/onnxruntime-cuda/current.json"
    fi

    if [[ "$embedding_provider_request" == "cuda" ]]; then
        if [[ -f "$ORT_ARTIFACT_MANIFEST" ]]; then
            ORT_DYLIB_PATH="$(axon_manifest_value "$ORT_ARTIFACT_MANIFEST" "core_lib")"
            CUDA_PROVIDER_PATH="$(axon_manifest_value "$ORT_ARTIFACT_MANIFEST" "cuda_provider_lib")"
            TENSORRT_PROVIDER_PATH="$(axon_manifest_value "$ORT_ARTIFACT_MANIFEST" "tensorrt_provider_lib")"
            TENSORRT_LIB_DIR="$(axon_manifest_value "$ORT_ARTIFACT_MANIFEST" "tensorrt_lib_dir")"
            if [[ -n "${ORT_DYLIB_PATH:-}" && -f "$ORT_DYLIB_PATH" && -n "${CUDA_PROVIDER_PATH:-}" && -f "$CUDA_PROVIDER_PATH" ]] && { [[ "$gpu_service_tensorrt_requested" != "1" ]] || [[ -n "${TENSORRT_PROVIDER_PATH:-}" && -f "$TENSORRT_PROVIDER_PATH" ]]; }; then
                ORT_OUT_PATH="$(dirname "$(dirname "$ORT_DYLIB_PATH")")"
                if [[ "$gpu_service_tensorrt_requested" == "1" ]]; then
                    echo "♻️ Using external TensorRT ONNX Runtime artifact from manifest..."
                else
                    echo "♻️ Using external CUDA ONNX Runtime artifact from manifest..."
                fi
                echo "   Manifest: $ORT_ARTIFACT_MANIFEST"
            else
                if [[ "$gpu_service_tensorrt_requested" == "1" ]]; then
                    axon_log_warn "Ignoring invalid external TensorRT artifact manifest: $ORT_ARTIFACT_MANIFEST"
                    echo "   TensorRT mode requires core, CUDA provider, and TensorRT provider libraries."
                else
                    axon_log_warn "Ignoring invalid external CUDA artifact manifest: $ORT_ARTIFACT_MANIFEST"
                fi
                ORT_DYLIB_PATH=""

                # REQ-AXO-91564 — when the cuda-only manifest points to a
                # nix-store path the GC already swept, attempt a sibling
                # fallback to the tensorrt manifest (same `core_lib` +
                # `cuda_provider_lib` layout, just contains the TRT
                # provider too). Saves a 30-60 min nixpkgs#onnxruntime
                # rebuild whenever the cuda-only artifact's store path
                # gets garbage-collected but the tensorrt one survives.
                # Only attempted when caller did NOT request tensorrt
                # explicitly (because the tensorrt branch already reads
                # this same manifest).
                local sibling_manifest
                if [[ "$gpu_service_tensorrt_requested" != "1" ]]; then
                    sibling_manifest="$project_root/.axon/ort-artifacts/onnxruntime-tensorrt-${cuda_package_label}/current.json"
                    if [[ -f "$sibling_manifest" ]]; then
                        local sibling_core
                        local sibling_cuda
                        sibling_core="$(axon_manifest_value "$sibling_manifest" "core_lib")"
                        sibling_cuda="$(axon_manifest_value "$sibling_manifest" "cuda_provider_lib")"
                        if [[ -n "${sibling_core:-}" && -f "$sibling_core" && -n "${sibling_cuda:-}" && -f "$sibling_cuda" ]]; then
                            echo "♻️ CUDA manifest stale ; reusing sibling TensorRT artifact for cuda provider (REQ-AXO-91564)."
                            echo "   Sibling manifest: $sibling_manifest"
                            ORT_DYLIB_PATH="$sibling_core"
                            CUDA_PROVIDER_PATH="$sibling_cuda"
                            ORT_OUT_PATH="$(dirname "$(dirname "$ORT_DYLIB_PATH")")"
                        fi
                    fi
                fi

                if [[ -z "${ORT_DYLIB_PATH:-}" ]]; then
                    echo "   Falling back to nixpkgs materialization."
                fi
            fi
        fi

        if [[ -z "${ORT_DYLIB_PATH:-}" ]]; then
            if [[ "$gpu_service_tensorrt_requested" == "1" ]]; then
                echo "❌ TensorRT mode requires a validated local ORT artifact manifest."
                echo "   Missing or invalid manifest: $ORT_ARTIFACT_MANIFEST"
                echo "   Build it first with: bash scripts/build_ort_tensorrt_artifact.sh"
                echo "   Or use: ./scripts/axon-dev qualify --cold --tensorrt --build-tensorrt-from-tarball PATH"
                return 1
            fi

            ORT_BUILD_TARGET="(import (builtins.getFlake \"nixpkgs\").outPath {
              system = builtins.currentSystem;
              config = {
                cudaSupport = true;
                allowUnfreePredicate = _: true;
              };
            }).onnxruntime"
            echo "🔧 Materializing CUDA-enabled ONNX Runtime from nixpkgs..."
        fi
    fi

    if [[ -z "${ORT_DYLIB_PATH:-}" ]]; then
        if [[ "$ORT_BUILD_TARGET" == "nixpkgs#onnxruntime" ]]; then
            ORT_OUT_PATH="$(nix build --no-link --print-out-paths "$ORT_BUILD_TARGET" 2>&1 | tee "$ORT_BUILD_LOG" | tail -n 1)"
        else
            ORT_OUT_PATH="$(nix build --impure --no-link --print-out-paths --expr "$ORT_BUILD_TARGET" 2>&1 | tee "$ORT_BUILD_LOG" | tail -n 1)"
        fi
        if [[ -z "${ORT_OUT_PATH:-}" || ! -f "$ORT_OUT_PATH/lib/libonnxruntime.so" ]]; then
            echo "❌ Unable to materialize a valid ONNX Runtime output path."
            if [[ "$embedding_provider_request" == "cuda" ]]; then
                echo "   Tried to build nixpkgs onnxruntime with cudaSupport=true."
                if rg -q "unexpected eof while reading|cannot download .*cudnn|developer\\.download\\.nvidia\\.com" "$ORT_BUILD_LOG" 2>/dev/null; then
                    echo "   The failure came from downloading NVIDIA CUDA/cuDNN artifacts, not from Axon itself."
                    echo "   Retry the start once connectivity to developer.download.nvidia.com is stable."
                fi
            fi
            echo "   Build log: $ORT_BUILD_LOG"
            return 1
        fi
        ORT_DYLIB_PATH="$ORT_OUT_PATH/lib/libonnxruntime.so"
    fi

    if [[ "$embedding_provider_request" == "cuda" ]]; then
        local ort_lib_dir
        local cuda_ld_prefix
        local -a cuda_ld_path_segments=()

        ort_lib_dir="$(dirname "$ORT_DYLIB_PATH")"
        if [[ -d "$ort_lib_dir" ]]; then
            cuda_ld_path_segments+=("$ort_lib_dir")
        fi
        if [[ -n "${TENSORRT_LIB_DIR:-}" && -d "$TENSORRT_LIB_DIR" ]]; then
            cuda_ld_path_segments+=("$TENSORRT_LIB_DIR")
        fi
        if [[ -d "/usr/lib/wsl/lib" ]]; then
            cuda_ld_path_segments+=("/usr/lib/wsl/lib")
        fi
        # REQ-AXO-181: Nix gcc-cc.lib provides libstdc++.so.6 with GLIBCXX
        # symbols required by Nix-built libonnxruntime.so. Mirrors
        # scripts/dev/embed-bench.sh:64. Without this, indexer subprocess
        # dlopen fails against system /lib/x86_64-linux-gnu/libstdc++.
        local nix_gcc_lib
        nix_gcc_lib="$(find /nix/store -maxdepth 1 -name '*-gcc-*-lib' -type d 2>/dev/null | head -1)/lib"
        if [[ -n "$nix_gcc_lib" && -d "$nix_gcc_lib" ]]; then
            cuda_ld_path_segments+=("$nix_gcc_lib")
        fi
        if [[ ${#cuda_ld_path_segments[@]} -gt 0 ]]; then
            cuda_ld_prefix="$(IFS=:; echo "${cuda_ld_path_segments[*]}")"
            if [[ -n "${LD_LIBRARY_PATH:-}" ]]; then
                PRELAUNCH_LD_LIBRARY_PATH_EXPORT="export LD_LIBRARY_PATH=\"$cuda_ld_prefix:$LD_LIBRARY_PATH\"; "
            else
                PRELAUNCH_LD_LIBRARY_PATH_EXPORT="export LD_LIBRARY_PATH=\"$cuda_ld_prefix\"; "
            fi
        fi

        if [[ ! -f "$ORT_OUT_PATH/lib/libonnxruntime_providers_cuda.so" ]]; then
            axon_log_warn "The selected ONNX Runtime package does not include libonnxruntime_providers_cuda.so."
            echo "   CUDA embedding cannot activate with this system ORT package; Axon will fall back to CPU diagnostics."
        fi
        if [[ "$gpu_service_tensorrt_requested" == "1" && ! -f "$ORT_OUT_PATH/lib/libonnxruntime_providers_tensorrt.so" ]]; then
            echo "❌ TensorRT mode requested but the selected ONNX Runtime package does not include libonnxruntime_providers_tensorrt.so."
            echo "   Build or point to a TensorRT-enabled ORT artifact before starting Axon."
            return 1
        fi
    fi

    export PRELAUNCH_LD_LIBRARY_PATH_EXPORT
    export ORT_ARTIFACT_MANIFEST
    export ORT_BUILD_LOG
    export ORT_OUT_PATH
    export ORT_DYLIB_PATH
    export TENSORRT_LIB_DIR
    export GPU_SERVICE_TENSORRT_REQUESTED
}
