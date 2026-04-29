use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use libloading::Library;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpuMemorySnapshot {
    pub total_mb: u64,
    pub used_mb: u64,
    pub free_mb: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GpuUtilizationSnapshot {
    pub gpu_utilization_ratio: f64,
    pub memory_utilization_ratio: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GpuTelemetryBackend {
    None,
    Nvml,
    NvidiaSmi,
}

#[derive(Debug, Clone, Copy)]
struct CachedGpuMemorySnapshot {
    captured_at: Instant,
    snapshot: Option<GpuMemorySnapshot>,
}

static GPU_MEMORY_SNAPSHOT_CACHE: OnceLock<Mutex<Option<CachedGpuMemorySnapshot>>> =
    OnceLock::new();

fn gpu_memory_snapshot_cache_slot() -> &'static Mutex<Option<CachedGpuMemorySnapshot>> {
    GPU_MEMORY_SNAPSHOT_CACHE.get_or_init(|| Mutex::new(None))
}

fn gpu_telemetry_backend() -> GpuTelemetryBackend {
    match std::env::var("AXON_GPU_TELEMETRY_BACKEND")
        .ok()
        .map(|value| value.trim().to_ascii_lowercase())
        .as_deref()
    {
        Some("none") | Some("disabled") => GpuTelemetryBackend::None,
        Some("nvml") => GpuTelemetryBackend::Nvml,
        _ => GpuTelemetryBackend::NvidiaSmi,
    }
}

pub(crate) fn gpu_telemetry_backend_name() -> &'static str {
    match gpu_telemetry_backend() {
        GpuTelemetryBackend::None => "none",
        GpuTelemetryBackend::Nvml => "nvml",
        GpuTelemetryBackend::NvidiaSmi => "nvidia-smi",
    }
}

pub(crate) fn gpu_telemetry_command() -> String {
    std::env::var("AXON_GPU_TELEMETRY_COMMAND")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "/usr/lib/wsl/lib/nvidia-smi".to_string())
}

pub(crate) fn nvml_library_path() -> String {
    std::env::var("AXON_NVML_LIBRARY_PATH")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "/usr/lib/wsl/lib/libnvidia-ml.so.1".to_string())
}

pub fn gpu_telemetry_device_index() -> u32 {
    std::env::var("AXON_GPU_TELEMETRY_DEVICE_INDEX")
        .ok()
        .and_then(|value| value.trim().parse::<u32>().ok())
        .unwrap_or(0)
}

pub fn gpu_telemetry_cache_ttl_ms() -> u64 {
    std::env::var("AXON_GPU_TELEMETRY_CACHE_TTL_MS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value >= 100)
        .unwrap_or(2_000)
}

pub(crate) fn parse_nvidia_smi_memory_csv(line: &str) -> Option<GpuMemorySnapshot> {
    let mut parts = line.split(',').map(|part| part.trim().parse::<u64>().ok());
    let total_mb = parts.next()??;
    let used_mb = parts.next()??;
    let free_mb = parts.next()??;
    Some(GpuMemorySnapshot {
        total_mb,
        used_mb,
        free_mb,
    })
}

pub(crate) fn parse_nvidia_smi_utilization_csv(line: &str) -> Option<GpuUtilizationSnapshot> {
    let mut parts = line.split(',').map(|part| part.trim().parse::<f64>().ok());
    let gpu_utilization = parts.next()??;
    let memory_utilization = parts.next()??;
    Some(GpuUtilizationSnapshot {
        gpu_utilization_ratio: (gpu_utilization / 100.0).clamp(0.0, 1.0),
        memory_utilization_ratio: (memory_utilization / 100.0).clamp(0.0, 1.0),
    })
}

#[repr(C)]
struct NvmlMemoryInfo {
    total: u64,
    free: u64,
    used: u64,
}

#[repr(C)]
struct NvmlUtilizationInfo {
    gpu: u32,
    memory: u32,
}

fn current_gpu_memory_snapshot_via_nvml() -> Option<GpuMemorySnapshot> {
    type NvmlInitV2 = unsafe extern "C" fn() -> i32;
    type NvmlShutdown = unsafe extern "C" fn() -> i32;
    type NvmlDeviceGetHandleByIndexV2 =
        unsafe extern "C" fn(u32, *mut *mut std::ffi::c_void) -> i32;
    type NvmlDeviceGetMemoryInfo =
        unsafe extern "C" fn(*mut std::ffi::c_void, *mut NvmlMemoryInfo) -> i32;

    const NVML_SUCCESS: i32 = 0;

    unsafe {
        let library = Library::new(nvml_library_path()).ok()?;
        let nvml_init: libloading::Symbol<'_, NvmlInitV2> = library.get(b"nvmlInit_v2").ok()?;
        let nvml_shutdown: libloading::Symbol<'_, NvmlShutdown> =
            library.get(b"nvmlShutdown").ok()?;
        let nvml_device_get_handle_by_index: libloading::Symbol<'_, NvmlDeviceGetHandleByIndexV2> =
            library.get(b"nvmlDeviceGetHandleByIndex_v2").ok()?;
        let nvml_device_get_memory_info: libloading::Symbol<'_, NvmlDeviceGetMemoryInfo> =
            library.get(b"nvmlDeviceGetMemoryInfo").ok()?;

        if nvml_init() != NVML_SUCCESS {
            return None;
        }

        let mut device: *mut std::ffi::c_void = std::ptr::null_mut();
        if nvml_device_get_handle_by_index(gpu_telemetry_device_index(), &mut device)
            != NVML_SUCCESS
        {
            let _ = nvml_shutdown();
            return None;
        }

        let mut memory = NvmlMemoryInfo {
            total: 0,
            free: 0,
            used: 0,
        };
        let result = if nvml_device_get_memory_info(device, &mut memory) == NVML_SUCCESS {
            Some(GpuMemorySnapshot {
                total_mb: memory.total / (1024 * 1024),
                used_mb: memory.used / (1024 * 1024),
                free_mb: memory.free / (1024 * 1024),
            })
        } else {
            None
        };
        let _ = nvml_shutdown();
        result
    }
}

fn current_gpu_utilization_snapshot_via_nvml() -> Option<GpuUtilizationSnapshot> {
    type NvmlInitV2 = unsafe extern "C" fn() -> i32;
    type NvmlShutdown = unsafe extern "C" fn() -> i32;
    type NvmlDeviceGetHandleByIndexV2 =
        unsafe extern "C" fn(u32, *mut *mut std::ffi::c_void) -> i32;
    type NvmlDeviceGetUtilizationRates =
        unsafe extern "C" fn(*mut std::ffi::c_void, *mut NvmlUtilizationInfo) -> i32;

    const NVML_SUCCESS: i32 = 0;

    unsafe {
        let library = Library::new(nvml_library_path()).ok()?;
        let nvml_init: libloading::Symbol<'_, NvmlInitV2> = library.get(b"nvmlInit_v2").ok()?;
        let nvml_shutdown: libloading::Symbol<'_, NvmlShutdown> =
            library.get(b"nvmlShutdown").ok()?;
        let nvml_device_get_handle_by_index: libloading::Symbol<'_, NvmlDeviceGetHandleByIndexV2> =
            library.get(b"nvmlDeviceGetHandleByIndex_v2").ok()?;
        let nvml_device_get_utilization_rates: libloading::Symbol<
            '_,
            NvmlDeviceGetUtilizationRates,
        > = library.get(b"nvmlDeviceGetUtilizationRates").ok()?;

        if nvml_init() != NVML_SUCCESS {
            return None;
        }

        let mut device: *mut std::ffi::c_void = std::ptr::null_mut();
        if nvml_device_get_handle_by_index(gpu_telemetry_device_index(), &mut device)
            != NVML_SUCCESS
        {
            let _ = nvml_shutdown();
            return None;
        }

        let mut utilization = NvmlUtilizationInfo { gpu: 0, memory: 0 };
        let result = if nvml_device_get_utilization_rates(device, &mut utilization) == NVML_SUCCESS
        {
            Some(GpuUtilizationSnapshot {
                gpu_utilization_ratio: (utilization.gpu as f64 / 100.0).clamp(0.0, 1.0),
                memory_utilization_ratio: (utilization.memory as f64 / 100.0).clamp(0.0, 1.0),
            })
        } else {
            None
        };
        let _ = nvml_shutdown();
        result
    }
}

pub fn current_gpu_memory_snapshot() -> Option<GpuMemorySnapshot> {
    let cache_ttl = Duration::from_millis(gpu_telemetry_cache_ttl_ms());
    let cache = gpu_memory_snapshot_cache_slot();
    let now = Instant::now();
    {
        let guard = cache.lock().unwrap_or_else(|poison| poison.into_inner());
        if let Some(cached) = *guard {
            if now.duration_since(cached.captured_at) <= cache_ttl {
                return cached.snapshot;
            }
        }
    }

    let snapshot = match gpu_telemetry_backend() {
        GpuTelemetryBackend::None => None,
        GpuTelemetryBackend::Nvml => current_gpu_memory_snapshot_via_nvml(),
        GpuTelemetryBackend::NvidiaSmi => std::process::Command::new(gpu_telemetry_command())
            .args([
                "--query-gpu=memory.total,memory.used,memory.free",
                "--format=csv,noheader,nounits",
            ])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .and_then(|stdout| {
                stdout
                    .lines()
                    .find(|line| !line.trim().is_empty())
                    .map(str::to_string)
            })
            .and_then(|line| parse_nvidia_smi_memory_csv(&line)),
    };

    let mut guard = cache.lock().unwrap_or_else(|poison| poison.into_inner());
    *guard = Some(CachedGpuMemorySnapshot {
        captured_at: now,
        snapshot,
    });
    snapshot
}

pub fn current_gpu_utilization_snapshot() -> Option<GpuUtilizationSnapshot> {
    match gpu_telemetry_backend() {
        GpuTelemetryBackend::None => None,
        GpuTelemetryBackend::Nvml => current_gpu_utilization_snapshot_via_nvml(),
        GpuTelemetryBackend::NvidiaSmi => std::process::Command::new(gpu_telemetry_command())
            .args([
                "--query-gpu=utilization.gpu,utilization.memory",
                "--format=csv,noheader,nounits",
            ])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .and_then(|stdout| {
                stdout
                    .lines()
                    .find(|line| !line.trim().is_empty())
                    .map(str::to_string)
            })
            .and_then(|line| parse_nvidia_smi_utilization_csv(&line)),
    }
}
