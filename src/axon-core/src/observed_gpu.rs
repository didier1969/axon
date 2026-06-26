//! DEC-AXO-901626 — observable GPU compute derivation.
//!
//! The brain composer must answer "is the embedder really using the GPU?"
//! WITHOUT trusting a self-reported, race-prone provider slot. The NVIDIA
//! driver (via the NVML API) knows exactly which pids hold a GPU compute
//! context and how much VRAM each holds; cross-referencing that with the
//! indexer's published pid yields a binary GPU/CPU answer that cannot be
//! lied to by in-process init-order races.
//!
//! TMG-AXO-002 — GPU telemetry is NVML-only (operator directive s84: the
//! `nvidia-smi` CLI is too imprecise and is retired). Everything here is
//! **best-effort, never blocking**: NVML may be absent (CPU-only host) or the
//! pid may hold no compute context. In every such case the probe returns
//! `None`; the caller falls back to the PG throughput counter
//! (`embedder_observed_state`).

/// A resident embedding model (BGE-Large ≈ 1–2 GiB) pushes device VRAM well
/// above the idle-driver baseline; this threshold separates "model loaded on
/// GPU" from "nothing on GPU". Override via `AXON_EMBEDDER_GPU_RESIDENT_MIN_MIB`.
pub const EMBEDDER_GPU_RESIDENT_MIN_MIB: u64 = 512;

fn embedder_gpu_resident_min_mib() -> u64 {
    std::env::var("AXON_EMBEDDER_GPU_RESIDENT_MIN_MIB")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(EMBEDDER_GPU_RESIDENT_MIN_MIB)
}

/// DEC-AXO-901626 — the embedder-owning process (indexer) observes whether a
/// model is resident on the GPU and publishes the binary verdict to PG.
///
/// Reads **device-level** VRAM through the precise `gpu_telemetry` path (NVML
/// driver API, or `nvidia-smi --query-gpu` device-level). This is WSL2-robust:
/// WSL2 masks per-process `--query-compute-apps` memory as `[N/A]`, but
/// device-level memory reports correctly. Returns `(compute, compute_source)`:
///   * device VRAM ≥ resident threshold → `("GPU", "device_vram")`
///   * device VRAM below threshold       → `("CPU", "device_vram")`
///   * GPU telemetry unavailable          → `("CPU", "unknown")`
pub fn observed_self_compute() -> (&'static str, &'static str) {
    match crate::embedder::current_gpu_memory_snapshot() {
        Some(snapshot) if snapshot.used_mb >= embedder_gpu_resident_min_mib() => {
            ("GPU", "device_vram")
        }
        Some(_) => ("CPU", "device_vram"),
        None => ("CPU", "unknown"),
    }
}

/// VRAM (MiB) held by `pid`. `Some(mib)` with `mib > 0` is the canonical
/// "this pid is doing GPU compute" signal; `None` when no GPU telemetry is
/// available or the pid holds no compute context.
///
/// REQ-AXO-902037 / TMG-AXO-002 — the NVML driver API is the SOLE source
/// (precise + WSL2-robust: NVML reports per-process memory where the retired
/// `nvidia-smi --query-compute-apps` CLI masked it as `[N/A]`/`[Not Found]`).
/// `None` when NVML is unavailable (no driver lib) — never a CLI fallback.
pub fn observed_gpu_used_mib(pid: u32) -> Option<u64> {
    crate::embedder::gpu_process_used_mib_via_nvml(pid)
}
