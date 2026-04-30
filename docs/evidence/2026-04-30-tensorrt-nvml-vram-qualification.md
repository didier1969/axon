# TensorRT NVML VRAM Qualification

Date: 2026-04-30
Project: Axon
Requirement: REQ-AXO-038

## Finding

`nvidia-smi` is not precise enough to be the primary qualification signal for TensorRT VRAM control. TensorRT qualification now uses direct NVML telemetry first and keeps `nvidia-smi` only as a fallback when NVML is unavailable.

## Decision

The TensorRT qualification contract is:

- Primary VRAM source: NVML through `libnvidia-ml.so.1`.
- Runtime TensorRT telemetry backend: `AXON_GPU_TELEMETRY_BACKEND=nvml`.
- Fallback: `nvidia-smi`, explicitly marked as fallback in the sample payload.
- Hard overshoot rule on 8 GB cards: `memory_used_mb >= 7900` fails qualification and stops the dev runtime.
- Conservative 8 GB TensorRT default envelope: `AXON_OPT_MAX_VRAM_USED_MB=2048`, `AXON_CUDA_MEMORY_LIMIT_MB=1024`, `AXON_GPU_PRIMARY_WORKER_MAX_USED_MB=1536`.

## Evidence

Validation command:

```bash
./scripts/axon --instance dev qualify-dev-indexer-tensorrt-cold --duration 20 --interval 5 --label tensorrt-nvml-smoke
```

Observed artifact:

```text
.axon/qualification-runs/2026-04-30T07-17-23-indexer_full-tensorrt-nvml-smoke/summary.json
```

Key values:

```text
measurement_contract=nvml_primary_nvidia_smi_fallback
gpu_telemetry_backend=nvml
first_gpu_source=nvml
first_gpu_library=/usr/lib/wsl/lib/libnvidia-ml.so.1
max_gpu_used_mb=2135
overshoot_fail_mb=7900
```

## Consequence

TensorRT can still be qualified on the 8 GB machine, but promotion must use NVML-backed measurements and must not rely on `nvidia-smi` alone. The current conservative envelope avoids the previously observed overshoot while preserving the TensorRT provider path for further tuning.
