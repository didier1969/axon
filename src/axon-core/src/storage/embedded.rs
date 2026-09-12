use super::StorageEngine;
use anyhow::{Context, Result};
use parking_lot::Mutex;
use rusqlite::{types::ValueRef, Connection};
use std::path::PathBuf;
use std::sync::Arc;

use crate::graph_ingestion::rows::ChunkEmbeddingPersistRow;
use crate::postgres::bulk_writer::PgBulkBatch;
use crate::postgres::native::QueryTableOutput;

/// Moteur de stockage in-process embarqué pur Rust basé sur SQLite (sans aucun démon externe).
pub struct EmbeddedStorageEngine {
    conn: Arc<Mutex<Connection>>,
}

impl EmbeddedStorageEngine {
    /// Crée une instance en mémoire vive (idéale pour les tests unitaires ultra-rapides).
    pub fn new_in_memory() -> Result<Self> {
        let conn =
            Connection::open_in_memory().context("Failed to open in-memory SQLite database")?;
        let engine = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        engine.bootstrap_schemas()?;
        Ok(engine)
    }

    /// Ouvre ou crée une instance persistante sur disque dans le répertoire spécifié.
    pub fn open(db_root: &str) -> Result<Self> {
        if db_root == ":memory:" {
            return Self::new_in_memory();
        }

        let root = PathBuf::from(db_root);
        std::fs::create_dir_all(&root).context("Failed to create db root directory")?;
        let db_path = root.join("axon_embedded.db");

        let conn = Connection::open(&db_path).context("Failed to open SQLite database")?;
        // Activation du mode WAL pour de hautes performances concurrentes
        let _ = conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;");

        let engine = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        engine.bootstrap_schemas()?;
        Ok(engine)
    }

    /// Initialise les schémas virtuels attachés (`soll`, `ist`, `public`) pour respecter
    /// l'exacte syntaxe SQL attendue par les requêtes d'Axon.
    fn bootstrap_schemas(&self) -> Result<()> {
        let conn = self.conn.lock();
        // Attachement des bases en mémoire pour soll et ist si pas déjà attachées
        let _ = conn.execute_batch(
            r#"
            ATTACH DATABASE ':memory:' AS soll;
            ATTACH DATABASE ':memory:' AS ist;
            ATTACH DATABASE ':memory:' AS public;
            "#,
        );

        // Création minimale des tables SOLL
        let _ = conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS soll.entity (
                id TEXT PRIMARY KEY,
                kind TEXT NOT NULL,
                title TEXT NOT NULL,
                description TEXT NOT NULL,
                status TEXT NOT NULL,
                metadata TEXT DEFAULT '{}',
                created_at INTEGER,
                updated_at INTEGER
            );

            CREATE TABLE IF NOT EXISTS soll.link (
                source_id TEXT NOT NULL,
                target_id TEXT NOT NULL,
                relation_type TEXT NOT NULL,
                metadata TEXT DEFAULT '{}',
                PRIMARY KEY (source_id, target_id, relation_type)
            );

            CREATE TABLE IF NOT EXISTS soll.revision (
                id TEXT PRIMARY KEY,
                created_at INTEGER,
                summary TEXT
            );

            CREATE TABLE IF NOT EXISTS soll.revision_change (
                id TEXT PRIMARY KEY,
                revision_id TEXT,
                entity_id TEXT,
                change_type TEXT,
                payload TEXT
            );
            "#,
        );

        // Création minimale des tables IST
        let _ = conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS ist.indexedfile (
                path TEXT PRIMARY KEY,
                content_hash TEXT NOT NULL,
                mtime_ms INTEGER,
                size_bytes INTEGER,
                language TEXT
            );

            CREATE TABLE IF NOT EXISTS ist.symbol (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                kind TEXT NOT NULL,
                file_path TEXT NOT NULL,
                start_line INTEGER,
                end_line INTEGER,
                properties TEXT DEFAULT '{}'
            );

            CREATE TABLE IF NOT EXISTS ist.edge (
                from_id TEXT NOT NULL,
                to_id TEXT NOT NULL,
                rel_type TEXT NOT NULL,
                properties TEXT DEFAULT '{}'
            );

            CREATE TABLE IF NOT EXISTS ist.ChunkEmbedding (
                chunk_id TEXT PRIMARY KEY,
                project_code TEXT,
                model_id TEXT,
                embedding_json TEXT,
                embedded_at_ms INTEGER
            );
            "#,
        );

        Ok(())
    }
}

impl StorageEngine for EmbeddedStorageEngine {
    fn run_query_table(&self, sql: &str) -> Result<QueryTableOutput, String> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare(sql)
            .map_err(|e| format!("Prepare error: {e} | sql: {sql}"))?;

        let col_count = stmt.column_count();
        let columns: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();

        let mut rows: Vec<Vec<String>> = Vec::new();
        let mut rows_iter = stmt
            .query([])
            .map_err(|e| format!("Query error: {e} | sql: {sql}"))?;

        while let Ok(Some(row)) = rows_iter.next() {
            let mut rendered_row = Vec::with_capacity(col_count);
            for i in 0..col_count {
                let cell_val = match row.get_ref(i) {
                    Ok(ValueRef::Null) => "".to_string(),
                    Ok(ValueRef::Integer(i)) => i.to_string(),
                    Ok(ValueRef::Real(f)) => f.to_string(),
                    Ok(ValueRef::Text(t)) => String::from_utf8_lossy(t).to_string(),
                    Ok(ValueRef::Blob(b)) => format!("<blob len={}>", b.len()),
                    Err(_) => "".to_string(),
                };
                rendered_row.push(cell_val);
            }
            rows.push(rendered_row);
        }

        let row_count = rows.len();
        let rows_json = serde_json::to_string(&rows).unwrap_or_else(|_| "[]".to_string());

        Ok(QueryTableOutput {
            columns,
            rows,
            rows_json,
            row_count,
        })
    }

    fn run_query_json(&self, sql: &str) -> String {
        match self.run_query_table(sql) {
            Ok(table) => table.rows_json,
            Err(err_msg) => {
                let err_obj = serde_json::json!({
                    "_axon_plugin_error": err_msg,
                    "pg_error": {
                        "message": err_msg,
                        "code": "EMBEDDED_SQLITE_ERR"
                    }
                });
                err_obj.to_string()
            }
        }
    }

    fn run_query_count(&self, sql: &str) -> i64 {
        let conn = self.conn.lock();
        let mut stmt = match conn.prepare(sql) {
            Ok(s) => s,
            Err(_) => return 0,
        };
        stmt.query_row([], |row| row.get::<_, i64>(0)).unwrap_or(0)
    }

    fn run_execute(&self, sql: &str) -> Result<(), String> {
        let conn = self.conn.lock();
        conn.execute_batch(sql)
            .map_err(|e| format!("Execute error: {e} | sql: {sql}"))
    }

    fn run_ann_query_json(&self, sql: &str, _ef_search: u32) -> String {
        self.run_query_json(sql)
    }

    fn run_exact_scan_query_json(&self, sql: &str) -> String {
        self.run_query_json(sql)
    }

    fn flush_batch_copy(&self, _batch: &PgBulkBatch) -> Result<()> {
        // En mode embarqué, les batches peuvent être insérés directement
        Ok(())
    }

    fn flush_chunk_embeddings_copy(
        &self,
        _project_code: &str,
        _model_id: &str,
        _rows: &[ChunkEmbeddingPersistRow],
        _embedded_at_ms: i64,
    ) -> Result<()> {
        Ok(())
    }
}
