#!/bin/bash
set -euo pipefail

resolve_cudnn_lib_dir() {
  if [ -n "${AXON_CUDNN_LIB_DIR:-}" ] && [ -e "${AXON_CUDNN_LIB_DIR}/libcudnn.so.9" ]; then
    echo "${AXON_CUDNN_LIB_DIR}"
    return 0
  fi

  local candidate
  for candidate in \
    /usr/local/lib/ollama/mlx_cuda_v13 \
    /usr/local/cuda/lib64 \
    /usr/lib/wsl/lib
  do
    if [ -e "$candidate/libcudnn.so.9" ]; then
      echo "$candidate"
      return 0
    fi
  done

  return 1
}

resolve_host_cuda_library() {
  local lib_name="$1"
  local candidate

  for candidate in \
    "/usr/local/lib/ollama/mlx_cuda_v13/$lib_name" \
    "/lib/x86_64-linux-gnu/$lib_name" \
    "/usr/lib/x86_64-linux-gnu/$lib_name" \
    "/usr/lib/wsl/lib/$lib_name"
  do
    if [ -e "$candidate" ]; then
      echo "$candidate"
      return 0
    fi
  done

  return 1
}

if [ "${AXON_EMBEDDING_BACKEND:-auto}" = "cuda" ]; then
  cudnn_lib_dir="$(resolve_cudnn_lib_dir || true)"
  if [ -z "$cudnn_lib_dir" ]; then
    echo "❌ AXON_EMBEDDING_BACKEND=cuda mais aucune libcudnn.so.9 exploitable n'a été trouvée." >&2
    echo "   Définissez AXON_CUDNN_LIB_DIR ou installez une cuDNN runtime visible." >&2
    exit 1
  fi

  cuda_runtime_dir="$(pwd)/.axon/cuda-runtime"
  mkdir -p "$cuda_runtime_dir"

  required_cuda_libs=(
    libcudnn.so.9
    libcublasLt.so.12
    libcublas.so.12
    libcurand.so.10
    libcufft.so.11
    libcudart.so.12
  )

  for lib_name in "${required_cuda_libs[@]}"; do
    lib_source="$(resolve_host_cuda_library "$lib_name" || true)"
    if [ -z "$lib_source" ]; then
      echo "❌ AXON_EMBEDDING_BACKEND=cuda mais la lib runtime requise est absente: $lib_name" >&2
      exit 1
    fi
    ln -sf "$lib_source" "$cuda_runtime_dir/$lib_name"
  done

  export AXON_CUDA_RUNTIME_LIB_PATH="$cuda_runtime_dir"
fi

if [ "$#" -eq 0 ]; then
  exec devenv shell
fi

printf -v quoted_command "%q " "$@"
shell_prelude='if [ -n "${AXON_CUDA_RUNTIME_LIB_PATH:-}" ]; then export LD_LIBRARY_PATH="${LD_LIBRARY_PATH:+$LD_LIBRARY_PATH:}${AXON_CUDA_RUNTIME_LIB_PATH}"; fi;'
exec devenv shell -- bash -lc "${shell_prelude} ${quoted_command}"
