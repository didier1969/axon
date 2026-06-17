//! In-RAM `IndexedFile` filter cache for the streaming pipeline (CPT-AXO-054).
//!
//! The cache is the watcher's only persistent "already-seen" memory under
//! v2. It is the **two-level skip filter** that makes a re-walk / restart cheap
//! (PIL-AXO-007):
//!
//!  - **Level 1 — `should_read` (mtime + size).** Answered from a bare
//!    `stat()` BEFORE any file read. If the path's mtime AND size match the
//!    last indexed state, the file is skipped with **zero I/O** (no read, no
//!    sha256, no parse). This is the I/O optimisation that replaces the SQL
//!    `mtime/size` change-detection the scanner used to do in PG.
//!  - **Level 2 — `should_index` (content_hash).** Once A1 has read+hashed a
//!    file (because level 1 said "changed"), this catches the touched-but-
//!    identical case (e.g. `touch`, whitespace-only reformat) and skips the
//!    A2 tree-sitter parse + A3 write.
//!
//! Once A3 successfully UPSERTs the graph, the A3 receipt drain (in
//! `pipeline_v2_runtime`, not A3 itself) calls [`IndexedFileCache::mark_indexed`]
//! to record `(content_hash, mtime, size, last_seen_ms)` for the next walk.
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
    /// File mtime (ms) at last successful index — level-1 change detection.
    pub mtime_ms: i64,
    /// File size (bytes) at last successful index — level-1 change detection.
    pub size_bytes: u64,
}

/// Watcher filter — answers "must I read / re-parse this path?".
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
    /// `SELECT path, content_hash, last_seen_ms, mtime_ms, size_bytes FROM
    /// IndexedFile` round-trip at boot).
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

    /// **Level 1 (cheapest).** Returns `true` if the file must be READ + hashed
    /// — i.e. the path is unknown OR its `mtime`/`size` differ from the last
    /// indexed metadata. Returns `false` (skip the read entirely — no I/O, no
    /// sha256, no parse) when BOTH mtime and size match. Answerable from a bare
    /// `stat()`, which is why it gates the expensive read in A1 (PIL-AXO-007).
    pub fn should_read(&self, path: &str, mtime_ms: i64, size_bytes: u64) -> bool {
        let map = self.inner.load();
        map.get(path).map_or(true, |entry| {
            entry.mtime_ms != mtime_ms || entry.size_bytes != size_bytes
        })
    }

    /// **Level 2.** Returns `true` if A1's freshly-read content must go to A2 —
    /// i.e. the path is unknown OR its current content hash differs from the
    /// cached one. Returns `false` if the content is unchanged (skip the parse —
    /// already indexed). Catches the touched-but-identical case that level 1
    /// let through.
    pub fn should_index(&self, path: &str, content_hash: &str) -> bool {
        let map = self.inner.load();
        map.get(path)
            .map_or(true, |entry| entry.content_hash != content_hash)
    }

    /// Record that `path` is now indexed at `content_hash` with the given
    /// `mtime`/`size` metadata (overwriting any prior entry). Called from A3
    /// after a successful UPSERT batch.
    pub fn mark_indexed(
        &self,
        path: String,
        content_hash: String,
        last_seen_ms: i64,
        mtime_ms: i64,
        size_bytes: u64,
    ) {
        let map = self.inner.load();
        map.insert(
            path,
            IndexedFileEntry {
                content_hash,
                last_seen_ms,
                mtime_ms,
                size_bytes,
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

    fn entry(hash: &str, last_seen: i64, mtime: i64, size: u64) -> IndexedFileEntry {
        IndexedFileEntry {
            content_hash: hash.into(),
            last_seen_ms: last_seen,
            mtime_ms: mtime,
            size_bytes: size,
        }
    }

    // --- Level 1: should_read (mtime + size) — the I/O pre-filter (PIL-AXO-007) ---

    #[test]
    fn should_read_returns_true_for_unknown_path() {
        let cache = IndexedFileCache::new();
        assert!(cache.should_read("/tmp/new.rs", 100, 42));
    }

    #[test]
    fn should_read_returns_false_when_mtime_and_size_match() {
        let cache = IndexedFileCache::new();
        cache.mark_indexed("/tmp/known.rs".into(), "hash-a".into(), 1_000, 100, 42);
        // Unchanged mtime AND size → skip the read entirely (zero I/O).
        assert!(!cache.should_read("/tmp/known.rs", 100, 42));
    }

    #[test]
    fn should_read_returns_true_when_mtime_changed() {
        let cache = IndexedFileCache::new();
        cache.mark_indexed("/tmp/f.rs".into(), "hash-a".into(), 1_000, 100, 42);
        assert!(cache.should_read("/tmp/f.rs", 200, 42));
    }

    #[test]
    fn should_read_returns_true_when_size_changed() {
        let cache = IndexedFileCache::new();
        cache.mark_indexed("/tmp/f.rs".into(), "hash-a".into(), 1_000, 100, 42);
        assert!(cache.should_read("/tmp/f.rs", 100, 43));
    }

    // --- Level 2: should_index (content_hash) — the parse skip ---

    #[test]
    fn should_index_returns_true_for_unknown_paths() {
        let cache = IndexedFileCache::new();
        assert!(cache.should_index("/tmp/new.rs", "hash-a"));
    }

    #[test]
    fn should_index_returns_false_when_hash_matches_cache() {
        let cache = IndexedFileCache::new();
        cache.mark_indexed("/tmp/known.rs".into(), "hash-a".into(), 1_000, 100, 42);
        assert!(!cache.should_index("/tmp/known.rs", "hash-a"));
    }

    #[test]
    fn should_index_returns_true_when_hash_changed() {
        let cache = IndexedFileCache::new();
        cache.mark_indexed("/tmp/changed.rs".into(), "hash-a".into(), 1_000, 100, 42);
        assert!(cache.should_index("/tmp/changed.rs", "hash-b"));
    }

    // --- Two-level interaction: touched-but-identical (mtime moves, hash stays) ---

    #[test]
    fn touched_but_identical_file_reads_at_level1_but_skips_parse_at_level2() {
        let cache = IndexedFileCache::new();
        cache.mark_indexed("/tmp/f.rs".into(), "hash-a".into(), 1_000, 100, 42);
        // `touch` bumps mtime → level 1 says "read it".
        assert!(cache.should_read("/tmp/f.rs", 200, 42));
        // But the content (hash) is unchanged → level 2 skips the parse.
        assert!(!cache.should_index("/tmp/f.rs", "hash-a"));
    }

    #[test]
    fn mark_indexed_overwrites_previous_entry_atomically() {
        let cache = IndexedFileCache::new();
        cache.mark_indexed("/tmp/file.rs".into(), "hash-a".into(), 1_000, 100, 42);
        cache.mark_indexed("/tmp/file.rs".into(), "hash-b".into(), 2_000, 200, 84);
        let entry = cache.get("/tmp/file.rs").expect("entry must exist");
        assert_eq!(entry.content_hash, "hash-b");
        assert_eq!(entry.last_seen_ms, 2_000);
        assert_eq!(entry.mtime_ms, 200);
        assert_eq!(entry.size_bytes, 84);
        assert_eq!(cache.len(), 1, "no duplicate entries on overwrite");
    }

    #[test]
    fn from_iter_populates_cache_in_one_shot() {
        let cache = IndexedFileCache::from_iter([
            ("/tmp/a.rs".into(), entry("hash-a", 1_000, 100, 42)),
            ("/tmp/b.rs".into(), entry("hash-b", 2_000, 200, 84)),
        ]);
        assert_eq!(cache.len(), 2);
        assert!(!cache.should_index("/tmp/a.rs", "hash-a"));
        assert!(!cache.should_read("/tmp/a.rs", 100, 42));
        assert!(cache.should_index("/tmp/a.rs", "hash-changed"));
    }

    #[test]
    fn replace_atomically_swaps_underlying_map() {
        let cache = IndexedFileCache::new();
        cache.mark_indexed("/tmp/old.rs".into(), "hash-x".into(), 1_000, 100, 42);
        cache.replace([("/tmp/new.rs".into(), entry("hash-y", 5_000, 500, 99))]);
        assert!(cache.get("/tmp/old.rs").is_none(), "old entries cleared");
        assert!(cache.get("/tmp/new.rs").is_some(), "new entries visible");
    }

    #[test]
    fn forget_removes_entry_from_cache() {
        let cache = IndexedFileCache::new();
        cache.mark_indexed("/tmp/deleted.rs".into(), "hash-a".into(), 1_000, 100, 42);
        assert_eq!(cache.len(), 1);
        cache.forget("/tmp/deleted.rs");
        assert_eq!(cache.len(), 0);
        assert!(cache.should_index("/tmp/deleted.rs", "hash-a"));
        assert!(cache.should_read("/tmp/deleted.rs", 100, 42));
    }

    #[test]
    fn snapshot_returns_all_entries_for_export() {
        let cache = IndexedFileCache::new();
        cache.mark_indexed("/tmp/a.rs".into(), "hash-a".into(), 1_000, 100, 42);
        cache.mark_indexed("/tmp/b.rs".into(), "hash-b".into(), 2_000, 200, 84);
        let mut snap = cache.snapshot();
        snap.sort_by(|x, y| x.0.cmp(&y.0));
        assert_eq!(snap.len(), 2);
        assert_eq!(snap[0].0, "/tmp/a.rs");
        assert_eq!(snap[0].1.content_hash, "hash-a");
        assert_eq!(snap[1].0, "/tmp/b.rs");
        assert_eq!(snap[1].1.content_hash, "hash-b");
    }
}
