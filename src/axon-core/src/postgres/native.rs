//! Native in-process PostgreSQL access (REQ-AXO-901881 W2 / REQ-AXO-901880).
//!
//! This module is the in-process replacement for the `axon-plugin-postgres`
//! C-ABI cdylib (a DuckDB-era swap-seam: the plugin was itself just a
//! `deadpool_postgres::Pool` + tokio runtime + render/session/error helpers
//! marshalled through `*mut c_void`). The logic here is copied VERBATIM from
//! that plugin (render_pg_value, build_session_setup_sql, the REQ-AXO-129
//! error envelope, REQ-AXO-91494 statement_timeout) so the JSON the ~411 read
//! call-sites parse is byte-identical — parity is structural, not a rewrite.
//!
//! Contract mirror: `run_query_json` returns the rendered `Vec<Vec<String>>`
//! JSON on success OR the `{"_axon_plugin_error":...}` envelope string on
//! error, exactly as the plugin's `pg_query_json` did, so
//! `graph_query::query_on_ctx`'s leading-`{` envelope detection is unchanged.

use std::sync::OnceLock;

use deadpool_postgres::{Pool, Runtime as DpRuntime};
use tokio::runtime::{Builder as RtBuilder, Runtime};
use tokio_postgres::NoTls;

/// Process-global tokio runtime that drives all native PG block_on calls.
///
/// A single shared runtime (vs one per `NativePgCtx`) avoids the
/// drop-a-runtime-inside-a-runtime panic when a per-test `GraphStore` is
/// dropped on a `#[tokio::test]` thread, and shrinks the runtime sprawl the
/// audit flagged (proof #21). Calls always originate from sync `fn main` or
/// `spawn_blocking` threads, so `block_on` here never nests inside a worker.
fn native_runtime() -> &'static Runtime {
    static RT: OnceLock<Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        RtBuilder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .thread_name("axon-pg-native")
            .build()
            .expect("axon native PG runtime build failed")
    })
}

/// Per-store native PG context: a deadpool connection pool + the optional
/// per-connection `search_path` schema. Replaces the FFI `PgPluginContext`
/// (the runtime is now the process-global `native_runtime`).
pub struct NativePgCtx {
    pub pool: Pool,
    pub schema_search_path: Option<String>,
}

impl NativePgCtx {
    /// Build a native context against `database_url`, optionally pinning a
    /// validated `search_path` schema. Mirrors the plugin's `pg_init_db`:
    /// create the deadpool pool and probe one connection so a misconfigured
    /// URL fails fast at boot.
    pub fn connect(database_url: &str, schema: Option<&str>) -> anyhow::Result<Self> {
        let schema_search_path = match schema {
            None => None,
            Some(s) if s.is_empty() => None,
            Some(s) => Some(validate_schema_identifier(s).map_err(|reason| {
                anyhow::anyhow!("rejected schema {s:?}: {reason}")
            })?),
        };

        let pool = native_runtime()
            .block_on(async {
                let mut cfg = deadpool_postgres::Config::new();
                cfg.url = Some(database_url.to_string());
                cfg.manager = Some(deadpool_postgres::ManagerConfig {
                    recycling_method: deadpool_postgres::RecyclingMethod::Fast,
                });
                cfg.create_pool(Some(DpRuntime::Tokio1), NoTls)
            })
            .map_err(|e| anyhow::anyhow!("pool creation failed: {e}"))?;

        // Probe one connection so misconfigured URLs fail fast at boot.
        let _probe = native_runtime()
            .block_on(async { pool.get().await })
            .map_err(|e| anyhow::anyhow!("probe connection failed: {e}"))?;
        drop(_probe);

        Ok(NativePgCtx {
            pool,
            schema_search_path,
        })
    }

    /// Run a query and render the result as the canonical `Vec<Vec<String>>`
    /// JSON. On error returns the REQ-AXO-129 envelope string (leading `{`)
    /// so `query_on_ctx` converts it to `Err`. Non-row statements return `[]`.
    pub fn run_query_json(&self, sql: &str) -> String {
        let returns_rows = query_returns_rows(sql);
        native_runtime().block_on(async {
            let conn = match self.pool.get().await {
                Ok(c) => c,
                Err(e) => return error_envelope("acquire", sql, &e.to_string()),
            };
            if let Err(e) = apply_session_setup(&conn, &self.schema_search_path).await {
                return db_error_envelope("set_search_path", sql, &e);
            }
            if !returns_rows {
                return match conn.batch_execute(sql).await {
                    Ok(_) => "[]".to_string(),
                    Err(e) => db_error_envelope("execute", sql, &e),
                };
            }
            match conn.query(sql, &[]).await {
                Ok(rows) => {
                    let mut out: Vec<Vec<String>> = Vec::with_capacity(rows.len());
                    for row in &rows {
                        let mut rendered = Vec::with_capacity(row.len());
                        for col in 0..row.len() {
                            rendered.push(render_pg_value(row, col));
                        }
                        out.push(rendered);
                    }
                    serde_json::to_string(&out).unwrap_or_else(|_| "[]".to_string())
                }
                Err(e) => db_error_envelope("query", sql, &e),
            }
        })
    }

    /// REQ-AXO-901883 — ANN (HNSW) read path for the `retrieve_context` /
    /// `retrieve_context_v2` semantic lane.
    ///
    /// pgvector only chooses `chunk_embedding_hnsw_idx` for a clean
    /// `ORDER BY embedding <=> $q LIMIT k`. On a small `ist.ChunkEmbedding`
    /// (few thousand rows) the planner's cost model still prefers a
    /// `Seq Scan + Sort` because the HNSW first-tuple cost is high relative
    /// to a full scan of a small table. The semantic lane needs the index
    /// regardless of table size (recall@k stability + O(log n) scaling), so
    /// this runs the ANN SELECT inside an explicit transaction with
    /// `SET LOCAL enable_seqscan = off` + `SET LOCAL hnsw.ef_search`. The
    /// GUCs are transaction-scoped (`LOCAL`), so they never leak to the
    /// pooled connection's other consumers, and the search-path / timeout
    /// session setup is applied exactly as `run_query_json` does so the
    /// returned JSON / error-envelope contract is byte-identical.
    ///
    /// `ef_search` is clamped to `[10, 1000]` (pgvector's accepted range).
    pub fn run_ann_query_json(&self, sql: &str, ef_search: u32) -> String {
        let ef = ef_search.clamp(10, 1000);
        native_runtime().block_on(async {
            let mut conn = match self.pool.get().await {
                Ok(c) => c,
                Err(e) => return error_envelope("acquire", sql, &e.to_string()),
            };
            if let Err(e) = apply_session_setup(&conn, &self.schema_search_path).await {
                return db_error_envelope("set_search_path", sql, &e);
            }
            let tx = match conn.transaction().await {
                Ok(tx) => tx,
                Err(e) => return db_error_envelope("ann_begin", sql, &e),
            };
            // SET LOCAL only takes effect inside a transaction; it reverts at
            // COMMIT/ROLLBACK. ef_search is a literal in [10,1000] (no inject).
            if let Err(e) = tx
                .batch_execute(&format!(
                    "SET LOCAL enable_seqscan = off; SET LOCAL hnsw.ef_search = {ef}"
                ))
                .await
            {
                return db_error_envelope("ann_set_local", sql, &e);
            }
            let rendered = match tx.query(sql, &[]).await {
                Ok(rows) => {
                    let mut out: Vec<Vec<String>> = Vec::with_capacity(rows.len());
                    for row in &rows {
                        let mut rendered = Vec::with_capacity(row.len());
                        for col in 0..row.len() {
                            rendered.push(render_pg_value(row, col));
                        }
                        out.push(rendered);
                    }
                    serde_json::to_string(&out).unwrap_or_else(|_| "[]".to_string())
                }
                Err(e) => return db_error_envelope("ann_query", sql, &e),
            };
            if let Err(e) = tx.commit().await {
                return db_error_envelope("ann_commit", sql, &e);
            }
            rendered
        })
    }

    /// `SELECT count(*)`-style scalar count. Mirrors the plugin's
    /// `pg_query_count`: returns the first column of the first row as i64,
    /// or 0 on any error / empty result.
    pub fn run_query_count(&self, sql: &str) -> i64 {
        native_runtime().block_on(async {
            let conn = match self.pool.get().await {
                Ok(c) => c,
                Err(_) => return 0,
            };
            if apply_session_setup(&conn, &self.schema_search_path)
                .await
                .is_err()
            {
                return 0;
            }
            match conn.query_opt(sql, &[]).await {
                Ok(Some(row)) => row.try_get::<_, i64>(0).unwrap_or(0),
                _ => 0,
            }
        })
    }

    /// REQ-AXO-901881 W3 #33/#34 — bulk COPY BINARY chunk-embedding flush on
    /// THIS store's pool (per-instance / per-test DB). The live B3 writer
    /// (`upsert_chunk_embedding_v2_batch`) routes large batches here so the
    /// embedding upsert lands in the same database the GraphStore reads from —
    /// NOT bulk_writer's global env-resolved pool (also closes the embedding
    /// half of the linchpin REQ-AXO-901877). The COPY path is fully
    /// schema-qualified (ist.ChunkEmbedding + schema-qualified vector +
    /// session-local temp), so no `search_path` setup is required.
    pub fn flush_chunk_embeddings_copy(
        &self,
        project_code: &str,
        model_id: &str,
        rows: &[crate::graph_ingestion::rows::ChunkEmbeddingPersistRow],
        embedded_at_ms: i64,
    ) -> anyhow::Result<()> {
        if rows.is_empty() {
            return Ok(());
        }
        native_runtime().block_on(async {
            let mut client = self.pool.get().await.map_err(|e| {
                anyhow::anyhow!("flush_chunk_embeddings_copy: native pool acquire failed: {e}")
            })?;
            crate::postgres::bulk_writer::flush_chunk_embeddings_async(
                &mut client,
                project_code,
                model_id,
                rows,
                embedded_at_ms,
            )
            .await
        })
    }

    /// Execute a (possibly multi-statement) SQL string. Mirrors the plugin's
    /// `pg_execute`: returns `true` on success, `false` on any error.
    pub fn run_execute(&self, sql: &str) -> bool {
        native_runtime().block_on(async {
            let conn = match self.pool.get().await {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!("native pg execute: pool acquire failed: {e}");
                    return false;
                }
            };
            if let Err(e) = apply_session_setup(&conn, &self.schema_search_path).await {
                tracing::warn!("native pg execute: set search_path failed: {e}");
                return false;
            }
            match conn.batch_execute(sql).await {
                Ok(_) => true,
                Err(e) => {
                    tracing::warn!("native pg execute: {e} | {sql}");
                    false
                }
            }
        })
    }
}

fn query_returns_rows(sql: &str) -> bool {
    let leading = sql.trim_start().to_lowercase();
    leading.starts_with("select")
        || leading.starts_with("with")
        || leading.starts_with("show")
        || leading.starts_with("table ")
        || leading.starts_with("values")
        || sql.to_lowercase().contains(" returning ")
}

/// REQ-AXO-91494 — per-statement timeout (ms) from
/// `AXON_PG_STATEMENT_TIMEOUT_MS`, default 30000; 0 disables.
fn statement_timeout_ms() -> u64 {
    std::env::var("AXON_PG_STATEMENT_TIMEOUT_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(30_000)
}

/// Project schema first (so unqualified names resolve to `<schema>.X`) then
/// `public`; statement_timeout always runs (REQ-AXO-91494).
fn build_session_setup_sql(schema: Option<&str>) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if let Some(s) = schema {
        parts.push(format!("SET search_path TO {s}, public"));
    }
    let timeout_ms = statement_timeout_ms();
    if timeout_ms > 0 {
        parts.push(format!("SET statement_timeout TO {timeout_ms}"));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("; "))
    }
}

async fn apply_session_setup(
    conn: &deadpool_postgres::Client,
    schema: &Option<String>,
) -> Result<(), tokio_postgres::Error> {
    if let Some(sql) = build_session_setup_sql(schema.as_deref()) {
        conn.batch_execute(&sql).await?;
    }
    Ok(())
}

/// `[a-zA-Z0-9_]{1,64}` schema validation (injection guard).
fn validate_schema_identifier(s: &str) -> Result<String, &'static str> {
    if s.is_empty() {
        return Err("schema is empty");
    }
    if s.len() > 64 {
        return Err("schema is longer than 64 chars");
    }
    if !s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err("schema contains characters outside [a-zA-Z0-9_]");
    }
    Ok(s.to_string())
}

/// REQ-AXO-129 — generic error envelope (leading `{`) for non-DbError stages.
fn error_envelope(stage: &str, sql: &str, err: &str) -> String {
    serde_json::json!({
        "_axon_plugin_error": format!("{stage}: {err}"),
        "stage": stage,
        "sql_excerpt": sql.chars().take(240).collect::<String>(),
    })
    .to_string()
}

/// REQ-AXO-129 — rich envelope drilling into the `DbError` (SQLSTATE / message
/// / hint / position) so callers see the real failure, not `db error`.
fn db_error_envelope(stage: &str, sql: &str, err: &tokio_postgres::Error) -> String {
    let display = err.to_string();
    let mut detail = serde_json::json!({});
    if let Some(db) = err.as_db_error() {
        detail = serde_json::json!({
            "code": db.code().code(),
            "severity": db.severity(),
            "message": db.message(),
            "detail": db.detail(),
            "hint": db.hint(),
            "position": db.position().map(|p| format!("{p:?}")),
        });
    }
    serde_json::json!({
        "_axon_plugin_error": format!("{stage}: {display}"),
        "stage": stage,
        "sql_excerpt": sql.chars().take(240).collect::<String>(),
        "pg_error": detail,
    })
    .to_string()
}

/// Per-column typed→String rendering. Copied verbatim from the retired plugin
/// so the JSON matrix the ~411 readers parse is byte-identical.
fn render_pg_value(row: &tokio_postgres::Row, col: usize) -> String {
    use tokio_postgres::types::Type;
    let ty = row.columns()[col].type_().clone();
    if let Ok(opt) = row.try_get::<_, Option<&str>>(col) {
        match opt {
            Some(s) => return s.to_string(),
            None => return "null".to_string(),
        }
    }
    if ty == Type::INT2 {
        if let Ok(v) = row.try_get::<_, Option<i16>>(col) {
            return v.map(|n| n.to_string()).unwrap_or_else(|| "null".into());
        }
    }
    if ty == Type::INT4 {
        if let Ok(v) = row.try_get::<_, Option<i32>>(col) {
            return v.map(|n| n.to_string()).unwrap_or_else(|| "null".into());
        }
    }
    if ty == Type::INT8 {
        if let Ok(v) = row.try_get::<_, Option<i64>>(col) {
            return v.map(|n| n.to_string()).unwrap_or_else(|| "null".into());
        }
    }
    if ty == Type::FLOAT4 {
        if let Ok(v) = row.try_get::<_, Option<f32>>(col) {
            return v.map(|n| n.to_string()).unwrap_or_else(|| "null".into());
        }
    }
    if ty == Type::FLOAT8 {
        if let Ok(v) = row.try_get::<_, Option<f64>>(col) {
            return v.map(|n| n.to_string()).unwrap_or_else(|| "null".into());
        }
    }
    if ty == Type::BOOL {
        if let Ok(v) = row.try_get::<_, Option<bool>>(col) {
            return v.map(|b| b.to_string()).unwrap_or_else(|| "null".into());
        }
    }
    if ty == Type::JSON || ty == Type::JSONB {
        if let Ok(v) = row.try_get::<_, Option<serde_json::Value>>(col) {
            return v.map(|j| j.to_string()).unwrap_or_else(|| "null".into());
        }
    }
    format!("<unsupported type {}>", ty.name())
}
