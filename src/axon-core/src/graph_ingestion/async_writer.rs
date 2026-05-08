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
    /// E.3a: passthrough variant carrying pre-rendered SQL. Lets the
    /// vector-lane mark-done call (the −56% regression in VAL-AXO-040)
    /// move off the producer thread without waiting for the typed-row
    /// port (E.3 / E.7). Writer accumulates these strings and flushes
    /// them inside a single transaction.
    RawQueries(Vec<String>),
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
    pub raw_queries: Vec<String>,
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
            WriteDiff::RawQueries(rows) => self.raw_queries.extend(rows),
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
            + self.raw_queries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.row_count() == 0
    }

    /// REQ-AXO-238 / REQ-AXO-193 E.7: render the accumulator's typed
    /// buckets into bulk INSERT…ON CONFLICT SQL statements that match
    /// the producer's legacy output bit-for-bit, then chain the
    /// E.3a RawQueries passthrough.
    ///
    /// Order matters: Symbols → Chunks → RawQueries. Symbols come
    /// before Chunks because Chunk.source_id may reference a freshly-
    /// inserted Symbol.id; the legacy producer emits them in that
    /// order at graph_ingestion.rs:920/927. RawQueries land last
    /// because they typically carry trailing UPDATE/DELETE that
    /// depend on the typed INSERTs already taking effect.
    pub fn render_bulk_queries(&self) -> Vec<String> {
        let mut out = Vec::new();
        out.extend(self.render_symbols_duckdb());
        out.extend(self.render_chunks_duckdb());
        out.extend(self.raw_queries.iter().cloned());
        out
    }

    /// Render accumulated `SymbolRow`s into INSERT…ON CONFLICT
    /// statements chunked at 500 rows per query. Format matches
    /// `graph_ingestion.rs:617-628 + 920-923` byte-for-byte under the
    /// DuckDB backend, including the embedding-column CAST literal
    /// (`CAST([0.1, 0.2, ...] AS FLOAT[1024])`) when set.
    pub fn render_symbols_duckdb(&self) -> Vec<String> {
        if self.symbols.is_empty() {
            return Vec::new();
        }
        use super::sql_helpers::escape_sql_text;
        use crate::embedding_contract::DIMENSION;
        let mut queries = Vec::with_capacity(self.symbols.len() / 500 + 1);
        for batch in self.symbols.chunks(500) {
            let values: Vec<String> = batch
                .iter()
                .map(|s| {
                    let embedding_sql = match s.embedding.as_ref() {
                        Some(v) => format!("CAST({:?} AS FLOAT[{DIMENSION}])", v),
                        None => "NULL".to_string(),
                    };
                    format!(
                        "('{}', '{}', '{}', {}, {}, {}, {}, '{}', {})",
                        escape_sql_text(&s.symbol_id),
                        escape_sql_text(&s.name),
                        s.kind,
                        s.tested,
                        s.is_public,
                        s.is_nif,
                        s.is_unsafe,
                        escape_sql_text(&s.project_code),
                        embedding_sql,
                    )
                })
                .collect();
            queries.push(format!(
                "INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, is_unsafe, project_code, embedding) VALUES {} ON CONFLICT(id) DO UPDATE SET name=EXCLUDED.name, kind=EXCLUDED.kind, tested=EXCLUDED.tested, is_public=EXCLUDED.is_public, is_nif=EXCLUDED.is_nif, is_unsafe=EXCLUDED.is_unsafe, project_code=EXCLUDED.project_code, embedding=EXCLUDED.embedding;",
                values.join(",")
            ));
        }
        queries
    }

    /// Render accumulated `ChunkRow`s into INSERT…ON CONFLICT statements
    /// chunked at 500 rows per query (mirrors the legacy producer batch
    /// size in `graph_ingestion.rs:925`). Format matches the legacy
    /// emitter at `graph_ingestion.rs:927-930` byte-for-byte under the
    /// DuckDB backend.
    pub fn render_chunks_duckdb(&self) -> Vec<String> {
        if self.chunks.is_empty() {
            return Vec::new();
        }
        use super::sql_helpers::escape_sql_text;
        let mut queries = Vec::with_capacity(self.chunks.len() / 500 + 1);
        for batch in self.chunks.chunks(500) {
            let values: Vec<String> = batch
                .iter()
                .map(|c| {
                    format!(
                        "('{}', '{}', '{}', '{}', '{}', '{}', '{}', '{}', {}, {}, {}, {}, '{}')",
                        escape_sql_text(&c.chunk_id),
                        escape_sql_text(&c.source_type),
                        escape_sql_text(&c.source_id),
                        escape_sql_text(&c.project_code),
                        escape_sql_text(&c.file_path),
                        escape_sql_text(&c.kind),
                        escape_sql_text(&c.content),
                        escape_sql_text(&c.content_hash),
                        c.start_line,
                        c.end_line,
                        c.part_index,
                        c.part_count,
                        escape_sql_text(&c.chunk_path),
                    )
                })
                .collect();
            queries.push(format!(
                "INSERT INTO Chunk (id, source_type, source_id, project_code, file_path, kind, content, content_hash, start_line, end_line, chunk_part_index, chunk_part_count, chunk_path) VALUES {} \
                 ON CONFLICT(id) DO UPDATE SET source_type=EXCLUDED.source_type, source_id=EXCLUDED.source_id, project_code=EXCLUDED.project_code, file_path=EXCLUDED.file_path, kind=EXCLUDED.kind, content=EXCLUDED.content, content_hash=EXCLUDED.content_hash, start_line=EXCLUDED.start_line, end_line=EXCLUDED.end_line, chunk_part_index=EXCLUDED.chunk_part_index, chunk_part_count=EXCLUDED.chunk_part_count, chunk_path=EXCLUDED.chunk_path;",
                values.join(",")
            ));
        }
        queries
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
        self.raw_queries.clear();
    }
}

/// Send a pre-rendered batch of writer queries through the async writer
/// when it's installed; fall through to the synchronous `execute_batch`
/// otherwise. Used by call sites that previously held the writer mutex
/// directly (e.g. `mark_file_vectorization_work_done`) and don't depend
/// on read-after-write visibility within the producer thread.
pub fn route_writer_batch(graph_store: &GraphStore, queries: &[String]) -> anyhow::Result<()> {
    if queries.is_empty() {
        return Ok(());
    }
    if let Some(disp) = dispatcher() {
        match disp.dispatch(WriteDiff::RawQueries(queries.to_vec())) {
            Ok(()) => return Ok(()),
            Err(e) => {
                warn!(
                    "async_writer: dispatch failed ({} queries); falling through to sync: {:?}",
                    queries.len(),
                    e
                );
            }
        }
    }
    graph_store.execute_batch(queries)
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
    fn render_emits_symbols_then_chunks_then_raw_queries() {
        // E.7 (REQ-AXO-238): both Symbols and Chunks render. Order
        // contract: Symbols → Chunks → RawQueries. Symbols come before
        // Chunks because Chunk.source_id may reference a freshly-
        // inserted Symbol.id; legacy producer order at
        // graph_ingestion.rs:920 (Symbols) then :927 (Chunks).
        let mut acc = WriteAccumulator::new();
        acc.absorb(WriteDiff::Symbols(vec![sample_symbol("s1")]));
        let rendered = acc.render_bulk_queries();
        assert_eq!(rendered.len(), 1, "single Symbol batch -> single query");
        assert!(rendered[0].starts_with("INSERT INTO Symbol"));

        acc.absorb(WriteDiff::Chunks(vec![sample_chunk("c1")]));
        let rendered = acc.render_bulk_queries();
        assert_eq!(rendered.len(), 2, "Symbols + Chunks -> two queries");
        assert!(rendered[0].starts_with("INSERT INTO Symbol"));
        assert!(rendered[1].starts_with("INSERT INTO Chunk"));

        acc.absorb(WriteDiff::RawQueries(vec![
            "DELETE FROM FileVectorizationQueue WHERE file_path = '/tmp/a.rs'".to_string(),
            "UPDATE File SET vector_ready = TRUE WHERE path = '/tmp/a.rs'".to_string(),
        ]));
        let rendered = acc.render_bulk_queries();
        assert_eq!(rendered.len(), 4);
        assert!(rendered[0].starts_with("INSERT INTO Symbol"));
        assert!(rendered[1].starts_with("INSERT INTO Chunk"));
        assert!(rendered[2].starts_with("DELETE FROM FileVectorizationQueue"));
        assert!(rendered[3].starts_with("UPDATE File"));
    }

    #[test]
    fn render_symbols_duckdb_matches_legacy_producer_format() {
        // Parity gate for E.7 Symbol slice. Mirrors graph_ingestion.rs:
        // 617-628 (value tuple shape) + :920-923 (header + ON CONFLICT
        // clause). Embedding-column branch covered:
        //   - None  -> NULL literal
        //   - Some  -> CAST([f1, f2, ...] AS FLOAT[1024])
        let mut acc = WriteAccumulator::new();
        acc.absorb(WriteDiff::Symbols(vec![
            SymbolRow {
                symbol_id: "AXO::path::no_embed".to_string(),
                name: "alpha".to_string(),
                kind: "function".to_string(),
                tested: true,
                is_public: false,
                is_nif: false,
                is_unsafe: false,
                project_code: "AXO".to_string(),
                embedding: None,
            },
            SymbolRow {
                symbol_id: "AXO::path::with_embed".to_string(),
                name: "beta".to_string(),
                kind: "function".to_string(),
                tested: false,
                is_public: true,
                is_nif: false,
                is_unsafe: true,
                project_code: "AXO".to_string(),
                embedding: Some(vec![0.1_f32, 0.2_f32, -0.3_f32]),
            },
        ]));
        let rendered = acc.render_symbols_duckdb();
        assert_eq!(rendered.len(), 1, "two symbols fit in a single 500-row batch");
        let q = &rendered[0];

        // Header + ON CONFLICT clause shape.
        assert!(q.starts_with(
            "INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, is_unsafe, project_code, embedding) VALUES "
        ));
        assert!(q.contains(
            "ON CONFLICT(id) DO UPDATE SET name=EXCLUDED.name, kind=EXCLUDED.kind, tested=EXCLUDED.tested, is_public=EXCLUDED.is_public, is_nif=EXCLUDED.is_nif, is_unsafe=EXCLUDED.is_unsafe, project_code=EXCLUDED.project_code, embedding=EXCLUDED.embedding;"
        ));

        // Boolean columns rendered as bare `true`/`false` literals
        // (DuckDB accepts both keywords). Integer columns absent here.
        assert!(q.contains(", true, false, false, false, "), "first row booleans: {q}");
        assert!(q.contains(", false, true, false, true, "), "second row booleans: {q}");

        // Embedding branch: None -> NULL, Some -> CAST literal with the
        // canonical FLOAT[1024] cast.
        assert!(q.contains(", NULL)"), "no-embed row should emit NULL: {q}");
        assert!(
            q.contains("CAST([0.1, 0.2, -0.3] AS FLOAT[1024])"),
            "embed row should emit CAST literal: {q}"
        );
    }

    #[test]
    fn render_symbols_duckdb_empty_returns_empty() {
        let acc = WriteAccumulator::new();
        assert!(acc.render_symbols_duckdb().is_empty());
    }

    #[test]
    fn render_symbols_duckdb_batches_at_500_rows() {
        let mut acc = WriteAccumulator::new();
        let rows: Vec<SymbolRow> = (0..1100).map(|i| sample_symbol(&format!("s{i}"))).collect();
        acc.absorb(WriteDiff::Symbols(rows));
        let rendered = acc.render_symbols_duckdb();
        assert_eq!(rendered.len(), 3, "1100 symbols split into 500/500/100 batches");
        for q in &rendered {
            assert!(q.starts_with("INSERT INTO Symbol"));
            assert!(q.ends_with("embedding=EXCLUDED.embedding;"));
        }
    }

    #[test]
    fn render_chunks_duckdb_matches_legacy_producer_format() {
        // Parity gate for E.7: the rendered INSERT must be byte-for-byte
        // equivalent to what graph_ingestion.rs:925-930 produces today.
        // Any drift here = silent regression on the writer-side path.
        let mut acc = WriteAccumulator::new();
        acc.absorb(WriteDiff::Chunks(vec![ChunkRow {
            chunk_id: "AXO::path::sym::0::1".to_string(),
            source_type: "symbol".to_string(),
            source_id: "AXO::path::sym".to_string(),
            project_code: "AXO".to_string(),
            file_path: "/tmp/a.rs".to_string(),
            kind: "function".to_string(),
            content: "fn alpha() {\n  println!(\"hi 'world'\");\n}".to_string(),
            content_hash: "deadbeef".to_string(),
            start_line: 1,
            end_line: 3,
            part_index: 0,
            part_count: 1,
            chunk_path: "/tmp/a.rs#alpha".to_string(),
        }]));
        let rendered = acc.render_chunks_duckdb();
        assert_eq!(rendered.len(), 1);
        let q = &rendered[0];

        // Header + ON CONFLICT clause shape (verbatim from legacy emit).
        assert!(q.starts_with(
            "INSERT INTO Chunk (id, source_type, source_id, project_code, file_path, kind, content, content_hash, start_line, end_line, chunk_part_index, chunk_part_count, chunk_path) VALUES "
        ));
        assert!(q.contains(
            "ON CONFLICT(id) DO UPDATE SET source_type=EXCLUDED.source_type, source_id=EXCLUDED.source_id, project_code=EXCLUDED.project_code, file_path=EXCLUDED.file_path, kind=EXCLUDED.kind, content=EXCLUDED.content, content_hash=EXCLUDED.content_hash, start_line=EXCLUDED.start_line, end_line=EXCLUDED.end_line, chunk_part_index=EXCLUDED.chunk_part_index, chunk_part_count=EXCLUDED.chunk_part_count, chunk_path=EXCLUDED.chunk_path;"
        ));

        // Single-quote escape: the apostrophe inside `"hi 'world'"` must
        // become two single quotes per ANSI SQL (and per legacy
        // `escape_sql`). Asserting the rendered tuple contains the
        // doubled single quote ensures the helper is wired.
        assert!(
            q.contains("''world''"),
            "expected single-quote doubling, got: {q}"
        );
        // start_line, end_line, part_index, part_count must be unquoted
        // integers (i64 literals), not '1' / '0'.
        assert!(q.contains(", 1, 3, 0, 1, "), "integer columns must be unquoted: {q}");
    }

    #[test]
    fn render_chunks_duckdb_batches_at_500_rows() {
        // Mirrors `chunk_values.chunks(500)` in graph_ingestion.rs:925
        // so that one accumulator flush of 1200 chunks yields exactly
        // ceil(1200 / 500) = 3 INSERT statements.
        let mut acc = WriteAccumulator::new();
        let rows: Vec<ChunkRow> = (0..1200)
            .map(|i| {
                let mut c = sample_chunk(&format!("c{i}"));
                c.start_line = i as i64;
                c.end_line = i as i64;
                c
            })
            .collect();
        acc.absorb(WriteDiff::Chunks(rows));
        let rendered = acc.render_chunks_duckdb();
        assert_eq!(rendered.len(), 3, "1200 chunks split into 500/500/200 batches");
        for q in &rendered {
            assert!(q.starts_with("INSERT INTO Chunk"));
            assert!(q.ends_with("chunk_path=EXCLUDED.chunk_path;"));
        }
    }

    #[test]
    fn render_chunks_duckdb_empty_returns_empty() {
        let acc = WriteAccumulator::new();
        assert!(acc.render_chunks_duckdb().is_empty());
    }

    #[test]
    fn raw_queries_absorb_appends_and_reset_clears() {
        let mut acc = WriteAccumulator::new();
        acc.absorb(WriteDiff::RawQueries(vec!["q1".to_string()]));
        acc.absorb(WriteDiff::RawQueries(vec!["q2".to_string(), "q3".to_string()]));
        assert_eq!(acc.row_count(), 3);
        assert_eq!(acc.raw_queries.len(), 3);
        acc.reset();
        assert!(acc.raw_queries.is_empty());
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
        // E.7 (REQ-AXO-238): typed Chunks now render to a single bulk
        // INSERT batch. Sink sees exactly one execute_batch call with
        // exactly one query (the INSERT for the two chunks).
        let batches = sink.batches();
        assert_eq!(batches.len(), 1, "one flush -> one execute_batch call");
        assert_eq!(batches[0].len(), 1, "two Chunks fit in a single 500-row batch");
        assert!(batches[0][0].starts_with("INSERT INTO Chunk"));

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

        // E.7 (REQ-AXO-238): typed Chunks now render to a real INSERT
        // statement, so the sink IS called. With fail_next=3, the next
        // 3 sink calls fail; subsequent flushes succeed. We wait until
        // the failure counter has incremented before asserting.
        for _ in 0..5 {
            dispatcher
                .dispatch(WriteDiff::Chunks(vec![sample_chunk("c1")]))
                .expect("dispatch ok");
        }
        assert!(wait_for(
            || dispatcher.stats().rows_drained() >= 5,
            Duration::from_secs(2),
        ));
        // At least one flush hit the failing sink, so flush_failures > 0.
        assert!(
            dispatcher.stats().flush_failures() >= 1,
            "expected >=1 flush failure once sink was primed, got {:?}",
            dispatcher.stats(),
        );
        // The sink's fail_next was decremented by each failed call, so
        // it's strictly below the seeded 3.
        assert!(
            sink.fail_next.load(Ordering::Relaxed) < 3,
            "sink.fail_next should have been consumed by failed flushes",
        );
        drop(dispatcher);
        handle.join().expect("writer thread joined");
    }

    #[test]
    fn raw_queries_reach_sink_through_dispatcher() {
        let sink = Arc::new(RecordingSink::default());
        let (dispatcher, handle) = spawn(Arc::clone(&sink));

        dispatcher
            .dispatch(WriteDiff::RawQueries(vec![
                "DELETE FROM FileVectorizationQueue WHERE file_path = '/tmp/a.rs'".to_string(),
                "UPDATE File SET vector_ready = TRUE WHERE path = '/tmp/a.rs'".to_string(),
            ]))
            .expect("dispatch ok");

        assert!(
            wait_for(
                || !sink.batches().is_empty(),
                Duration::from_secs(2),
            ),
            "sink should receive flushed RawQueries: stats={:?}",
            dispatcher.stats(),
        );
        let batches = sink.batches();
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].len(), 2);
        assert!(batches[0][0].starts_with("DELETE FROM FileVectorizationQueue"));
        assert!(batches[0][1].starts_with("UPDATE File"));
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
