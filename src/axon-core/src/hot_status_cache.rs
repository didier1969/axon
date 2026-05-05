//! Hot status cache — Phase A foundation (DEC-AXO-072 J.1).
//!
//! Keeps **transient** FileVectorizationQueue state (status, claim_token,
//! lease_epoch, lease_owner, timestamps) in process memory so the vector
//! lane can claim/release without paying a DuckDB single-writer round-trip
//! on every operation. DuckDB remains canonical for **content + durable
//! state** (Chunk, ChunkEmbedding, File.vector_ready=TRUE).
//!
//! Phase A is **write-through batched**: mutations are applied to the
//! cache AND queued in a dirty set; a flush thread (J.2) snapshots the
//! dirty set every 100ms and writes a single batched UPDATE per window
//! into the existing FVQ table. No schema change. Cache disabled by
//! default; activated by `AXON_HOT_STATUS_CACHE_ENABLED=true`.
//!
//! H.1 / H.2 contract (DEC-AXO-071) and DEC-AXO-070 commit G are
//! preserved: when the cache is disabled, callers fall through to the
//! existing direct-DB path — bit-identical behavior.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FvqStatus {
    Ready,
    Inflight,
    Done,
    Failed,
}

#[derive(Debug, Clone)]
pub struct FileHotState {
    pub status: FvqStatus,
    pub claim_token: Option<String>,
    pub lease_epoch: i64,
    pub lease_owner: Option<String>,
    pub enqueued_at_ms: i64,
    pub started_at_ms: Option<i64>,
    pub last_error: Option<String>,
    pub last_change_at_ms: i64,
}

#[derive(Debug, Clone)]
pub struct ClaimGrant {
    pub file_path: String,
    pub claim_token: String,
    pub lease_epoch: i64,
    pub lease_owner: String,
}

pub struct HotStatusCache {
    entries: RwLock<HashMap<String, FileHotState>>,
    dirty: Mutex<HashSet<String>>,
    enabled: AtomicBool,
    claim_seq: AtomicI64,
}

impl HotStatusCache {
    pub fn new() -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
            dirty: Mutex::new(HashSet::new()),
            enabled: AtomicBool::new(false),
            claim_seq: AtomicI64::new(1),
        }
    }

    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::Relaxed);
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub fn mark_ready(&self, file_path: &str, now_ms: i64) {
        let mut entries = self.entries.write().expect("hot cache write lock poisoned");
        match entries.get_mut(file_path) {
            Some(entry) if entry.status == FvqStatus::Done => return,
            Some(entry) => {
                entry.status = FvqStatus::Ready;
                entry.last_change_at_ms = now_ms;
                entry.last_error = None;
            }
            None => {
                entries.insert(
                    file_path.to_string(),
                    FileHotState {
                        status: FvqStatus::Ready,
                        claim_token: None,
                        lease_epoch: 0,
                        lease_owner: None,
                        enqueued_at_ms: now_ms,
                        started_at_ms: None,
                        last_error: None,
                        last_change_at_ms: now_ms,
                    },
                );
            }
        }
        drop(entries);
        self.dirty
            .lock()
            .expect("hot cache dirty lock poisoned")
            .insert(file_path.to_string());
    }

    pub fn try_claim(&self, file_path: &str, owner: &str, now_ms: i64) -> Option<ClaimGrant> {
        let mut entries = self.entries.write().expect("hot cache write lock poisoned");
        let entry = entries.get_mut(file_path)?;
        if entry.status != FvqStatus::Ready {
            return None;
        }
        let seq = self.claim_seq.fetch_add(1, Ordering::Relaxed);
        let token = format!("hot-{}-{}-{}", owner, now_ms, seq);
        entry.status = FvqStatus::Inflight;
        entry.claim_token = Some(token.clone());
        entry.lease_owner = Some(owner.to_string());
        entry.lease_epoch = entry.lease_epoch.saturating_add(1);
        entry.started_at_ms = Some(now_ms);
        entry.last_change_at_ms = now_ms;
        let grant = ClaimGrant {
            file_path: file_path.to_string(),
            claim_token: token,
            lease_epoch: entry.lease_epoch,
            lease_owner: owner.to_string(),
        };
        drop(entries);
        self.dirty
            .lock()
            .expect("hot cache dirty lock poisoned")
            .insert(file_path.to_string());
        Some(grant)
    }

    /// Mark the entry as completed and evict it from memory. The lane
    /// caller is expected to issue the actual `DELETE FROM
    /// FileVectorizationQueue` against DB in the same iteration; the
    /// cache no longer tracks done files. Evict-on-done is the
    /// canonical semantic — keeping a Done state in cache would race
    /// the DB DELETE with the periodic upsert flush and re-create the
    /// row.
    pub fn mark_done(&self, file_path: &str, _now_ms: i64) {
        let mut entries = self.entries.write().expect("hot cache write lock poisoned");
        entries.remove(file_path);
        drop(entries);
        self.dirty
            .lock()
            .expect("hot cache dirty lock poisoned")
            .remove(file_path);
    }

    pub fn mark_failed(&self, file_path: &str, reason: &str, now_ms: i64) {
        let mut entries = self.entries.write().expect("hot cache write lock poisoned");
        let Some(entry) = entries.get_mut(file_path) else {
            return;
        };
        entry.status = FvqStatus::Failed;
        entry.last_change_at_ms = now_ms;
        entry.last_error = Some(reason.to_string());
        drop(entries);
        self.dirty
            .lock()
            .expect("hot cache dirty lock poisoned")
            .insert(file_path.to_string());
    }

    pub fn pending_for_lane(&self, limit: usize) -> Vec<String> {
        let entries = self.entries.read().expect("hot cache read lock poisoned");
        entries
            .iter()
            .filter(|(_, state)| state.status == FvqStatus::Ready)
            .take(limit)
            .map(|(path, _)| path.clone())
            .collect()
    }

    pub fn snapshot_dirty(&self) -> Vec<(String, FileHotState)> {
        let mut dirty = self.dirty.lock().expect("hot cache dirty lock poisoned");
        let entries = self.entries.read().expect("hot cache read lock poisoned");
        let snap: Vec<(String, FileHotState)> = dirty
            .iter()
            .filter_map(|path| entries.get(path).map(|state| (path.clone(), state.clone())))
            .collect();
        dirty.clear();
        snap
    }

    pub fn dirty_len(&self) -> usize {
        self.dirty
            .lock()
            .expect("hot cache dirty lock poisoned")
            .len()
    }

    pub fn entries_len(&self) -> usize {
        self.entries
            .read()
            .expect("hot cache read lock poisoned")
            .len()
    }

    pub fn upsert_from_db(&self, file_path: &str, state: FileHotState) {
        let mut entries = self.entries.write().expect("hot cache write lock poisoned");
        entries.insert(file_path.to_string(), state);
    }
}

fn fvq_status_str(status: FvqStatus) -> &'static str {
    match status {
        FvqStatus::Ready => "queued",
        FvqStatus::Inflight => "inflight",
        FvqStatus::Done => "done",
        FvqStatus::Failed => "failed",
    }
}

pub fn parse_fvq_status(s: &str) -> Option<FvqStatus> {
    match s {
        "queued" | "ready" => Some(FvqStatus::Ready),
        "inflight" => Some(FvqStatus::Inflight),
        "done" => Some(FvqStatus::Done),
        "failed" => Some(FvqStatus::Failed),
        _ => None,
    }
}

fn sql_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn sql_quote_opt(s: &Option<String>) -> String {
    match s {
        Some(v) => sql_quote(v),
        None => "NULL".to_string(),
    }
}

fn sql_i64_opt(v: Option<i64>) -> String {
    match v {
        Some(x) => x.to_string(),
        None => "NULL".to_string(),
    }
}

/// Render a snapshot of dirty cache entries into batched DuckDB upsert
/// queries. One INSERT...ON CONFLICT per chunk of <=500 rows; the
/// per-row payload mirrors the FileVectorizationQueue schema in
/// `graph_bootstrap::CREATE TABLE FileVectorizationQueue`. Empty
/// snapshot returns an empty Vec.
pub fn render_flush_queries(snap: &[(String, FileHotState)]) -> Vec<String> {
    if snap.is_empty() {
        return Vec::new();
    }
    let rows: Vec<String> = snap
        .iter()
        .map(|(path, state)| {
            format!(
                "({path}, {status}, {queued_at}, {claim_token}, {claimed_at}, {lease_owner}, {lease_epoch}, {last_err})",
                path = sql_quote(path),
                status = sql_quote(fvq_status_str(state.status)),
                queued_at = state.enqueued_at_ms,
                claim_token = sql_quote_opt(&state.claim_token),
                claimed_at = sql_i64_opt(state.started_at_ms),
                lease_owner = sql_quote_opt(&state.lease_owner),
                lease_epoch = state.lease_epoch,
                last_err = sql_quote_opt(&state.last_error),
            )
        })
        .collect();

    let mut queries = Vec::new();
    for chunk in rows.chunks(500) {
        queries.push(format!(
            "INSERT INTO FileVectorizationQueue (file_path, status, queued_at, claim_token, claimed_at_ms, lease_owner, lease_epoch, last_error_reason) VALUES {} \
             ON CONFLICT(file_path) DO UPDATE SET \
                status = EXCLUDED.status, \
                claim_token = EXCLUDED.claim_token, \
                claimed_at_ms = EXCLUDED.claimed_at_ms, \
                lease_owner = EXCLUDED.lease_owner, \
                lease_epoch = EXCLUDED.lease_epoch, \
                last_error_reason = EXCLUDED.last_error_reason;",
            chunk.join(",")
        ));
    }
    queries
}

impl Default for HotStatusCache {
    fn default() -> Self {
        Self::new()
    }
}

static CACHE: OnceLock<Arc<HotStatusCache>> = OnceLock::new();

pub fn install(cache: Arc<HotStatusCache>) -> bool {
    CACHE.set(cache).is_ok()
}

pub fn cache() -> Option<Arc<HotStatusCache>> {
    CACHE.get().cloned()
}

pub fn cache_enabled() -> bool {
    CACHE
        .get()
        .map(|c| c.is_enabled())
        .unwrap_or(false)
}

pub fn parse_env_enabled() -> bool {
    std::env::var("AXON_HOT_STATUS_CACHE_ENABLED")
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_cache() -> HotStatusCache {
        let c = HotStatusCache::new();
        c.set_enabled(true);
        c
    }

    #[test]
    fn mark_ready_creates_entry_and_marks_dirty() {
        let cache = fresh_cache();
        cache.mark_ready("/p/a.rs", 100);
        assert_eq!(cache.entries_len(), 1);
        assert_eq!(cache.dirty_len(), 1);
        assert_eq!(cache.pending_for_lane(10), vec!["/p/a.rs".to_string()]);
    }

    #[test]
    fn mark_done_evicts_entry_so_subsequent_mark_ready_starts_fresh() {
        let cache = fresh_cache();
        cache.mark_ready("/p/a.rs", 100);
        cache.try_claim("/p/a.rs", "lane-0", 110);
        cache.mark_done("/p/a.rs", 120);
        assert_eq!(cache.entries_len(), 0);
        assert_eq!(cache.dirty_len(), 0);

        cache.mark_ready("/p/a.rs", 200);
        assert_eq!(cache.pending_for_lane(10), vec!["/p/a.rs".to_string()]);
    }

    #[test]
    fn try_claim_succeeds_only_when_ready() {
        let cache = fresh_cache();
        cache.mark_ready("/p/a.rs", 100);

        let grant1 = cache.try_claim("/p/a.rs", "lane-0", 110);
        assert!(grant1.is_some(), "first claim should succeed");
        let grant1 = grant1.unwrap();
        assert!(grant1.claim_token.starts_with("hot-lane-0-110-"));
        assert_eq!(grant1.lease_epoch, 1);

        let grant2 = cache.try_claim("/p/a.rs", "lane-0", 120);
        assert!(grant2.is_none(), "second claim must fail (Inflight, not Ready)");

        let absent = cache.try_claim("/p/missing.rs", "lane-0", 130);
        assert!(absent.is_none(), "claim on absent file returns None");
    }

    #[test]
    fn pending_for_lane_filters_by_status() {
        let cache = fresh_cache();
        cache.mark_ready("/p/a.rs", 100);
        cache.mark_ready("/p/b.rs", 110);
        cache.mark_ready("/p/c.rs", 120);
        cache.try_claim("/p/b.rs", "lane-0", 130);
        cache.mark_done("/p/c.rs", 140);

        let pending: Vec<String> = cache.pending_for_lane(10);
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0], "/p/a.rs");
    }

    #[test]
    fn snapshot_dirty_empties_dirty_set() {
        let cache = fresh_cache();
        cache.mark_ready("/p/a.rs", 100);
        cache.mark_ready("/p/b.rs", 110);
        assert_eq!(cache.dirty_len(), 2);

        let snap = cache.snapshot_dirty();
        assert_eq!(snap.len(), 2);
        assert_eq!(cache.dirty_len(), 0);

        // After snapshot, additional changes re-mark dirty
        cache.mark_ready("/p/a.rs", 200);
        assert_eq!(cache.dirty_len(), 1);
    }

    #[test]
    fn render_flush_queries_emits_upsert_with_escaped_paths() {
        let cache = fresh_cache();
        cache.mark_ready("/p/a's.rs", 100);
        cache.try_claim("/p/a's.rs", "lane-0", 110);
        let snap = cache.snapshot_dirty();
        let queries = render_flush_queries(&snap);
        assert_eq!(queries.len(), 1);
        let q = &queries[0];
        assert!(q.contains("INSERT INTO FileVectorizationQueue"));
        assert!(q.contains("ON CONFLICT(file_path) DO UPDATE"));
        assert!(q.contains("'/p/a''s.rs'"), "path quoted: {q}");
        assert!(q.contains("'inflight'"), "status set: {q}");
    }

    #[test]
    fn render_flush_queries_empty_snapshot_returns_empty_vec() {
        let queries = render_flush_queries(&[]);
        assert!(queries.is_empty());
    }

    #[test]
    fn parse_env_enabled_honors_truthy_values() {
        let prev = std::env::var("AXON_HOT_STATUS_CACHE_ENABLED").ok();
        for v in ["1", "true", "TRUE", "yes", "on"] {
            std::env::set_var("AXON_HOT_STATUS_CACHE_ENABLED", v);
            assert!(parse_env_enabled(), "expected enabled for {v}");
        }
        for v in ["", "0", "false", "no", "off", "garbage"] {
            std::env::set_var("AXON_HOT_STATUS_CACHE_ENABLED", v);
            assert!(!parse_env_enabled(), "expected disabled for {v:?}");
        }
        match prev {
            Some(v) => std::env::set_var("AXON_HOT_STATUS_CACHE_ENABLED", v),
            None => std::env::remove_var("AXON_HOT_STATUS_CACHE_ENABLED"),
        }
    }
}
