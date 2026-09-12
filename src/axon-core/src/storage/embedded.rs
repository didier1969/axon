use super::StorageEngine;
use anyhow::{Context, Result};
use parking_lot::Mutex;
use rusqlite::{types::ValueRef, Connection};
use std::path::PathBuf;
use std::sync::Arc;

use crate::graph_ingestion::rows::ChunkEmbeddingPersistRow;
use crate::postgres::bulk_writer::PgBulkBatch;
use crate::postgres::native::QueryTableOutput;

/// Calcule la similarité cosinus entre deux vecteurs de nombres flottants.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0;
    let mut norm_a = 0.0;
    let mut norm_b = 0.0;
    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }
    if norm_a <= 0.0 || norm_b <= 0.0 {
        0.0
    } else {
        dot / (norm_a.sqrt() * norm_b.sqrt())
    }
}

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
        engine.bootstrap_schemas(":memory:")?;
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
        engine.bootstrap_schemas(db_root)?;
        Ok(engine)
    }

    /// Initialise les fonctions scalaires vectorielles et les schémas virtuels attachés (`soll`, `ist`, `public`).
    fn bootstrap_schemas(&self, db_root: &str) -> Result<()> {
        let conn = self.conn.lock();

        // Enregistrement des fonctions vectorielles in-process
        conn.create_scalar_function(
            "cosine_similarity",
            2,
            rusqlite::functions::FunctionFlags::SQLITE_DETERMINISTIC,
            |ctx| {
                let a_str: String = ctx.get(0)?;
                let b_str: String = ctx.get(1)?;
                let a: Vec<f32> = serde_json::from_str(&a_str).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })?;
                let b: Vec<f32> = serde_json::from_str(&b_str).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        1,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })?;
                let sim = cosine_similarity(&a, &b);
                Ok(sim as f64)
            },
        )?;

        conn.create_scalar_function(
            "cosine_distance",
            2,
            rusqlite::functions::FunctionFlags::SQLITE_DETERMINISTIC,
            |ctx| {
                let a_str: String = ctx.get(0)?;
                let b_str: String = ctx.get(1)?;
                let a: Vec<f32> = serde_json::from_str(&a_str).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })?;
                let b: Vec<f32> = serde_json::from_str(&b_str).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        1,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })?;
                let sim = cosine_similarity(&a, &b);
                Ok((1.0 - sim) as f64)
            },
        )?;

        // Attachement des bases pour soll, ist et public
        if db_root == ":memory:" {
            let _ = conn.execute_batch(
                r#"
                ATTACH DATABASE ':memory:' AS soll;
                ATTACH DATABASE ':memory:' AS ist;
                ATTACH DATABASE ':memory:' AS public;
                "#,
            );
        } else {
            let root = PathBuf::from(db_root);
            let soll_path = root.join("soll.db");
            let ist_path = root.join("ist.db");
            let public_path = root.join("public.db");
            let soll_esc = soll_path.to_string_lossy().replace('\'', "''");
            let ist_esc = ist_path.to_string_lossy().replace('\'', "''");
            let public_esc = public_path.to_string_lossy().replace('\'', "''");

            let _ = conn.execute_batch(&format!(
                r#"
                ATTACH DATABASE '{soll_esc}' AS soll;
                ATTACH DATABASE '{ist_esc}' AS ist;
                ATTACH DATABASE '{public_esc}' AS public;
                "#
            ));
        }

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
                properties TEXT DEFAULT '{}',
                PRIMARY KEY (from_id, to_id, rel_type)
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

    fn flush_batch_copy(&self, batch: &PgBulkBatch) -> Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn
            .transaction()
            .context("Failed to begin SQLite transaction")?;

        // 1. Ingestion des indexed_files
        {
            let mut stmt = tx.prepare(
                r#"
                INSERT INTO ist.indexedfile (path, content_hash, mtime_ms, size_bytes, language)
                VALUES (?1, ?2, ?3, ?4, ?5)
                ON CONFLICT(path) DO UPDATE SET
                    content_hash = excluded.content_hash,
                    mtime_ms = excluded.mtime_ms,
                    size_bytes = excluded.size_bytes;
                "#,
            )?;
            for file in &batch.indexed_files {
                stmt.execute(rusqlite::params![&file.0, &file.1, file.2, file.3, "",])?;
            }
        }

        // 2. Ingestion des symbols
        {
            let mut stmt = tx.prepare(
                r#"
                INSERT INTO ist.symbol (id, name, kind, file_path, start_line, end_line, properties)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                ON CONFLICT(id) DO UPDATE SET
                    name = excluded.name,
                    kind = excluded.kind,
                    properties = excluded.properties;
                "#,
            )?;
            for sym in &batch.symbols {
                let props = serde_json::json!({
                    "project_code": sym.project_code,
                    "tested": sym.tested,
                    "is_public": sym.is_public,
                    "is_nif": sym.is_nif,
                    "is_unsafe": sym.is_unsafe,
                    "is_entry_point": sym.is_entry_point,
                    "cyclomatic_complexity": sym.cyclomatic_complexity,
                });
                stmt.execute(rusqlite::params![
                    &sym.symbol_id,
                    &sym.name,
                    &sym.kind,
                    "",
                    0,
                    0,
                    props.to_string(),
                ])?;
            }
        }

        // 3. Ingestion des relations (contains, calls, calls_nif, other_edges)
        {
            let mut stmt = tx.prepare(
                r#"
                INSERT INTO ist.edge (from_id, to_id, rel_type, properties)
                VALUES (?1, ?2, ?3, ?4)
                ON CONFLICT(from_id, to_id, rel_type) DO NOTHING;
                "#,
            )?;
            for rel in &batch.contains {
                stmt.execute(rusqlite::params![
                    &rel.source_id,
                    &rel.target_id,
                    "CONTAINS",
                    "{}"
                ])?;
            }
            for rel in &batch.calls {
                stmt.execute(rusqlite::params![
                    &rel.source_id,
                    &rel.target_id,
                    "CALLS",
                    "{}"
                ])?;
            }
            for rel in &batch.calls_nif {
                stmt.execute(rusqlite::params![
                    &rel.source_id,
                    &rel.target_id,
                    "CALLS_NIF",
                    "{}"
                ])?;
            }
            for (rel_type, rel) in &batch.other_edges {
                stmt.execute(rusqlite::params![
                    &rel.source_id,
                    &rel.target_id,
                    rel_type,
                    "{}"
                ])?;
            }
        }

        tx.commit().context("Failed to commit SQLite transaction")?;
        Ok(())
    }

    fn flush_chunk_embeddings_copy(
        &self,
        project_code: &str,
        model_id: &str,
        rows: &[ChunkEmbeddingPersistRow],
        embedded_at_ms: i64,
    ) -> Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn
            .transaction()
            .context("Failed to begin SQLite transaction")?;
        {
            let mut stmt = tx.prepare(
                r#"
                INSERT INTO ist.ChunkEmbedding (chunk_id, project_code, model_id, embedding_json, embedded_at_ms)
                VALUES (?1, ?2, ?3, ?4, ?5)
                ON CONFLICT(chunk_id) DO UPDATE SET
                    project_code = excluded.project_code,
                    model_id = excluded.model_id,
                    embedding_json = excluded.embedding_json,
                    embedded_at_ms = excluded.embedded_at_ms;
                "#,
            )?;
            for row in rows {
                let embedding_json =
                    serde_json::to_string(&row.embedding).unwrap_or_else(|_| "[]".to_string());
                stmt.execute(rusqlite::params![
                    &row.chunk_id,
                    project_code,
                    model_id,
                    embedding_json,
                    embedded_at_ms,
                ])?;
            }
        }
        tx.commit().context("Failed to commit SQLite transaction")?;
        Ok(())
    }
}
