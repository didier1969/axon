// REQ-AXO-193 direction E: route all hot-path DuckDB mutations through a
// single async writer thread to remove Writer Actor mutex contention from
// the producer hot path.
//   E.1 — typed contracts (this file's row structs, WriteDiff, accumulator)
//   E.2 — channel + writer thread + env-gated install (this file)
//   E.3 — port INSERT/UPDATE templates into render_bulk_queries
//   E.4 — vector lane sends FileVectorizationDone / ChunkEmbeddingPersist
//         through the same channel.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::{bounded, Receiver, RecvTimeoutError, Sender, TrySendError};
use tracing::{debug, info, warn};

use crate::graph::GraphStore;
use crate::graph_ingestion::FileVectorizationWork;

/// Hard cap on the channel capacity. Exceeding it backpressures the
/// producer (graph_projection / vector_lane) which is the desired
/// behavior — losing diffs is worse than throttling upstream.
const CHANNEL_CAPACITY: usize = 100;
/// Flush threshold in accumulated rows. ~10k rows = one large DuckDB
/// transaction (~50ms) per the operator's spec.
pub const ACCUMULATOR_BATCH: usize = 10_000;
/// Idle wake interval. Forces the writer to flush partial batches
/// instead of waiting for ACCUMULATOR_BATCH under low load.
pub const FLUSH_IDLE: Duration = Duration::from_millis(50);
/// Channel push timeout. Producers wait this long before returning a
/// dropped-diff error, which keeps the failure visible rather than
/// hiding it under an unbounded queue.
const SEND_TIMEOUT: Duration = Duration::from_millis(250);

const ENV_FLAG: &str = "AXON_ASYNC_WRITER_ENABLED";

#[derive(Debug, Clone, PartialEq)]
pub struct SymbolRow {
    pub symbol_id: String,
    pub name: String,
    pub kind: String,
    pub tested: bool,
    pub is_public: bool,
    pub is_nif: bool,
    pub is_unsafe: bool,
    pub project_code: String,
    pub embedding: Option<Vec<f32>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChunkRow {
    pub chunk_id: String,
    pub source_type: String,
    pub source_id: String,
    pub project_code: String,
    pub file_path: String,
    pub kind: String,
    pub content: String,
    pub content_hash: String,
    pub start_line: i64,
    pub end_line: i64,
    pub part_index: i64,
    pub part_count: i64,
    pub chunk_path: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RelationRow {
    pub source_id: String,
    pub target_id: String,
    pub project_code: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileWriteOutcome {
    Indexed,
    Degraded,
    Skipped,
    Deleted,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FileStateUpdate {
    pub paths: Vec<String>,
    pub outcome: FileWriteOutcome,
    pub at_ms: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChunkEmbeddingPersistRow {
    pub chunk_id: String,
    pub source_hash: String,
    pub embedding: Vec<f32>,
}

#[derive(Debug, Clone)]
pub enum WriteDiff {
    Symbols(Vec<SymbolRow>),
    Chunks(Vec<ChunkRow>),
    Contains(Vec<RelationRow>),
    Calls(Vec<RelationRow>),
    CallsNif(Vec<RelationRow>),
    FileState(FileStateUpdate),
    FileVectorizationDone(Vec<FileVectorizationWork>),
    ChunkEmbeddingPersist(Vec<ChunkEmbeddingPersistRow>),
}

#[derive(Debug, Default)]
pub struct WriteAccumulator {
    pub symbols: Vec<SymbolRow>,
    pub chunks: Vec<ChunkRow>,
    pub contains: Vec<RelationRow>,
    pub calls: Vec<RelationRow>,
    pub calls_nif: Vec<RelationRow>,
    pub file_state: Vec<FileStateUpdate>,
    pub vector_done: Vec<FileVectorizationWork>,
    pub embedding_persist: Vec<ChunkEmbeddingPersistRow>,
}

impl WriteAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn absorb(&mut self, diff: WriteDiff) {
        match diff {
            WriteDiff::Symbols(rows) => self.symbols.extend(rows),
            WriteDiff::Chunks(rows) => self.chunks.extend(rows),
            WriteDiff::Contains(rows) => self.contains.extend(rows),
            WriteDiff::Calls(rows) => self.calls.extend(rows),
            WriteDiff::CallsNif(rows) => self.calls_nif.extend(rows),
            WriteDiff::FileState(update) => self.file_state.push(update),
            WriteDiff::FileVectorizationDone(rows) => self.vector_done.extend(rows),
            WriteDiff::ChunkEmbeddingPersist(rows) => self.embedding_persist.extend(rows),
        }
    }

    pub fn row_count(&self) -> usize {
        self.symbols.len()
            + self.chunks.len()
            + self.contains.len()
            + self.calls.len()
            + self.calls_nif.len()
            + self.file_state.iter().map(|u| u.paths.len()).sum::<usize>()
            + self.vector_done.len()
            + self.embedding_persist.len()
    }

    pub fn is_empty(&self) -> bool {
        self.row_count() == 0
    }

    // E.3 ports the existing INSERT/UPDATE templates from
    // insert_file_data_batch_with_vectorization_policy. Until then the
    // accumulator is a no-op renderer so E.2's writer thread loop can
    // wire end-to-end without the SQL surface.
    pub fn render_bulk_queries(&self) -> Vec<String> {
        Vec::new()
    }

    pub fn reset(&mut self) {
        self.symbols.clear();
        self.chunks.clear();
        self.contains.clear();
        self.calls.clear();
        self.calls_nif.clear();
        self.file_state.clear();
        self.vector_done.clear();
        self.embedding_persist.clear();
    }
}

#[derive(Debug, Default)]
pub struct WriterStats {
    pub diffs_sent: AtomicUsize,
    pub diffs_dropped: AtomicUsize,
    pub flushes: AtomicUsize,
    pub rows_drained: AtomicUsize,
    pub flush_failures: AtomicUsize,
}

impl WriterStats {
    pub fn diffs_sent(&self) -> usize {
        self.diffs_sent.load(Ordering::Relaxed)
    }
    pub fn diffs_dropped(&self) -> usize {
        self.diffs_dropped.load(Ordering::Relaxed)
    }
    pub fn flushes(&self) -> usize {
        self.flushes.load(Ordering::Relaxed)
    }
    pub fn rows_drained(&self) -> usize {
        self.rows_drained.load(Ordering::Relaxed)
    }
    pub fn flush_failures(&self) -> usize {
        self.flush_failures.load(Ordering::Relaxed)
    }
}

pub trait WriterSink: Send + Sync + 'static {
    fn execute_batch(&self, queries: &[String]) -> anyhow::Result<()>;
}

impl WriterSink for GraphStore {
    fn execute_batch(&self, queries: &[String]) -> anyhow::Result<()> {
        GraphStore::execute_batch(self, queries)
    }
}

#[derive(Debug)]
pub struct WriteDispatcher {
    tx: Sender<WriteDiff>,
    stats: Arc<WriterStats>,
}

impl WriteDispatcher {
    pub fn dispatch(&self, diff: WriteDiff) -> Result<(), TrySendError<WriteDiff>> {
        match self.tx.send_timeout(diff, SEND_TIMEOUT) {
            Ok(()) => {
                self.stats.diffs_sent.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }
            Err(crossbeam_channel::SendTimeoutError::Timeout(diff)) => {
                self.stats.diffs_dropped.fetch_add(1, Ordering::Relaxed);
                Err(TrySendError::Full(diff))
            }
            Err(crossbeam_channel::SendTimeoutError::Disconnected(diff)) => {
                self.stats.diffs_dropped.fetch_add(1, Ordering::Relaxed);
                Err(TrySendError::Disconnected(diff))
            }
        }
    }

    pub fn stats(&self) -> &WriterStats {
        &self.stats
    }
}

static GLOBAL_DISPATCHER: OnceLock<Arc<WriteDispatcher>> = OnceLock::new();

pub fn dispatcher() -> Option<Arc<WriteDispatcher>> {
    GLOBAL_DISPATCHER.get().cloned()
}

pub fn async_writer_enabled() -> bool {
    std::env::var(ENV_FLAG)
        .ok()
        .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true"))
        .unwrap_or(false)
}

/// Spawn the writer thread + install the global dispatcher. Returns the
/// installed dispatcher (or None when the env flag is off). Honors
/// `AXON_ASYNC_WRITER_ENABLED` — disabled = producers fall through to the
/// legacy synchronous path. The writer thread is detached.
pub fn install_global<S: WriterSink>(sink: Arc<S>) -> Option<Arc<WriteDispatcher>> {
    if !async_writer_enabled() {
        return None;
    }
    let (dispatcher, _handle) = spawn(sink);
    let _ = GLOBAL_DISPATCHER.set(Arc::clone(&dispatcher));
    Some(GLOBAL_DISPATCHER.get().cloned().unwrap_or(dispatcher))
}

/// Spawn an isolated writer thread + dispatcher (no global install).
/// Returns both the dispatcher and the JoinHandle so tests can drop the
/// dispatcher and wait for the writer to exit. Production callers use
/// `install_global` and discard the handle.
pub fn spawn<S: WriterSink>(sink: Arc<S>) -> (Arc<WriteDispatcher>, thread::JoinHandle<()>) {
    let (tx, rx) = bounded::<WriteDiff>(CHANNEL_CAPACITY);
    let stats = Arc::new(WriterStats::default());
    let stats_for_loop = Arc::clone(&stats);
    let handle = thread::Builder::new()
        .name("axon-async-writer".to_string())
        .spawn(move || writer_loop(rx, sink, stats_for_loop))
        .expect("axon-async-writer thread spawn");
    info!(
        "async_writer: dispatcher installed (channel={}, batch={}, idle_ms={})",
        CHANNEL_CAPACITY,
        ACCUMULATOR_BATCH,
        FLUSH_IDLE.as_millis(),
    );
    (Arc::new(WriteDispatcher { tx, stats }), handle)
}

fn writer_loop<S: WriterSink>(
    rx: Receiver<WriteDiff>,
    sink: Arc<S>,
    stats: Arc<WriterStats>,
) {
    let mut accumulator = WriteAccumulator::new();
    let mut last_flush = Instant::now();
    loop {
        match rx.recv_timeout(FLUSH_IDLE) {
            Ok(diff) => accumulator.absorb(diff),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                if !accumulator.is_empty() {
                    flush(&sink, &mut accumulator, &stats);
                }
                debug!("async_writer: channel disconnected; writer loop exiting");
                return;
            }
        }

        let row_count = accumulator.row_count();
        let elapsed = last_flush.elapsed();
        let batch_full = row_count >= ACCUMULATOR_BATCH;
        let idle_flush = row_count > 0 && elapsed >= FLUSH_IDLE;
        if batch_full || idle_flush {
            flush(&sink, &mut accumulator, &stats);
            last_flush = Instant::now();
        }
    }
}

fn flush<S: WriterSink>(
    sink: &Arc<S>,
    accumulator: &mut WriteAccumulator,
    stats: &Arc<WriterStats>,
) {
    let drained = accumulator.row_count();
    let queries = accumulator.render_bulk_queries();
    if !queries.is_empty() {
        if let Err(e) = sink.execute_batch(&queries) {
            stats.flush_failures.fetch_add(1, Ordering::Relaxed);
            warn!("async_writer: flush failed ({} rows): {:?}", drained, e);
        }
    }
    accumulator.reset();
    stats.flushes.fetch_add(1, Ordering::Relaxed);
    stats.rows_drained.fetch_add(drained, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_symbol(id: &str) -> SymbolRow {
        SymbolRow {
            symbol_id: id.to_string(),
            name: "alpha".to_string(),
            kind: "function".to_string(),
            tested: true,
            is_public: false,
            is_nif: false,
            is_unsafe: false,
            project_code: "AXO".to_string(),
            embedding: None,
        }
    }

    fn sample_chunk(id: &str) -> ChunkRow {
        ChunkRow {
            chunk_id: id.to_string(),
            source_type: "symbol".to_string(),
            source_id: "sym-1".to_string(),
            project_code: "AXO".to_string(),
            file_path: "/tmp/a.rs".to_string(),
            kind: "function".to_string(),
            content: "fn alpha() {}".to_string(),
            content_hash: "abc".to_string(),
            start_line: 1,
            end_line: 1,
            part_index: 0,
            part_count: 1,
            chunk_path: "/tmp/a.rs#alpha".to_string(),
        }
    }

    fn sample_relation(src: &str, tgt: &str) -> RelationRow {
        RelationRow {
            source_id: src.to_string(),
            target_id: tgt.to_string(),
            project_code: "AXO".to_string(),
        }
    }

    #[test]
    fn accumulator_starts_empty() {
        let acc = WriteAccumulator::new();
        assert_eq!(acc.row_count(), 0);
        assert!(acc.is_empty());
        assert!(acc.render_bulk_queries().is_empty());
    }

    #[test]
    fn absorb_extends_buckets_per_variant() {
        let mut acc = WriteAccumulator::new();
        acc.absorb(WriteDiff::Symbols(vec![
            sample_symbol("s1"),
            sample_symbol("s2"),
        ]));
        acc.absorb(WriteDiff::Chunks(vec![sample_chunk("c1")]));
        acc.absorb(WriteDiff::Contains(vec![sample_relation("a", "b")]));
        acc.absorb(WriteDiff::Calls(vec![
            sample_relation("a", "b"),
            sample_relation("c", "d"),
        ]));
        acc.absorb(WriteDiff::CallsNif(vec![sample_relation("e", "f")]));
        acc.absorb(WriteDiff::FileState(FileStateUpdate {
            paths: vec!["/tmp/a.rs".to_string(), "/tmp/b.rs".to_string()],
            outcome: FileWriteOutcome::Indexed,
            at_ms: 1000,
        }));
        acc.absorb(WriteDiff::FileState(FileStateUpdate {
            paths: vec!["/tmp/c.rs".to_string()],
            outcome: FileWriteOutcome::Degraded,
            at_ms: 1100,
        }));
        acc.absorb(WriteDiff::FileState(FileStateUpdate {
            paths: vec!["/tmp/d.rs".to_string()],
            outcome: FileWriteOutcome::Deleted,
            at_ms: 1200,
        }));
        acc.absorb(WriteDiff::FileVectorizationDone(vec![FileVectorizationWork {
            file_path: "/tmp/a.rs".to_string(),
            resumed_after_interactive_pause: false,
        }]));
        acc.absorb(WriteDiff::ChunkEmbeddingPersist(vec![
            ChunkEmbeddingPersistRow {
                chunk_id: "c1".to_string(),
                source_hash: "abc".to_string(),
                embedding: vec![0.1; 1024],
            },
        ]));

        assert_eq!(acc.symbols.len(), 2);
        assert_eq!(acc.chunks.len(), 1);
        assert_eq!(acc.contains.len(), 1);
        assert_eq!(acc.calls.len(), 2);
        assert_eq!(acc.calls_nif.len(), 1);
        assert_eq!(acc.file_state.len(), 3);
        assert_eq!(acc.vector_done.len(), 1);
        assert_eq!(acc.embedding_persist.len(), 1);
        // file_state contributes paths.len() per update to row_count.
        // 2 + 1 + 1 = 4 paths from the three FileState updates.
        assert_eq!(acc.row_count(), 2 + 1 + 1 + 2 + 1 + 4 + 1 + 1);
        assert!(!acc.is_empty());
    }

    #[test]
    fn absorb_appends_within_same_variant_across_calls() {
        let mut acc = WriteAccumulator::new();
        acc.absorb(WriteDiff::Chunks(vec![sample_chunk("c1")]));
        acc.absorb(WriteDiff::Chunks(vec![sample_chunk("c2")]));
        acc.absorb(WriteDiff::Chunks(vec![sample_chunk("c3"), sample_chunk("c4")]));
        assert_eq!(acc.chunks.len(), 4);
        assert_eq!(acc.row_count(), 4);
    }

    #[test]
    fn reset_clears_every_bucket() {
        let mut acc = WriteAccumulator::new();
        acc.absorb(WriteDiff::Symbols(vec![sample_symbol("s1")]));
        acc.absorb(WriteDiff::Chunks(vec![sample_chunk("c1")]));
        acc.absorb(WriteDiff::FileState(FileStateUpdate {
            paths: vec!["/tmp/a.rs".to_string()],
            outcome: FileWriteOutcome::Skipped,
            at_ms: 0,
        }));
        acc.absorb(WriteDiff::FileVectorizationDone(vec![FileVectorizationWork {
            file_path: "/tmp/a.rs".to_string(),
            resumed_after_interactive_pause: true,
        }]));
        assert!(acc.row_count() > 0);
        acc.reset();
        assert!(acc.is_empty());
        assert_eq!(acc.symbols.len(), 0);
        assert_eq!(acc.chunks.len(), 0);
        assert_eq!(acc.file_state.len(), 0);
        assert_eq!(acc.vector_done.len(), 0);
    }

    #[test]
    fn render_bulk_queries_is_empty_until_e3() {
        // E.1 stubs SQL rendering. E.3 will port the existing
        // INSERT/UPDATE templates and replace this assertion with shape
        // checks on the generated batch.
        let mut acc = WriteAccumulator::new();
        acc.absorb(WriteDiff::Symbols(vec![sample_symbol("s1")]));
        acc.absorb(WriteDiff::Chunks(vec![sample_chunk("c1")]));
        assert!(acc.render_bulk_queries().is_empty());
    }

    #[test]
    fn write_diff_round_trips_through_accumulator_via_clone() {
        // WriteDiff is the channel payload; it must be Clone so producers
        // can keep rows around for retry / fallback paths if the writer
        // thread is unreachable.
        let diff = WriteDiff::Chunks(vec![sample_chunk("c1")]);
        let mut acc = WriteAccumulator::new();
        acc.absorb(diff.clone());
        acc.absorb(diff);
        assert_eq!(acc.chunks.len(), 2);
    }

    use std::sync::Mutex;

    #[derive(Default)]
    struct RecordingSink {
        calls: Mutex<Vec<Vec<String>>>,
        fail_next: AtomicUsize,
    }
    impl RecordingSink {
        fn batches(&self) -> Vec<Vec<String>> {
            self.calls.lock().unwrap().clone()
        }
    }
    impl WriterSink for RecordingSink {
        fn execute_batch(&self, queries: &[String]) -> anyhow::Result<()> {
            if self.fail_next.load(Ordering::Relaxed) > 0 {
                self.fail_next.fetch_sub(1, Ordering::Relaxed);
                anyhow::bail!("forced flush failure");
            }
            self.calls.lock().unwrap().push(queries.to_vec());
            Ok(())
        }
    }

    fn wait_for<F: FnMut() -> bool>(mut cond: F, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if cond() {
                return true;
            }
            thread::sleep(Duration::from_millis(5));
        }
        cond()
    }

    #[test]
    fn dispatcher_send_and_writer_drains_on_idle_flush() {
        let sink = Arc::new(RecordingSink::default());
        let (dispatcher, handle) = spawn(Arc::clone(&sink));

        dispatcher
            .dispatch(WriteDiff::Chunks(vec![sample_chunk("c1"), sample_chunk("c2")]))
            .expect("dispatch ok");

        assert!(
            wait_for(
                || dispatcher.stats().rows_drained() >= 2,
                Duration::from_secs(2),
            ),
            "writer should drain rows within idle flush window: stats={:?}",
            dispatcher.stats(),
        );
        assert_eq!(dispatcher.stats().diffs_sent(), 1);
        assert_eq!(dispatcher.stats().diffs_dropped(), 0);
        assert_eq!(dispatcher.stats().flush_failures(), 0);
        assert!(dispatcher.stats().flushes() >= 1);
        // render_bulk_queries returns empty Vec until E.3, so the sink
        // never sees a non-empty query batch even though rows drained.
        assert!(sink.batches().is_empty());

        drop(dispatcher);
        handle
            .join()
            .expect("writer thread should exit cleanly after channel disconnect");
    }

    #[test]
    fn writer_loop_exits_when_dispatcher_dropped() {
        let sink = Arc::new(RecordingSink::default());
        let (dispatcher, handle) = spawn(Arc::clone(&sink));
        drop(dispatcher);
        // join blocks until writer_loop returns. Without the
        // Disconnected branch this would hang.
        handle.join().expect("writer thread joined after disconnect");
    }

    #[test]
    fn flush_failure_counter_increments_on_sink_error() {
        let sink = Arc::new(RecordingSink::default());
        sink.fail_next.store(3, Ordering::Relaxed);
        let (dispatcher, handle) = spawn(Arc::clone(&sink));

        // render_bulk_queries returns empty until E.3, so the sink path
        // is never exercised today — confirm the counter stays at zero
        // after the rows drain. Once E.3 lands and renders real queries,
        // flush failures will start incrementing on real DB errors. Test
        // acts as a regression sentinel for the wiring.
        for _ in 0..5 {
            dispatcher
                .dispatch(WriteDiff::Chunks(vec![sample_chunk("c1")]))
                .expect("dispatch ok");
        }
        assert!(wait_for(
            || dispatcher.stats().rows_drained() >= 5,
            Duration::from_secs(2),
        ));
        assert_eq!(dispatcher.stats().flush_failures(), 0);
        // Sink's fail_next was primed but render returned no queries, so
        // the sink wasn't called — leaving fail_next untouched.
        assert_eq!(sink.fail_next.load(Ordering::Relaxed), 3);
        drop(dispatcher);
        handle.join().expect("writer thread joined");
    }

    #[test]
    fn async_writer_enabled_honors_env_flag() {
        // Snapshot + restore so parallel tests don't fight over the env.
        let prior = std::env::var(ENV_FLAG).ok();
        std::env::remove_var(ENV_FLAG);
        assert!(!async_writer_enabled());
        std::env::set_var(ENV_FLAG, "true");
        assert!(async_writer_enabled());
        std::env::set_var(ENV_FLAG, "0");
        assert!(!async_writer_enabled());
        match prior {
            Some(v) => std::env::set_var(ENV_FLAG, v),
            None => std::env::remove_var(ENV_FLAG),
        }
    }
}
