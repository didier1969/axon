use std::path::{Path, PathBuf};

use crate::graph::GraphStore;

#[derive(Debug, Clone, Copy, Default)]
pub struct ProcessMemorySnapshot {
    pub rss_bytes: u64,
    pub rss_anon_bytes: u64,
    pub rss_file_bytes: u64,
    pub rss_shmem_bytes: u64,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct DuckDbStorageSnapshot {
    pub db_file_bytes: u64,
    pub db_wal_bytes: u64,
    pub db_total_bytes: u64,
}

#[derive(Debug, Clone, Copy, Default)]
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

pub fn duckdb_storage_snapshot(store: &GraphStore) -> DuckDbStorageSnapshot {
    let Some(db_path) = store.db_path.as_ref() else {
        return DuckDbStorageSnapshot::default();
    };

    let db_file_bytes = file_len(db_path);
    let db_wal_bytes = file_len(&wal_path_for(db_path));

    DuckDbStorageSnapshot {
        db_file_bytes,
        db_wal_bytes,
        db_total_bytes: db_file_bytes.saturating_add(db_wal_bytes),
    }
}

pub fn duckdb_memory_snapshot(store: &GraphStore) -> DuckDbMemorySnapshot {
    let raw = match store.query_json(
        "SELECT COALESCE(sum(memory_usage_bytes), 0), \
                COALESCE(sum(temporary_storage_bytes), 0) \
         FROM duckdb_memory()",
    ) {
        Ok(raw) => raw,
        Err(_) => return DuckDbMemorySnapshot::default(),
    };

    let rows: Vec<Vec<serde_json::Value>> = serde_json::from_str(&raw).unwrap_or_default();
    let Some(row) = rows.first() else {
        return DuckDbMemorySnapshot::default();
    };

    DuckDbMemorySnapshot {
        memory_usage_bytes: parse_u64_value(row.first()).unwrap_or_default(),
        temporary_storage_bytes: parse_u64_value(row.get(1)).unwrap_or_default(),
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

fn file_len(path: &Path) -> u64 {
    std::fs::metadata(path).map(|metadata| metadata.len()).unwrap_or(0)
}

fn wal_path_for(db_path: &Path) -> PathBuf {
    PathBuf::from(format!("{}.wal", db_path.display()))
}

fn parse_u64_value(value: Option<&serde_json::Value>) -> Option<u64> {
    value
        .and_then(|value| value.as_u64())
        .or_else(|| value.and_then(|value| value.as_i64()).map(|value| value.max(0) as u64))
        .or_else(|| {
            value
                .and_then(|value| value.as_str())
                .and_then(|value| value.parse::<u64>().ok())
        })
}

#[cfg(test)]
mod tests {
    use super::{duckdb_storage_snapshot, malloc_trim_system_allocator, parse_proc_status_kb};
    use crate::graph::GraphStore;
    use tempfile::tempdir;

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
    fn duckdb_storage_snapshot_reports_db_and_wal_sizes() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("graph_v2");
        std::fs::create_dir_all(&root).unwrap();
        let store = GraphStore::new(root.to_string_lossy().as_ref()).unwrap();

        let snapshot = duckdb_storage_snapshot(&store);

        assert!(snapshot.db_file_bytes > 0);
        assert_eq!(
            snapshot.db_total_bytes,
            snapshot.db_file_bytes + snapshot.db_wal_bytes
        );
    }

    #[test]
    fn malloc_trim_helper_is_callable() {
        let _ = malloc_trim_system_allocator();
    }
}
