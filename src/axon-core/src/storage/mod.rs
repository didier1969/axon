pub mod embedded;
pub mod postgres;

#[cfg(test)]
pub mod contract_tests;

pub use embedded::EmbeddedStorageEngine;
pub use postgres::PostgresStorageEngine;

use crate::graph_ingestion::rows::ChunkEmbeddingPersistRow;
use crate::postgres::bulk_writer::PgBulkBatch;
use crate::postgres::native::QueryTableOutput;
use anyhow::Result;

/// Mode de fonctionnement du moteur de stockage Axon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageEngineMode {
    Postgres,
    Embedded,
}

impl StorageEngineMode {
    pub fn from_env() -> Self {
        match std::env::var("AXON_STORAGE").ok().as_deref() {
            Some("embedded") | Some("sqlite") => StorageEngineMode::Embedded,
            _ => StorageEngineMode::Postgres,
        }
    }
}

/// Trait unifié d'accès au stockage pour Axon Core (Dual-Engine: PostgreSQL & Embedded).
pub trait StorageEngine: Send + Sync {
    /// Exécute une requête de lecture et renvoie les lignes encodées en JSON Vec<Vec<String>>.
    fn run_query_json(&self, sql: &str) -> String;

    /// Exécute une requête de comptage (COUNT) et renvoie le nombre scalaire.
    fn run_query_count(&self, sql: &str) -> i64;

    /// Exécute une requête et renvoie les colonnes et lignes sous forme de table typée.
    fn run_query_table(&self, sql: &str) -> Result<QueryTableOutput, String>;

    /// Exécute une instruction de mutation SQL.
    fn run_execute(&self, sql: &str) -> Result<(), String>;

    /// Exécute une requête de recherche vectorielle approchée (ANN / HNSW).
    fn run_ann_query_json(&self, sql: &str, ef_search: u32) -> String;

    /// Exécute un scan vectoriel exact délimité.
    fn run_exact_scan_query_json(&self, sql: &str) -> String;

    /// Écrit par lot un ensemble d'entités du graphe (Symboles, Fichiers, Arêtes).
    fn flush_batch_copy(&self, batch: &PgBulkBatch) -> Result<()>;

    /// Écrit par lot un ensemble d'embeddings de chunks.
    fn flush_chunk_embeddings_copy(
        &self,
        project_code: &str,
        model_id: &str,
        rows: &[ChunkEmbeddingPersistRow],
        embedded_at_ms: i64,
    ) -> Result<()>;
}
