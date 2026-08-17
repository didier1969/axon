#!/usr/bin/env bash

axon_manifest_value() {
    local manifest_path="${1:?manifest path required}"
    local key="${2:?manifest key required}"
    python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get(sys.argv[2], ""))' "$manifest_path" "$key" 2>/dev/null || true
}

# REQ-AXO-902344 — resolve the nix-store libstdc++ directory DETERMINISTICALLY.
# Nix-built libonnxruntime carries CXXABI/GLIBCXX requirements only a recent
# libstdc++ satisfies: ORT 1.27.1's libonnxruntime_providers_cuda.so needs
# CXXABI_1.3.15, i.e. gcc >= 14.
#
# The previous `find ... | head -1` returned whatever the /nix/store directory
# order happened to be, and that order is NOT stable. On 2026-08-17 (live AXO,
# post WSL->Ubuntu migration) it yielded gcc-13.4.0 — whose libstdc++ stops at
# CXXABI_1.3.14 — while the same store also held gcc-15.3.0. The CUDA EP then
# failed to dlopen and the embedder fell back to CPU with NO error channel:
# `embedding_status` reported compute=CPU and the brain's query-embed worker
# went unavailable, all while the runtime declared itself HEALTHY.
#
# libstdc++ is forward-compatible — the newest lib satisfies every older
# consumer — so max-version is both the correct and the stable choice.
# Prints the lib dir on stdout; prints NOTHING when the store holds no
# gcc-*-lib (the old form degenerated to the literal "/lib", which exists on
# Ubuntu and would have been prepended to LD_LIBRARY_PATH).
axon_resolve_nix_gcc_lib_dir() {
    local newest
    newest="$(find /nix/store -maxdepth 1 -name '*-gcc-*-lib' -type d 2>/dev/null \
        | sed -E 's|^.*-gcc-([0-9]+(\.[0-9]+)*)-lib$|\1\t&|' \
        | sort -t$'\t' -k1,1 -V \
        | tail -n 1 \
        | cut -f2-)"
    if [[ -n "$newest" && -d "$newest/lib" ]]; then
        printf '%s\n' "$newest/lib"
    fi
}

# REQ-AXO-902347 — locate the NVIDIA *driver* library directory.
#
# `libcuda.so.1` ships with the DRIVER, not with the CUDA toolkit: it is not in
# the nix store and appears in no RPATH. Nix-built binaries run under the nix
# dynamic loader, which does NOT search /usr/lib/x86_64-linux-gnu, so without an
# explicit LD_LIBRARY_PATH segment `libonnxruntime_providers_cuda.so` fails to
# dlopen with:
#     libcuda.so.1: cannot open shared object file: No such file or directory
# and the embedder falls back to CPU. Under WSL2 the `/usr/lib/wsl/lib` segment
# happened to cover this; native Linux had no equivalent, which is what broke
# GPU embedding after the 2026-08-17 WSL->Ubuntu migration.
#
# ⚠️ `ldd` does NOT reveal this: it resolves through the SYSTEM loader (which
# reads /etc/ld.so.conf and does find libcuda.so.1), so a clean `ldd` is a FALSE
# NEGATIVE for a nix-loaded process. Trust the runtime dlopen error, not ldd.
#
# Prints the directory on stdout, or nothing when no driver lib is installed.
axon_resolve_nvidia_driver_lib_dir() {
    local dir
    for dir in /usr/lib/x86_64-linux-gnu /usr/lib64 /usr/lib; do
        if [[ -e "$dir/libcuda.so.1" ]]; then
            printf '%s\n' "$dir"
            return 0
        fi
    done
}

# REQ-AXO-902345 — compose the COMPLETE LD_LIBRARY_PATH prefix that an ORT/CUDA
# consumer needs, in ONE place.
#
# The individual segment lookups were already shared helpers, but the ASSEMBLY
# stayed duplicated three times — this resolver plus both dev benches — with the
# same segments in the same order, i.e. three chances to drift apart. That is how
# the driver-lib segment came to be missing from the runtime while the benches
# were being fixed separately. Any segment added here is inherited by every
# consumer, which is the whole point.
#
#   $1 = ORT lib directory (dirname of the manifest's core_lib) — required
#   $2 = TensorRT lib directory — optional
#
# Prints the ':'-joined prefix on stdout. Does NOT append the caller's inherited
# LD_LIBRARY_PATH: the caller decides where its own value goes.
axon_compose_ort_ld_library_path() {
    local ort_lib_dir="${1:-}"
    local tensorrt_lib_dir="${2:-}"
    local -a segments=()
    local resolved

    if [[ -n "$ort_lib_dir" && -d "$ort_lib_dir" ]]; then
        segments+=("$ort_lib_dir")
    fi
    if [[ -n "$tensorrt_lib_dir" && -d "$tensorrt_lib_dir" ]]; then
        segments+=("$tensorrt_lib_dir")
    fi
    # WSL2 paravirtualised driver libs. Kept for hosts still on WSL2; on native
    # Linux the directory is simply absent and the next segment covers it.
    if [[ -d "/usr/lib/wsl/lib" ]]; then
        segments+=("/usr/lib/wsl/lib")
    fi
    resolved="$(axon_resolve_nvidia_driver_lib_dir)"
    if [[ -n "$resolved" ]]; then
        segments+=("$resolved")
    fi
    resolved="$(axon_resolve_nix_gcc_lib_dir)"
    if [[ -n "$resolved" ]]; then
        segments+=("$resolved")
    fi

    if [[ ${#segments[@]} -gt 0 ]]; then
        (IFS=:; printf '%s\n' "${segments[*]}")
    fi
    return 0
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

    # REQ-AXO-901737 : tensorrt request derived directly from the canonical
    # `AXON_EMBEDDING_PROVIDER` knob. Legacy `AXON_GPU_EMBED_SERVICE_TENSORRT`
    # honored as a deprecated alias for older bench scripts.
    if [[ "$embedding_provider_request" == "tensorrt" ]] || \
       [[ "${AXON_GPU_EMBED_SERVICE_TENSORRT:-0}" =~ ^(1|true|yes|on)$ ]]; then
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

    if [[ "$embedding_provider_request" == "cuda" || "$embedding_provider_request" == "tensorrt" ]]; then
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
                # REQ-AXO-901630 — log the resolved provider paths so the
                # operator can verify at boot which `.so` files the
                # indexer will dlopen. Diagnosing session 49's silent
                # NoOpEmbedder fallback required reading these from a
                # stack trace ; surfacing them here turns it into a
                # one-line check.
                echo "   core_lib:               $ORT_DYLIB_PATH"
                echo "   cuda_provider_lib:      $CUDA_PROVIDER_PATH"
                if [[ "$gpu_service_tensorrt_requested" == "1" ]]; then
                    echo "   tensorrt_provider_lib:  $TENSORRT_PROVIDER_PATH"
                    if [[ -n "${TENSORRT_LIB_DIR:-}" && -d "$TENSORRT_LIB_DIR" ]]; then
                        echo "   tensorrt_lib_dir:       $TENSORRT_LIB_DIR"
                    fi
                fi
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
            if [[ "$embedding_provider_request" == "cuda" || "$embedding_provider_request" == "tensorrt" ]]; then
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

    if [[ "$embedding_provider_request" == "cuda" || "$embedding_provider_request" == "tensorrt" ]]; then
        local ort_lib_dir
        local cuda_ld_prefix

        # REQ-AXO-902345 — the segment list (ORT libs, TensorRT libs, WSL2 libs,
        # NVIDIA driver libs, nix libstdc++) is assembled ONCE, in
        # axon_compose_ort_ld_library_path, and shared with both dev benches.
        ort_lib_dir="$(dirname "$ORT_DYLIB_PATH")"
        cuda_ld_prefix="$(axon_compose_ort_ld_library_path "$ort_lib_dir" "${TENSORRT_LIB_DIR:-}")"
        if [[ -n "$cuda_ld_prefix" ]]; then
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
    export ORT_DYLIB_PATH
    export TENSORRT_LIB_DIR
    export GPU_SERVICE_TENSORRT_REQUESTED
}
