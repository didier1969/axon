# Session 77 — B2 throughput experiment (batch size + multi-lane)

Branch `experiment/b2-throughput-tuning`. Goal: can we push GPU embed past ~158 ch/s via bigger batch or 2 B2 lanes? Operator suspected lanes had contention issues — confirm whether it was bad development or physics.

## Setup
RTX 3070 Laptop (8 GB, SM86), BGE-large FP16, TensorRT EP. Live indexer stopped for exclusive GPU. Bench `axon-bench-pipeline-v2 --source src/axon-core/src --gpu`, 70 s sustained, 30 s warmup, against the populated dev DB.

## Result — multi-lane does NOT help (contention is physics, not a bug)

| Config | B2 embed | GPU util (t_work_ratio) |
|---|---|---|
| batch=64 **workers=1** | ~210 ch/s | **99.47 %** |
| batch=64 **workers=2** | ~159 ch/s | **99.52 %** |

Both configs **saturate the GPU at ~99.5 %**. A 2nd ORT session on a single GPU just contends for CUDA kernels — no throughput gain (if anything, per-lane overhead drops it). This is not "mal développé": a GPU already at 99.5 % cannot go faster by adding feeder lanes. Confirms the operator's prior experience.

(End-to-end numbers diverge — w1 27.75 vs w2 157.76 ch/s — due to the bench's **inline feeder re-feeding in-flight chunks** → B3 persist errors, a bench artifact. Production `spawn_vector_sorted_drain` bounds this via channel backpressure + the `embed_status='pending'` guard.)

## batch > 64 — NOT pursued (low value + high risk)
- The GPU is already saturated at batch=64, so a bigger batch can at best shave per-launch overhead (marginal at 99.5 %).
- **The TRT engine cache filename is keyed by the graph hash, NOT the profile** (`axon-bge-large_<hash>_0_fp16_sm86.engine`). Building a batch>64 engine would **overwrite the live default engine** → live breaks on next restart. Not worth it for a marginal/likely-zero gain.

## TRT cache rebuild gotcha (cost us a detour)
Clearing the engine cache breaks TRT until rebuilt, and the rebuild needs the **builder lib** `libnvinfer_builder_resource_sm86.so` which lives in `tensorrt_lib_dir` (from the ORT manifest `.axon/ort-artifacts/.../current.json`), NOT in the ORT lib dir. The CLAUDE.md bench recipe omits it (fine for *loading* a cached engine, fatal for *rebuilding*). Correct env for a rebuild:
```
export TRT_LIB_DIR=$(jq -r .tensorrt_lib_dir .axon/ort-artifacts/onnxruntime-tensorrt-cudaPackages/current.json)
export LD_LIBRARY_PATH=/usr/lib/wsl/lib:$(dirname $ORT_DYLIB_PATH):$TRT_LIB_DIR:$LD_LIBRARY_PATH
```
The default engine cache was restored (680 MB) after the detour; live is safe.

## Conclusion
The complexity removal (session 77 main) already unlocked the GPU's full saturation: **~35 ch/s @ 1 % → ~158-210 ch/s @ 99.5 %**. Beyond that, the GPU is the wall. Higher throughput requires a **different lever, not more lanes/batch**:
- Faster GPU (the 3070 laptop is the constraint).
- **INT8/quantized BGE** (FP16→INT8 ≈ 1.5-2× on tensor cores) — biggest realistic win.
- Shorter/fewer chunks (chunking strategy) — reduces tokens to embed.

Recommendation: do not invest further in batch/lane tuning. The branch can be discarded (no code change improved throughput); the finding is recorded here.
