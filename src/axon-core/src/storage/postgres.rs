use super::StorageEngine;
use crate::graph_ingestion::rows::ChunkEmbeddingPersistRow;
use crate::postgres::bulk_writer::PgBulkBatch;
use crate::postgres::native::{NativePgCtx, QueryTableOutput};
use anyhow::Result;
use std::sync::Arc;

/// Implémentation PostgreSQL du moteur de stockage (production nominale).
pub struct PostgresStorageEngine {
    pub ctx: Arc<NativePgCtx>,
}

impl PostgresStorageEngine {
    pub fn new(ctx: NativePgCtx) -> Self {
        Self { ctx: Arc::new(ctx) }
    }

    pub fn from_arc(ctx: Arc<NativePgCtx>) -> Self {
        Self { ctx }
    }
}

impl StorageEngine for PostgresStorageEngine {
    fn run_query_json(&self, sql: &str) -> String {
        self.ctx.run_query_json(sql)
    }

    fn run_query_count(&self, sql: &str) -> i64 {
        self.ctx.run_query_count(sql)
    }

    fn run_query_table(&self, sql: &str) -> Result<QueryTableOutput, String> {
        self.ctx.run_query_table(sql)
    }

    fn run_execute(&self, sql: &str) -> Result<(), String> {
        self.ctx.run_execute(sql)
    }

    fn run_ann_query_json(&self, sql: &str, ef_search: u32) -> String {
        self.ctx.run_ann_query_json(sql, ef_search)
    }

    fn run_exact_scan_query_json(&self, sql: &str) -> String {
        self.ctx.run_exact_scan_query_json(sql)
    }

    fn flush_batch_copy(&self, batch: &PgBulkBatch) -> Result<()> {
        self.ctx.flush_batch_copy(batch)
    }

    fn flush_chunk_embeddings_copy(
        &self,
        project_code: &str,
        model_id: &str,
        rows: &[ChunkEmbeddingPersistRow],
        embedded_at_ms: i64,
    ) -> Result<()> {
        self.ctx
            .flush_chunk_embeddings_copy(project_code, model_id, rows, embedded_at_ms)
    }
}
