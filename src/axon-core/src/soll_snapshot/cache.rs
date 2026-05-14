//! Lock-free, lazily-populated cache of `SollSnapshot` per project (REQ-AXO-322).
//!
//! Reads are served from `ArcSwap<HashMap<project_code, Arc<SollSnapshot>>>`
//! with a single atomic load. On miss, the cache holds a write lock for
//! the duration of the SQL load (~50 ms cold), then atomically swaps in
//! the new map. Invalidation clears the entry under the write lock; the
//! next read triggers a reload.
//!
//! Hot-path readers should call [`SollSnapshotCache::snapshot`], which
//! returns `Arc<SollSnapshot>` so the underlying data can outlive the
//! cache map without copying.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use arc_swap::ArcSwap;

use crate::graph::GraphStore;

use super::loader::load_snapshot;
use super::snapshot::SollSnapshot;

type SnapshotMap = HashMap<String, Arc<SollSnapshot>>;

pub struct SollSnapshotCache {
    snapshots: ArcSwap<SnapshotMap>,
    load_lock: Mutex<()>,
    next_generation: AtomicU64,
    store: Arc<GraphStore>,
}

impl SollSnapshotCache {
    pub fn new(store: Arc<GraphStore>) -> Arc<Self> {
        Arc::new(Self {
            snapshots: ArcSwap::from_pointee(HashMap::new()),
            load_lock: Mutex::new(()),
            next_generation: AtomicU64::new(1),
            store,
        })
    }

    /// Hot read path. Returns a cached snapshot if present, otherwise
    /// loads synchronously, populates the cache, and returns the new
    /// `Arc<SollSnapshot>`. Concurrent callers for the same project
    /// serialize on the internal load lock; the second caller observes
    /// the snapshot the first caller produced.
    pub fn snapshot(&self, project_code: &str) -> Result<Arc<SollSnapshot>> {
        if let Some(snap) = self.snapshots.load().get(project_code).cloned() {
            return Ok(snap);
        }
        // Miss path. Serialize loads so a thundering herd doesn't
        // hammer PG with duplicate queries.
        let _guard = self.load_lock.lock().unwrap_or_else(|e| e.into_inner());
        // Re-check under the lock — another thread may have populated
        // the slot while we waited.
        if let Some(snap) = self.snapshots.load().get(project_code).cloned() {
            return Ok(snap);
        }
        let generation = self.next_generation.fetch_add(1, Ordering::SeqCst);
        let snap = Arc::new(load_snapshot(&self.store, project_code, generation)?);
        let mut new_map = (**self.snapshots.load()).clone();
        new_map.insert(project_code.to_string(), snap.clone());
        self.snapshots.store(Arc::new(new_map));
        Ok(snap)
    }

    /// Drop the cached snapshot for `project_code`. The next call to
    /// `snapshot()` will reload from PG. Cheap (atomic swap of the
    /// outer map).
    pub fn invalidate(&self, project_code: &str) {
        let _guard = self.load_lock.lock().unwrap_or_else(|e| e.into_inner());
        let current = self.snapshots.load();
        if !current.contains_key(project_code) {
            return;
        }
        let mut new_map = (**current).clone();
        new_map.remove(project_code);
        self.snapshots.store(Arc::new(new_map));
    }

    /// Drop ALL cached snapshots. Used by mutation paths whose
    /// project_code cannot be inferred (e.g. bulk restore).
    #[allow(dead_code)]
    pub fn invalidate_all(&self) {
        let _guard = self.load_lock.lock().unwrap_or_else(|e| e.into_inner());
        self.snapshots.store(Arc::new(HashMap::new()));
    }

    /// Currently cached project codes — diagnostic only.
    #[allow(dead_code)]
    pub fn cached_projects(&self) -> Vec<String> {
        self.snapshots.load().keys().cloned().collect()
    }
}
