use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Result;

use crate::graph::GraphStore;

const ENABLE_ENV: &str = "AXON_ENABLE_FILE_INGRESS_GUARD";

static GUARD_HITS: AtomicU64 = AtomicU64::new(0);
static GUARD_MISSES: AtomicU64 = AtomicU64::new(0);
static GUARD_BYPASSED_TOTAL: AtomicU64 = AtomicU64::new(0);
static GUARD_HYDRATED_ENTRIES: AtomicU64 = AtomicU64::new(0);
static GUARD_HYDRATION_DURATION_MS: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardDecision {
    StageNew,
    StageChanged,
    SkipUnchanged,
    RetombstoneMissing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileIngressRow {
    pub path: String,
    pub status: String,
    pub mtime: i64,
    pub size: i64,
    pub file_stage: String,
    pub status_reason: String,
    pub graph_ready: bool,
}

impl FileIngressRow {
    pub fn is_pending_graph_eligible(&self) -> bool {
        self.status == "pending"
            && !self.graph_ready
            && !matches!(
                self.file_stage.as_str(),
                "deleted" | "skipped" | "oversized"
            )
            && !matches!(
                self.status.as_str(),
                "deleted" | "skipped" | "oversized_for_current_budget"
            )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileStamp {
    mtime: i64,
    size: i64,
    is_deleted: bool,
    is_indexing: bool,
}

#[derive(Debug)]
pub struct FileIngressGuard {
    enabled: bool,
    by_path: HashMap<String, FileStamp>,
}

pub type SharedFileIngressGuard = Arc<Mutex<FileIngressGuard>>;

#[derive(Debug, Clone, Copy, Default)]
pub struct GuardMetricsSnapshot {
    pub hits: u64,
    pub misses: u64,
    pub bypassed_total: u64,
    pub hydrated_entries: u64,
    pub hydration_duration_ms: u64,
}

impl FileIngressGuard {
    pub fn hydrate_from_store(_store: &GraphStore) -> Result<Self> {
        // REQ-AXO-901653 slice-5c — `public.File` table retired ; pipeline_v2
        // (REQ-AXO-289 / CPT-AXO-054) writes IndexedFile (3 cols : path,
        // content_hash, last_seen_ms) and does not carry per-path mtime/size
        // snapshots. The FileIngressGuard hydration cache now boots empty :
        // pipeline_v2 stage A1 short-circuits unchanged files via
        // content_hash comparison directly, so the in-memory mtime/size dedup
        // is no longer load-bearing. GuardDecision::StageNew is returned for
        // every probe on a cold guard, which matches pipeline_v2 semantics.
        let started_at = std::time::Instant::now();
        let by_path: HashMap<String, FileStamp> = HashMap::new();
        GUARD_HYDRATED_ENTRIES.store(by_path.len() as u64, Ordering::Relaxed);
        GUARD_HYDRATION_DURATION_MS
            .store(started_at.elapsed().as_millis() as u64, Ordering::Relaxed);

        Ok(Self {
            enabled: read_enabled_from_env(),
            by_path,
        })
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn should_stage(&self, path: &Path, mtime: i64, size: i64) -> GuardDecision {
        if !self.enabled {
            GUARD_BYPASSED_TOTAL.fetch_add(1, Ordering::Relaxed);
            return GuardDecision::StageNew;
        }

        let key = normalize_path(path);
        let Some(existing) = self.by_path.get(&key) else {
            GUARD_MISSES.fetch_add(1, Ordering::Relaxed);
            return GuardDecision::StageNew;
        };

        if existing.is_deleted {
            GUARD_MISSES.fetch_add(1, Ordering::Relaxed);
            return GuardDecision::StageChanged;
        }

        if existing.mtime == mtime && existing.size == size {
            GUARD_HITS.fetch_add(1, Ordering::Relaxed);
            return GuardDecision::SkipUnchanged;
        }

        if existing.is_indexing {
            GUARD_MISSES.fetch_add(1, Ordering::Relaxed);
            return GuardDecision::StageChanged;
        }

        GUARD_MISSES.fetch_add(1, Ordering::Relaxed);
        GuardDecision::StageChanged
    }

    pub fn record_committed_row(&mut self, row: FileIngressRow) {
        self.by_path.insert(
            row.path,
            FileStamp {
                mtime: row.mtime,
                size: row.size,
                is_deleted: row.status == "deleted",
                is_indexing: row.status == "indexing",
            },
        );
    }

    pub fn record_tombstone(&mut self, path: &Path) {
        let key = normalize_path(path);
        let entry = self.by_path.entry(key).or_insert(FileStamp {
            mtime: 0,
            size: 0,
            is_deleted: true,
            is_indexing: false,
        });
        entry.is_deleted = true;
        entry.is_indexing = false;
    }

    pub fn invalidate_all(&mut self) {
        self.by_path.clear();
    }
}

impl Default for FileIngressGuard {
    fn default() -> Self {
        Self {
            enabled: read_enabled_from_env(),
            by_path: HashMap::new(),
        }
    }
}

fn normalize_path(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

fn read_enabled_from_env() -> bool {
    match std::env::var(ENABLE_ENV) {
        Ok(value) => !matches!(value.trim(), "0" | "false" | "FALSE" | "False"),
        Err(_) => true,
    }
}

// REQ-AXO-901653 slice-5c — `parse_i64_value` removed (was only used by the
// retired File-table hydration path).

pub fn guard_metrics_snapshot() -> GuardMetricsSnapshot {
    GuardMetricsSnapshot {
        hits: GUARD_HITS.load(Ordering::Relaxed),
        misses: GUARD_MISSES.load(Ordering::Relaxed),
        bypassed_total: GUARD_BYPASSED_TOTAL.load(Ordering::Relaxed),
        hydrated_entries: GUARD_HYDRATED_ENTRIES.load(Ordering::Relaxed),
        hydration_duration_ms: GUARD_HYDRATION_DURATION_MS.load(Ordering::Relaxed),
    }
}
