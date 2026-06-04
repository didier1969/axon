//! Canonical write-path row carriers (REQ-AXO-901881 W1).
//!
//! These four structs are the in-memory shapes the A3 graph-ingestion path
//! builds and the `postgres::bulk_writer` flushes. They previously lived in
//! `graph_ingestion::async_writer` — a 659-line DuckDB-era async dispatch
//! module (REQ-AXO-193) whose dispatcher / accumulator / WriteDiff machinery
//! was never wired in production (it only ever flushed `RawQueries`, and the
//! typed variants were built solely in its own tests). The dead module is
//! retired; the rows it defined survive here, in their real home.

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
    /// Estimated BGE-Large token count from `code_chunker::estimated_token_count`.
    /// Stored so the B1 bucket-batching SELECT can `ORDER BY token_count`
    /// without recomputing — see DEC-AXO-086 follow-up. `None` for back-fill
    /// scenarios where the chunker was bypassed; SELECT falls back to a
    /// `length(content)/3` proxy via `COALESCE`.
    pub token_count: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RelationRow {
    pub source_id: String,
    pub target_id: String,
    pub project_code: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChunkEmbeddingPersistRow {
    pub chunk_id: String,
    pub source_hash: String,
    pub embedding: Vec<f32>,
}
