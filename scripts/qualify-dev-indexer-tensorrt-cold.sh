#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

duration="120"
interval="5"
label="dev-indexer-tensorrt-cold"
cuda_package_set="${AXON_CUDA_PACKAGE_SET:-cudaPackages}"
cuda_package_label="${cuda_package_set//_/-}"
manifest_path="${AXON_ORT_ARTIFACT_MANIFEST:-$PROJECT_ROOT/.axon/ort-artifacts/onnxruntime-tensorrt-${cuda_package_label}/current.json}"
max_vram_used_mb="${AXON_TENSORRT_QUALIFY_MAX_VRAM_USED_MB:-6144}"
gpu_admission_vram_used_mb="${AXON_TENSORRT_QUALIFY_GPU_ADMISSION_VRAM_USED_MB:-}"
tensorrt_workspace_mb="${AXON_TENSORRT_QUALIFY_WORKSPACE_MB:-}"
extra_args=()

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
        --manifest)
            manifest_path="$2"
            shift 2
            ;;
        --manifest=*)
            manifest_path="${1#*=}"
            shift
            ;;
        --max-vram-used-mb)
            max_vram_used_mb="$2"
            shift 2
            ;;
        --max-vram-used-mb=*)
            max_vram_used_mb="${1#*=}"
            shift
            ;;
        --gpu-admission-vram-used-mb)
            gpu_admission_vram_used_mb="$2"
            shift 2
            ;;
        --gpu-admission-vram-used-mb=*)
            gpu_admission_vram_used_mb="${1#*=}"
            shift
            ;;
        --tensorrt-workspace-mb)
            tensorrt_workspace_mb="$2"
            shift 2
            ;;
        --tensorrt-workspace-mb=*)
            tensorrt_workspace_mb="${1#*=}"
            shift
            ;;
        --help|-h)
            cat <<'EOF'
Usage: bash scripts/qualify-dev-indexer-tensorrt-cold.sh [options] [extra qualify args...]

Runs an indexer-only cold qualification with the hard-cut TensorRT GPU service:
- requires an ONNX Runtime TensorRT manifest to exist locally
- forces the dedicated GPU service on
- forces TensorRT as the nominal embedding provider for that service
- then delegates to qualify-dev-indexer-cold.sh

Options:
  --duration N        Qualification duration in seconds (default: 120)
  --interval N        Sample interval in seconds (default: 5)
  --label NAME        Qualification label (default: dev-indexer-tensorrt-cold)
  --manifest PATH     Explicit TensorRT artifact manifest path
  --max-vram-used-mb N
                     Operator VRAM budget in MB (default: 6144)
  --gpu-admission-vram-used-mb N
                     Maximum already-used VRAM before GPU batch admission
                     (default: budget minus max(10%, 512 MiB))
  --tensorrt-workspace-mb N
                     TensorRT workspace/memory-pool cap in MB
                     (default: budget minus 512 MiB)
EOF
            exit 0
            ;;
        *)
            extra_args+=("$1")
            shift
            ;;
    esac
done

manifest_path="$(realpath -m "$manifest_path")"

case "$max_vram_used_mb" in
    ''|*[!0-9]*)
        echo "❌ --max-vram-used-mb must be a positive integer, got: $max_vram_used_mb" >&2
        exit 1
        ;;
esac
if (( max_vram_used_mb < 1024 )); then
    echo "❌ --max-vram-used-mb must be at least 1024 MiB, got: $max_vram_used_mb" >&2
    exit 1
fi

if [[ -z "$gpu_admission_vram_used_mb" ]]; then
    reserve_mb=$(( max_vram_used_mb / 10 ))
    if (( reserve_mb < 512 )); then
        reserve_mb=512
    fi
    gpu_admission_vram_used_mb=$(( max_vram_used_mb - reserve_mb ))
fi
case "$gpu_admission_vram_used_mb" in
    ''|*[!0-9]*)
        echo "❌ --gpu-admission-vram-used-mb must be a positive integer, got: $gpu_admission_vram_used_mb" >&2
        exit 1
        ;;
esac
if (( gpu_admission_vram_used_mb >= max_vram_used_mb )); then
    echo "❌ --gpu-admission-vram-used-mb must stay below --max-vram-used-mb" >&2
    exit 1
fi

if [[ -z "$tensorrt_workspace_mb" ]]; then
    tensorrt_workspace_mb=$(( max_vram_used_mb > 512 ? max_vram_used_mb - 512 : max_vram_used_mb ))
fi
case "$tensorrt_workspace_mb" in
    ''|*[!0-9]*)
        echo "❌ --tensorrt-workspace-mb must be a positive integer, got: $tensorrt_workspace_mb" >&2
        exit 1
        ;;
esac
if (( tensorrt_workspace_mb < 512 )); then
    echo "❌ --tensorrt-workspace-mb must be at least 512 MiB, got: $tensorrt_workspace_mb" >&2
    exit 1
fi
if (( tensorrt_workspace_mb > max_vram_used_mb )); then
    echo "❌ --tensorrt-workspace-mb must not exceed --max-vram-used-mb" >&2
    exit 1
fi

if [[ ! -f "$manifest_path" ]]; then
    echo "❌ TensorRT manifest not found: $manifest_path" >&2
    echo "Build it first with: bash scripts/build_ort_tensorrt_artifact.sh" >&2
    exit 1
fi

python3 - "$manifest_path" <<'PY'
import json
import sys
from pathlib import Path

manifest_path = Path(sys.argv[1])
payload = json.loads(manifest_path.read_text())
provider = payload.get("provider")
if provider != "tensorrt":
    raise SystemExit(f"manifest provider must be 'tensorrt', found: {provider!r}")
PY
python3 "$SCRIPT_DIR/lib/validate_ort_manifest.py" "$manifest_path"

export AXON_ORT_ARTIFACT_MANIFEST="$manifest_path"
export AXON_GPU_EMBED_SERVICE_ENABLED=1
export AXON_GPU_EMBED_SERVICE_RECYCLE_EVERY_BATCH=0
export AXON_GPU_EMBED_SERVICE_TENSORRT=1
export AXON_OPT_MAX_VRAM_USED_MB="$max_vram_used_mb"
export AXON_CUDA_MEMORY_SOFT_LIMIT_MB="$max_vram_used_mb"
export AXON_CUDA_MEMORY_LIMIT_MB="$tensorrt_workspace_mb"
export AXON_GPU_PRIMARY_WORKER_MAX_USED_MB="$gpu_admission_vram_used_mb"
export AXON_GPU_TELEMETRY_CACHE_TTL_MS="${AXON_GPU_TELEMETRY_CACHE_TTL_MS:-250}"

echo "🔒 TensorRT VRAM envelope"
echo "   max_vram_used_mb          : $AXON_OPT_MAX_VRAM_USED_MB"
echo "   gpu_admission_vram_used_mb: $AXON_GPU_PRIMARY_WORKER_MAX_USED_MB"
echo "   tensorrt_workspace_mb     : $AXON_CUDA_MEMORY_LIMIT_MB"

exec bash "$SCRIPT_DIR/qualify-dev-indexer-cold.sh" \
    --duration "$duration" \
    --interval "$interval" \
    --label "$label" \
    --include-rich-mcp-diagnostics \
    "${extra_args[@]}"
