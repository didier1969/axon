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
    // REQ-AXO-901757 slice C (AC4) — SOLL read RAM-coverage observability. Every
    // `snapshot()` either serves the cached RAM graph (`ram_hits`) or pays a PG
    // load (`pg_loads`). The ratio = how well SOLL reads stay on RAM vs round-
    // trip PG (PIL-AXO-9002 invariant: RAM mirror primary, PG fallback explicit).
    ram_hits: AtomicU64,
    pg_loads: AtomicU64,
}

impl SollSnapshotCache {
    pub fn new(store: Arc<GraphStore>) -> Arc<Self> {
        Arc::new(Self {
            snapshots: ArcSwap::from_pointee(HashMap::new()),
            load_lock: Mutex::new(()),
            next_generation: AtomicU64::new(1),
            store,
            ram_hits: AtomicU64::new(0),
            pg_loads: AtomicU64::new(0),
        })
    }

    /// REQ-AXO-901757 slice C (AC4) — `(ram_hits, pg_loads)` since process start.
    /// `ram_hits` = snapshot reads served from the in-memory graph; `pg_loads` =
    /// reads that had to (re)load from PG (cold/invalidated). Surfaced in
    /// `status mode=verbose` as the SOLL read RAM-coverage ratio.
    pub fn read_stats(&self) -> (u64, u64) {
        (
            self.ram_hits.load(Ordering::Relaxed),
            self.pg_loads.load(Ordering::Relaxed),
        )
    }

    /// Hot read path. Returns a cached snapshot if present, otherwise
    /// loads synchronously, populates the cache, and returns the new
    /// `Arc<SollSnapshot>`. Concurrent callers for the same project
    /// serialize on the internal load lock; the second caller observes
    /// the snapshot the first caller produced.
    pub fn snapshot(&self, project_code: &str) -> Result<Arc<SollSnapshot>> {
        if let Some(snap) = self.snapshots.load().get(project_code).cloned() {
            self.ram_hits.fetch_add(1, Ordering::Relaxed);
            return Ok(snap);
        }
        // Miss path. Serialize loads so a thundering herd doesn't
        // hammer PG with duplicate queries.
        let _guard = self.load_lock.lock().unwrap_or_else(|e| e.into_inner());
        // Re-check under the lock — another thread may have populated
        // the slot while we waited (counts as a RAM hit, not a PG load).
        if let Some(snap) = self.snapshots.load().get(project_code).cloned() {
            self.ram_hits.fetch_add(1, Ordering::Relaxed);
            return Ok(snap);
        }
        self.pg_loads.fetch_add(1, Ordering::Relaxed);
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

#[cfg(test)]
mod tests {
    use super::*;

    // REQ-AXO-901757 slice C (AC4) — the first snapshot() of a project pays a PG
    // load; subsequent reads are served from RAM. read_stats() reflects this so
    // status can expose the SOLL read RAM-coverage ratio.
    #[test]
    fn read_stats_counts_pg_load_then_ram_hits() {
        let store = Arc::new(crate::tests::test_helpers::create_test_db().unwrap());
        let cache = SollSnapshotCache::new(store);
        assert_eq!(cache.read_stats(), (0, 0), "no reads yet");

        // First read for AXO: cold cache → PG load.
        let _ = cache.snapshot("AXO").expect("snapshot loads");
        assert_eq!(cache.read_stats(), (0, 1), "cold read = one PG load");

        // Next two reads are served from the warmed RAM cache.
        let _ = cache.snapshot("AXO").expect("cached");
        let _ = cache.snapshot("AXO").expect("cached");
        assert_eq!(cache.read_stats(), (2, 1), "warm reads = RAM hits");

        // Invalidation forces the next read back to PG.
        cache.invalidate("AXO");
        let _ = cache.snapshot("AXO").expect("reload");
        assert_eq!(cache.read_stats(), (2, 2), "post-invalidation = PG load");
    }
}
