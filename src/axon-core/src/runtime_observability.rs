use crate::graph::GraphStore;

#[derive(Debug, Clone, Copy, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ProcessMemorySnapshot {
    pub rss_bytes: u64,
    pub rss_anon_bytes: u64,
    pub rss_file_bytes: u64,
    pub rss_shmem_bytes: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DuckDbStorageSnapshot {
    pub db_file_bytes: u64,
    pub db_wal_bytes: u64,
    pub db_total_bytes: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DuckDbMemorySnapshot {
    pub memory_usage_bytes: u64,
    pub temporary_storage_bytes: u64,
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

/// PG canonical (REQ-AXO-271 slice 4c) : the DuckDB-specific db-file
/// + WAL telemetry is irrelevant under PG (storage lives server-side
/// outside the process). Returned as zero-filled for backwards-compat
/// of the telemetry envelope ; PG storage observability will land in
/// a follow-up slice via `pg_stat_database` / `pg_database_size`.
pub fn duckdb_storage_snapshot(_store: &GraphStore) -> DuckDbStorageSnapshot {
    DuckDbStorageSnapshot::default()
}

/// PG canonical (REQ-AXO-271 slice 4c) : the DuckDB `duckdb_memory()`
/// table function does not exist under PG and previously poisoned
/// connections with aborted-transaction state (REQ-AXO-242). Returned
/// as zero-filled for backwards-compat of the telemetry envelope ; PG
/// memory observability will land in a follow-up slice via
/// `pg_stat_activity` / `pg_buffercache`.
pub fn duckdb_memory_snapshot(_store: &GraphStore) -> DuckDbMemorySnapshot {
    DuckDbMemorySnapshot::default()
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
    use super::{malloc_trim_system_allocator, parse_proc_status_kb};

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
