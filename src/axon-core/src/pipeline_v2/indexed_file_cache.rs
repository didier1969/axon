//! In-RAM `IndexedFile` filter cache for the streaming pipeline (CPT-AXO-054).
//!
//! The cache is the watcher's only persistent "already-seen" memory under
//! v2. A file is skipped (no A1 work) when its `(path, content_hash)` pair
//! is already recorded as indexed. Otherwise the path enters the A1 stage,
//! and once A3 successfully UPSERTs the graph it also UPSERTs IndexedFile
//! to record the new hash.
//!
//! The cache lives behind an [`arc_swap::ArcSwap`] so bulk reloads (e.g. at
//! boot when reading the full table back from PG) are atomic — readers see
//! either the old or the new snapshot, never a torn intermediate state.
//! Steady-state writes go through the inner [`dashmap::DashMap`], which is
//! concurrent and lock-free for the common case.

use std::sync::Arc;

use arc_swap::ArcSwap;
use dashmap::DashMap;

/// Cached record for one indexed path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedFileEntry {
    pub content_hash: String,
    pub last_seen_ms: i64,
}

/// Watcher filter — answers "have I seen this `(path, hash)` already?"
///
/// All methods are safe to call concurrently from any number of tasks /
/// threads. Bulk reload at boot is done via [`IndexedFileCache::replace`].
pub struct IndexedFileCache {
    inner: ArcSwap<DashMap<String, IndexedFileEntry>>,
}

impl IndexedFileCache {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: ArcSwap::from_pointee(DashMap::new()),
        })
    }

    /// Build a cache pre-populated from an iterator of entries (e.g. from a
    /// `SELECT path, content_hash, last_seen_ms FROM IndexedFile` round-trip
    /// at boot).
    pub fn from_iter<I>(items: I) -> Arc<Self>
    where
        I: IntoIterator<Item = (String, IndexedFileEntry)>,
    {
        let map = DashMap::new();
        for (path, entry) in items {
            map.insert(path, entry);
        }
        Arc::new(Self {
            inner: ArcSwap::from_pointee(map),
        })
    }

    /// Atomically swap the underlying map. Use this for boot-time bulk reload
    /// so the watcher hot path observes a single transition rather than N
    /// partial updates.
    pub fn replace<I>(&self, items: I)
    where
        I: IntoIterator<Item = (String, IndexedFileEntry)>,
    {
        let next = DashMap::new();
        for (path, entry) in items {
            next.insert(path, entry);
        }
        self.inner.store(Arc::new(next));
    }

    /// Returns `true` if the watcher should hand `path` off to A1 — i.e. the
    /// path is unknown OR its current content hash differs from the cached one.
    /// Returns `false` if the file is unchanged (skip — already indexed).
    pub fn should_index(&self, path: &str, content_hash: &str) -> bool {
        let map = self.inner.load();
        map.get(path)
            .map_or(true, |entry| entry.content_hash != content_hash)
    }

    /// Record that `path` is now indexed at `content_hash` (overwriting any
    /// prior entry). Called from A3 after a successful UPSERT batch.
    pub fn mark_indexed(&self, path: String, content_hash: String, last_seen_ms: i64) {
        let map = self.inner.load();
        map.insert(
            path,
            IndexedFileEntry {
                content_hash,
                last_seen_ms,
            },
        );
    }

    /// Look up the cached entry for `path`, if any.
    pub fn get(&self, path: &str) -> Option<IndexedFileEntry> {
        self.inner.load().get(path).map(|e| e.value().clone())
    }

    /// Drop the entry for `path` (e.g. after a file deletion event from the
    /// watcher). No-op if the path is not present.
    pub fn forget(&self, path: &str) {
        let map = self.inner.load();
        map.remove(path);
    }

    pub fn len(&self) -> usize {
        self.inner.load().len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.load().is_empty()
    }

    /// Snapshot every entry as a Vec — useful for flushing to disk / PG or
    /// for telemetry dumps. NOT lock-free over the iteration; do not call on
    /// the watcher hot path.
    pub fn snapshot(&self) -> Vec<(String, IndexedFileEntry)> {
        self.inner
            .load()
            .iter()
            .map(|item| (item.key().clone(), item.value().clone()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_index_returns_true_for_unknown_paths() {
        let cache = IndexedFileCache::new();
        assert!(cache.should_index("/tmp/new.rs", "hash-a"));
    }

    #[test]
    fn should_index_returns_false_when_hash_matches_cache() {
        let cache = IndexedFileCache::new();
        cache.mark_indexed("/tmp/known.rs".into(), "hash-a".into(), 1_000);
        assert!(!cache.should_index("/tmp/known.rs", "hash-a"));
    }

    #[test]
    fn should_index_returns_true_when_hash_changed() {
        let cache = IndexedFileCache::new();
        cache.mark_indexed("/tmp/changed.rs".into(), "hash-a".into(), 1_000);
        assert!(cache.should_index("/tmp/changed.rs", "hash-b"));
    }

    #[test]
    fn mark_indexed_overwrites_previous_entry_atomically() {
        let cache = IndexedFileCache::new();
        cache.mark_indexed("/tmp/file.rs".into(), "hash-a".into(), 1_000);
        cache.mark_indexed("/tmp/file.rs".into(), "hash-b".into(), 2_000);
        let entry = cache.get("/tmp/file.rs").expect("entry must exist");
        assert_eq!(entry.content_hash, "hash-b");
        assert_eq!(entry.last_seen_ms, 2_000);
        assert_eq!(cache.len(), 1, "no duplicate entries on overwrite");
    }

    #[test]
    fn from_iter_populates_cache_in_one_shot() {
        let cache = IndexedFileCache::from_iter([
            (
                "/tmp/a.rs".into(),
                IndexedFileEntry {
                    content_hash: "hash-a".into(),
                    last_seen_ms: 1_000,
                },
            ),
            (
                "/tmp/b.rs".into(),
                IndexedFileEntry {
                    content_hash: "hash-b".into(),
                    last_seen_ms: 2_000,
                },
            ),
        ]);
        assert_eq!(cache.len(), 2);
        assert!(!cache.should_index("/tmp/a.rs", "hash-a"));
        assert!(!cache.should_index("/tmp/b.rs", "hash-b"));
        assert!(cache.should_index("/tmp/a.rs", "hash-changed"));
    }

    #[test]
    fn replace_atomically_swaps_underlying_map() {
        let cache = IndexedFileCache::new();
        cache.mark_indexed("/tmp/old.rs".into(), "hash-x".into(), 1_000);
        cache.replace([(
            "/tmp/new.rs".into(),
            IndexedFileEntry {
                content_hash: "hash-y".into(),
                last_seen_ms: 5_000,
            },
        )]);
        assert!(cache.get("/tmp/old.rs").is_none(), "old entries cleared");
        assert!(cache.get("/tmp/new.rs").is_some(), "new entries visible");
    }

    #[test]
    fn forget_removes_entry_from_cache() {
        let cache = IndexedFileCache::new();
        cache.mark_indexed("/tmp/deleted.rs".into(), "hash-a".into(), 1_000);
        assert_eq!(cache.len(), 1);
        cache.forget("/tmp/deleted.rs");
        assert_eq!(cache.len(), 0);
        assert!(cache.should_index("/tmp/deleted.rs", "hash-a"));
    }

    #[test]
    fn snapshot_returns_all_entries_for_export() {
        let cache = IndexedFileCache::new();
        cache.mark_indexed("/tmp/a.rs".into(), "hash-a".into(), 1_000);
        cache.mark_indexed("/tmp/b.rs".into(), "hash-b".into(), 2_000);
        let mut snap = cache.snapshot();
        snap.sort_by(|x, y| x.0.cmp(&y.0));
        assert_eq!(snap.len(), 2);
        assert_eq!(snap[0].0, "/tmp/a.rs");
        assert_eq!(snap[0].1.content_hash, "hash-a");
        assert_eq!(snap[1].0, "/tmp/b.rs");
        assert_eq!(snap[1].1.content_hash, "hash-b");
    }
}
