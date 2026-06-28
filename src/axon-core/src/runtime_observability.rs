use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};

#[derive(Debug, Clone, Copy, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ProcessMemorySnapshot {
    pub rss_bytes: u64,
    pub rss_anon_bytes: u64,
    pub rss_file_bytes: u64,
    pub rss_shmem_bytes: u64,
}

// ── REQ-AXO-902152 — VM-aggregate memory-pressure signal (host-safety co-tenant) ──────────────
// The OOM class is AGGREGATE WSL-cap saturation, NOT a mono-process Axon leak (Axon stays ~3.4 GB).
// So per-process RSS vs its own 14 GB cap never fires. This signal is host-wide (MemAvailable), so
// the brain AND the indexer recede TOGETHER with NO IPC — the host floor is the implicit coordinator.
// Written by the memory watchdog (binary, main_background); read by pipeline-A intake (lib, stage_a1)
// which throttles new file intake under CRITICAL pressure instead of piling on toward a host freeze.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryPressure {
    Normal,
    Elevated,
    Critical,
}

impl MemoryPressure {
    fn as_u8(self) -> u8 {
        match self {
            MemoryPressure::Normal => 0,
            MemoryPressure::Elevated => 1,
            MemoryPressure::Critical => 2,
        }
    }

    fn from_u8(value: u8) -> Self {
        match value {
            0 => MemoryPressure::Normal,
            1 => MemoryPressure::Elevated,
            _ => MemoryPressure::Critical,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            MemoryPressure::Normal => "normal",
            MemoryPressure::Elevated => "elevated",
            MemoryPressure::Critical => "critical",
        }
    }
}

static MEMORY_PRESSURE_LEVEL: AtomicU8 = AtomicU8::new(0);
static MEMORY_BACKPRESSURE_PAUSES_TOTAL: AtomicU64 = AtomicU64::new(0);

/// Publish the current VM-aggregate memory-pressure level (called by the watchdog).
pub fn set_memory_pressure(level: MemoryPressure) {
    MEMORY_PRESSURE_LEVEL.store(level.as_u8(), Ordering::Relaxed);
}

/// Read the current memory-pressure level (called by the pipeline-A intake + reclaimer).
pub fn current_memory_pressure() -> MemoryPressure {
    MemoryPressure::from_u8(MEMORY_PRESSURE_LEVEL.load(Ordering::Relaxed))
}

/// Observability: total number of A1 intake pauses applied under CRITICAL pressure.
pub fn memory_backpressure_pauses_total() -> u64 {
    MEMORY_BACKPRESSURE_PAUSES_TOTAL.load(Ordering::Relaxed)
}

/// Record one A1 intake backoff tick (called from the pipeline-A intake gate).
pub fn record_memory_backpressure_pause() {
    MEMORY_BACKPRESSURE_PAUSES_TOTAL.fetch_add(1, Ordering::Relaxed);
}

/// Host-wide allocatable memory (Linux `/proc/meminfo` `MemAvailable`). `None` if unreadable
/// (non-Linux, or read failure) → callers treat unknown as "no host pressure" (per-process still applies).
pub fn mem_available_bytes() -> Option<u64> {
    let content = std::fs::read_to_string("/proc/meminfo").ok()?;
    parse_mem_available_bytes(&content)
}

pub fn parse_mem_available_bytes(content: &str) -> Option<u64> {
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("MemAvailable:") {
            return rest
                .split_whitespace()
                .next()
                .and_then(|value| value.parse::<u64>().ok())
                .map(|kb| kb.saturating_mul(1024));
        }
    }
    None
}

/// Pure classifier (testable, no I/O). `Critical` when the host allocatable floor is breached OR a
/// process blew its own per-process cap; `Elevated` in the warning band (host < 2×floor, or RSS > 85%
/// of cap); else `Normal`. Unknown `mem_available` contributes no host pressure.
pub fn classify_memory_pressure(
    rss_bytes: u64,
    rss_limit_bytes: u64,
    mem_available_bytes: Option<u64>,
    vm_floor_bytes: u64,
) -> MemoryPressure {
    let process_critical = rss_limit_bytes > 0 && rss_bytes > rss_limit_bytes;
    let host_critical = mem_available_bytes
        .map(|available| available < vm_floor_bytes)
        .unwrap_or(false);
    if process_critical || host_critical {
        return MemoryPressure::Critical;
    }

    let process_elevated =
        rss_limit_bytes > 0 && (rss_bytes as f64) > (rss_limit_bytes as f64) * 0.85;
    let host_elevated = mem_available_bytes
        .map(|available| available < vm_floor_bytes.saturating_mul(2))
        .unwrap_or(false);
    if process_elevated || host_elevated {
        return MemoryPressure::Elevated;
    }

    MemoryPressure::Normal
}

pub fn process_memory_snapshot() -> ProcessMemorySnapshot {
    let snapshot = std::fs::read_to_string("/proc/self/status")
        .ok()
        .map(|content| parse_proc_status_kb(&content))
        .unwrap_or_default();

    if snapshot.rss_bytes > 0 {
        return snapshot;
    }

    ProcessMemorySnapshot {
        rss_bytes: read_statm_rss_bytes().unwrap_or_default(),
        ..snapshot
    }
}

#[cfg(target_os = "linux")]
pub fn malloc_trim_system_allocator() -> bool {
    unsafe extern "C" {
        fn malloc_trim(pad: usize) -> i32;
    }

    unsafe { malloc_trim(0) != 0 }
}

#[cfg(not(target_os = "linux"))]
pub fn malloc_trim_system_allocator() -> bool {
    false
}

pub fn parse_proc_status_kb(content: &str) -> ProcessMemorySnapshot {
    let mut snapshot = ProcessMemorySnapshot::default();

    for line in content.lines() {
        if let Some(value_kb) = parse_status_line_kb(line, "VmRSS:") {
            snapshot.rss_bytes = value_kb.saturating_mul(1024);
        } else if let Some(value_kb) = parse_status_line_kb(line, "RssAnon:") {
            snapshot.rss_anon_bytes = value_kb.saturating_mul(1024);
        } else if let Some(value_kb) = parse_status_line_kb(line, "RssFile:") {
            snapshot.rss_file_bytes = value_kb.saturating_mul(1024);
        } else if let Some(value_kb) = parse_status_line_kb(line, "RssShmem:") {
            snapshot.rss_shmem_bytes = value_kb.saturating_mul(1024);
        }
    }

    snapshot
}

fn parse_status_line_kb(line: &str, prefix: &str) -> Option<u64> {
    if !line.starts_with(prefix) {
        return None;
    }

    line.split_whitespace()
        .nth(1)
        .and_then(|value| value.parse::<u64>().ok())
}

fn read_statm_rss_bytes() -> Option<u64> {
    let page_size = 4096;
    let content = std::fs::read_to_string("/proc/self/statm").ok()?;
    let rss_pages = content
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u64>().ok())?;
    Some(rss_pages.saturating_mul(page_size))
}

#[cfg(test)]
mod tests {
    use super::{
        classify_memory_pressure, malloc_trim_system_allocator, parse_mem_available_bytes,
        parse_proc_status_kb, MemoryPressure,
    };

    const GB: u64 = 1024 * 1024 * 1024;

    #[test]
    fn mem_available_parses_kb_to_bytes() {
        let content = "MemTotal:    32000000 kB\nMemFree: 1000 kB\nMemAvailable:    2500000 kB\n";
        assert_eq!(parse_mem_available_bytes(content), Some(2_500_000 * 1024));
    }

    #[test]
    fn mem_available_absent_is_none() {
        assert_eq!(parse_mem_available_bytes("MemTotal: 32000000 kB\n"), None);
    }

    #[test]
    fn classify_host_floor_breach_is_critical_even_when_process_is_small() {
        // The incident class: Axon process tiny (3 GB << 14 GB cap) but the VM is saturated
        // aggregate (only 2 GB allocatable, below the 3 GB floor) → CRITICAL.
        let level = classify_memory_pressure(3 * GB, 14 * GB, Some(2 * GB), 3 * GB);
        assert_eq!(level, MemoryPressure::Critical);
    }

    #[test]
    fn classify_host_warning_band_is_elevated() {
        // 5 GB available, floor 3 GB → between floor and 2×floor → ELEVATED.
        let level = classify_memory_pressure(3 * GB, 14 * GB, Some(5 * GB), 3 * GB);
        assert_eq!(level, MemoryPressure::Elevated);
    }

    #[test]
    fn classify_healthy_host_is_normal() {
        let level = classify_memory_pressure(3 * GB, 14 * GB, Some(20 * GB), 3 * GB);
        assert_eq!(level, MemoryPressure::Normal);
    }

    #[test]
    fn classify_process_over_its_own_cap_is_critical() {
        let level = classify_memory_pressure(15 * GB, 14 * GB, Some(20 * GB), 3 * GB);
        assert_eq!(level, MemoryPressure::Critical);
    }

    #[test]
    fn classify_unknown_mem_available_uses_process_only() {
        // No host signal → fall back to per-process band only.
        assert_eq!(
            classify_memory_pressure(3 * GB, 14 * GB, None, 3 * GB),
            MemoryPressure::Normal
        );
        assert_eq!(
            classify_memory_pressure(13 * GB, 14 * GB, None, 3 * GB),
            MemoryPressure::Elevated
        );
    }


    #[test]
    fn parse_proc_status_extracts_rss_breakdown() {
        let snapshot = parse_proc_status_kb(
            "VmRSS:\t   7340 kB\nRssAnon:\t   5120 kB\nRssFile:\t   1920 kB\nRssShmem:\t    300 kB\n",
        );

        assert_eq!(snapshot.rss_bytes, 7_340 * 1024);
        assert_eq!(snapshot.rss_anon_bytes, 5_120 * 1024);
        assert_eq!(snapshot.rss_file_bytes, 1_920 * 1024);
        assert_eq!(snapshot.rss_shmem_bytes, 300 * 1024);
    }

    #[test]
    fn malloc_trim_helper_is_callable() {
        let _ = malloc_trim_system_allocator();
    }
}
