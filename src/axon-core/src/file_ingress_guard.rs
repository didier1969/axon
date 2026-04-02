use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use serde_json::Value;

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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileStamp {
    mtime: i64,
    size: i64,
    is_deleted: bool,
    is_indexing: bool,
}

#[derive(Debug, Default)]
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
    pub fn hydrate_from_store(store: &GraphStore) -> Result<Self> {
        let started_at = std::time::Instant::now();
        let raw = store.query_json(
            "SELECT path, status, mtime, size \
             FROM File",
        )?;
        let rows: Vec<Vec<Value>> = serde_json::from_str(&raw).unwrap_or_default();
        let mut by_path = HashMap::with_capacity(rows.len());
        for row in rows {
            let Some(path) = row.first().and_then(|value| value.as_str()) else {
                continue;
            };
            let status = row
                .get(1)
                .and_then(|value| value.as_str())
                .unwrap_or("pending");
            let mtime = row.get(2).and_then(parse_i64_value).unwrap_or_default();
            let size = row.get(3).and_then(parse_i64_value).unwrap_or_default();
            by_path.insert(
                path.to_string(),
                FileStamp {
                    mtime,
                    size,
                    is_deleted: status == "deleted",
                    is_indexing: status == "indexing",
                },
            );
        }
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

fn normalize_path(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

fn read_enabled_from_env() -> bool {
    match std::env::var(ENABLE_ENV) {
        Ok(value) => !matches!(value.trim(), "0" | "false" | "FALSE" | "False"),
        Err(_) => true,
    }
}

fn parse_i64_value(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|raw| i64::try_from(raw).ok()))
        .or_else(|| value.as_str().and_then(|raw| raw.parse::<i64>().ok()))
}

pub fn guard_metrics_snapshot() -> GuardMetricsSnapshot {
    GuardMetricsSnapshot {
        hits: GUARD_HITS.load(Ordering::Relaxed),
        misses: GUARD_MISSES.load(Ordering::Relaxed),
        bypassed_total: GUARD_BYPASSED_TOTAL.load(Ordering::Relaxed),
        hydrated_entries: GUARD_HYDRATED_ENTRIES.load(Ordering::Relaxed),
        hydration_duration_ms: GUARD_HYDRATION_DURATION_MS.load(Ordering::Relaxed),
    }
}
