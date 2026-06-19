//! DEC-AXO-901626 — observable GPU compute derivation.
//!
//! The brain composer must answer "is the embedder really using the GPU?"
//! WITHOUT trusting a self-reported, race-prone provider slot. The NVIDIA
//! driver knows exactly which pids hold a GPU compute context and how much
//! VRAM each holds; cross-referencing `nvidia-smi --query-compute-apps`
//! with the indexer's published pid yields a binary GPU/CPU answer that
//! cannot be lied to by in-process init-order races.
//!
//! Everything here is **best-effort, never blocking**: nvidia-smi may be
//! absent (CPU-only host), unreachable (brain and indexer on different
//! hosts), or slow/masked (WSL2 reports `[Not Found]` for process_name).
//! In every such case the probe returns `None`; the caller falls back to
//! the PG throughput counter (`embedder_observed_state`).

use std::sync::mpsc;
use std::time::Duration;

/// Upper bound on a single nvidia-smi probe. The indexer observes itself on
/// its heartbeat tick (~5 s); a hung `nvidia-smi` must never stall the
/// publisher. On timeout the probe yields `None` (→ compute "unknown").
pub const NVIDIA_SMI_PROBE_TIMEOUT_MS: u64 = 750;

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
/// REQ-AXO-902037 — prefer the NVML driver API (precise + WSL2-robust: NVML
/// reports per-process memory where `nvidia-smi --query-compute-apps` masks
/// it as `[N/A]`/`[Not Found]`). The `nvidia-smi` shell-out is now an
/// explicit FALLBACK, used only when NVML is unavailable (no driver lib).
pub fn observed_gpu_used_mib(pid: u32) -> Option<u64> {
    if let Some(mib) = crate::embedder::gpu_process_used_mib_via_nvml(pid) {
        return Some(mib);
    }
    let stdout = run_compute_apps_query()?;
    parse_compute_apps_used_mib(&stdout, pid)
}

/// Shell out to nvidia-smi with a bounded wait. Returns the raw CSV stdout
/// on success, `None` on any failure (binary missing, non-zero exit,
/// timeout). The probe runs on a detached thread so a hung process cannot
/// block the caller past `NVIDIA_SMI_PROBE_TIMEOUT_MS`.
fn run_compute_apps_query() -> Option<String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let result = std::process::Command::new("nvidia-smi")
            .args([
                "--query-compute-apps=pid,used_memory",
                "--format=csv,noheader,nounits",
            ])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).into_owned());
        // Receiver may have already timed out and dropped; ignore send error.
        let _ = tx.send(result);
    });
    match rx.recv_timeout(Duration::from_millis(NVIDIA_SMI_PROBE_TIMEOUT_MS)) {
        Ok(inner) => inner,
        Err(_) => None,
    }
}

/// Pure parser for the `pid,used_memory` CSV (no header, no units). Returns
/// the MiB held by the first row matching `pid`. Extracted so the matching
/// logic is unit-tested without a live GPU.
pub fn parse_compute_apps_used_mib(stdout: &str, pid: u32) -> Option<u64> {
    let pid_str = pid.to_string();
    for line in stdout.lines() {
        let mut parts = line.splitn(2, ',').map(str::trim);
        let line_pid = parts.next()?;
        let mib_str = match parts.next() {
            Some(value) => value,
            None => continue,
        };
        if line_pid == pid_str {
            if let Ok(mib) = mib_str.parse::<u64>() {
                return Some(mib);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::parse_compute_apps_used_mib;

    #[test]
    fn parses_used_memory_for_matching_pid() {
        let csv = "1234, 0\n7865, 1993\n9999, 512";
        assert_eq!(parse_compute_apps_used_mib(csv, 7865), Some(1993));
    }

    #[test]
    fn returns_none_when_pid_absent() {
        let csv = "1234, 0\n9999, 512";
        assert_eq!(parse_compute_apps_used_mib(csv, 7865), None);
    }

    #[test]
    fn returns_zero_mib_when_pid_present_but_idle() {
        // A pid with a compute context but zero VRAM is still "present";
        // the GPU/CPU verdict (mib > 0) is the caller's job, not the parser's.
        let csv = "7865, 0";
        assert_eq!(parse_compute_apps_used_mib(csv, 7865), Some(0));
    }

    #[test]
    fn empty_output_yields_none() {
        assert_eq!(parse_compute_apps_used_mib("", 7865), None);
    }

    #[test]
    fn tolerates_blank_and_malformed_lines() {
        let csv = "\n[Not Found]\n7865, 2048\n";
        assert_eq!(parse_compute_apps_used_mib(csv, 7865), Some(2048));
    }

    #[test]
    fn ignores_non_numeric_memory() {
        let csv = "7865, [Insufficient Permissions]";
        assert_eq!(parse_compute_apps_used_mib(csv, 7865), None);
    }
}
