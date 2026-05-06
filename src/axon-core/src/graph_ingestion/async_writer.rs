// REQ-AXO-193 direction E (E.1): typed write-diff data structures used to
// move all hot-path DuckDB mutations through a single async writer thread.
// E.1 ships the contracts only — `render_bulk_queries` returns an empty
// Vec until E.3 ports the existing INSERT/UPDATE templates from
// graph_ingestion::insert_file_data_batch_with_vectorization_policy.

use crate::graph_ingestion::FileVectorizationWork;

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
}
