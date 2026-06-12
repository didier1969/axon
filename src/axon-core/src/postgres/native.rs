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
/// audit flagged (proof #21). Callers reach it ONLY through `run_blocking`
/// (REQ-AXO-901884), which spawns onto this runtime + waits on a channel when
/// the caller is already inside a tokio runtime — so we never nest `block_on`
/// (the boot path runs inside `run_indexer`/`run_brain`'s runtime).
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

/// Drive a native PG future to completion from ANY calling context
/// (REQ-AXO-901884 — async-native PG migration, stage 0: nesting-safe shim).
///
/// `native_runtime().block_on(fut)` PANICS ("Cannot start a runtime from
/// within a runtime") when the current thread is already a tokio runtime
/// worker — which is exactly the case on the indexer/brain boot path
/// (`run_indexer`/`run_brain` run `boot()` via `block_on`, and `boot()`
/// constructs the GraphStore), as well as any MCP / pipeline async task that
/// reaches a sync GraphStore method directly. To stay correct from sync
/// `fn main`, from `spawn_blocking`, AND from inside an async runtime, we
/// spawn the future onto the process-global native runtime (its own worker
/// threads) and wait on a std channel — we NEVER call `block_on` on the
/// current thread. The temporary throughput cost (a runtime hop + channel)
/// disappears once the call-sites become fully async (later stages).
fn run_blocking<F>(fut: F) -> F::Output
where
    F: std::future::Future + Send + 'static,
    F::Output: Send + 'static,
{
    if tokio::runtime::Handle::try_current().is_ok() {
        // Already inside a tokio runtime → block_on would nest + panic.
        // Run on the native runtime's workers, block the current thread on a
        // std channel (allowed: not a runtime `block_on`).
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        native_runtime().spawn(async move {
            let _ = tx.send(fut.await);
        });
        rx.recv()
            .expect("axon native PG runtime task dropped before returning a result")
    } else {
        native_runtime().block_on(fut)
    }
}

/// Per-store native PG context: a deadpool connection pool + the optional
/// per-connection `search_path` schema. Replaces the FFI `PgPluginContext`
/// (the runtime is now the process-global `native_runtime`).
pub struct NativePgCtx {
    pub pool: Pool,
    pub schema_search_path: Option<String>,
}

impl Drop for NativePgCtx {
    fn drop(&mut self) {
        // REQ-AXO-901906 — release this store's pooled connections at drop
        // instead of letting them linger on the process-global native runtime
        // until process exit. Without this, every per-test GraphStore leaks its
        // idle connection(s) for the whole `cargo test` process, accumulating
        // until PG's max_connections is hit and later tests fail to connect.
        // `Pool::close` is idempotent + sync; production stores are long-lived
        // so this only fires on their (rare) teardown.
        self.pool.close();
    }
}

impl NativePgCtx {
    /// Build a native context against `database_url`, optionally pinning a
    /// validated `search_path` schema. Mirrors the plugin's `pg_init_db`:
    /// create the deadpool pool and probe one connection so a misconfigured
    /// URL fails fast at boot.
    pub fn connect(database_url: &str, schema: Option<&str>) -> anyhow::Result<Self> {
        // REQ-AXO-901890 — finish the public→ist migration at the connection
        // layer: default EVERY connection to the `ist` schema so unqualified
        // table names (the ~167 INSERT/FROM sites that rely on search_path)
        // always resolve to `ist.*`, never the dropped `public.*` leftovers.
        // `build_session_setup_sql` emits `SET search_path TO ist, public`, so
        // `public` stays on the path for extension types (pgvector `vector`).
        // Previously `None`/empty meant NO per-session SET, relying on the
        // ALTER DATABASE default — fragile if that drifts or a pooled conn
        // predates it (root of public.symbol/chunk/indexedfile getting written
        // in parallel with ist.* during a rebuild).
        let schema_search_path = match schema {
            None => Some("ist".to_string()),
            Some(s) if s.is_empty() => Some("ist".to_string()),
            Some(s) => Some(
                validate_schema_identifier(s)
                    .map_err(|reason| anyhow::anyhow!("rejected schema {s:?}: {reason}"))?,
            ),
        };

        let url = database_url.to_string();
        let pool = run_blocking(async move {
            let mut cfg = deadpool_postgres::Config::new();
            cfg.url = Some(url);
            // REQ-AXO-901884 stage 0.1 — Verified (not Fast) recycling: deadpool
            // runs a check query before handing back a pooled conn, so a
            // connection left in an aborted transaction (SQLSTATE 25P02, e.g. a
            // failed unqualified-table write) is DISCARDED + recreated instead of
            // poisoning every later borrower. Fast skipped the check, so one
            // aborted tx cascaded into a pool-wide 25P02 stall mid-indexation.
            cfg.manager = Some(deadpool_postgres::ManagerConfig {
                recycling_method: deadpool_postgres::RecyclingMethod::Verified,
            });
            cfg.create_pool(Some(DpRuntime::Tokio1), NoTls)
        })
        .map_err(|e| anyhow::anyhow!("pool creation failed: {e}"))?;

        // Probe one connection so misconfigured URLs fail fast at boot.
        let pool_probe = pool.clone();
        run_blocking(async move { pool_probe.get().await.map(|_| ()) })
            .map_err(|e| anyhow::anyhow!("probe connection failed: {e}"))?;

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
        let pool = self.pool.clone();
        let schema = self.schema_search_path.clone();
        let sql = sql.to_string();
        run_blocking(async move {
            let conn = match pool.get().await {
                Ok(c) => c,
                Err(e) => return error_envelope("acquire", &sql, &e.to_string()),
            };
            if let Err(e) = apply_session_setup(&conn, &schema).await {
                return db_error_envelope("set_search_path", &sql, &e);
            }
            if !returns_rows {
                return match conn.batch_execute(&sql).await {
                    Ok(_) => "[]".to_string(),
                    Err(e) => db_error_envelope("execute", &sql, &e),
                };
            }
            match conn.query(&sql, &[]).await {
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
                Err(e) => db_error_envelope("query", &sql, &e),
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
        let pool = self.pool.clone();
        let schema = self.schema_search_path.clone();
        let sql = sql.to_string();
        run_blocking(async move {
            let mut conn = match pool.get().await {
                Ok(c) => c,
                Err(e) => return error_envelope("acquire", &sql, &e.to_string()),
            };
            if let Err(e) = apply_session_setup(&conn, &schema).await {
                return db_error_envelope("set_search_path", &sql, &e);
            }
            let tx = match conn.transaction().await {
                Ok(tx) => tx,
                Err(e) => return db_error_envelope("ann_begin", &sql, &e),
            };
            // SET LOCAL only takes effect inside a transaction; it reverts at
            // COMMIT/ROLLBACK. ef_search is a literal in [10,1000] (no inject).
            if let Err(e) = tx
                .batch_execute(&format!(
                    "SET LOCAL enable_seqscan = off; SET LOCAL hnsw.ef_search = {ef}"
                ))
                .await
            {
                return db_error_envelope("ann_set_local", &sql, &e);
            }
            let rendered = match tx.query(&sql, &[]).await {
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
                Err(e) => return db_error_envelope("ann_query", &sql, &e),
            };
            if let Err(e) = tx.commit().await {
                return db_error_envelope("ann_commit", &sql, &e);
            }
            rendered
        })
    }

    /// `SELECT count(*)`-style scalar count. Mirrors the plugin's
    /// `pg_query_count`: returns the first column of the first row as i64,
    /// or 0 on any error / empty result.
    pub fn run_query_count(&self, sql: &str) -> i64 {
        let pool = self.pool.clone();
        let schema = self.schema_search_path.clone();
        let sql = sql.to_string();
        run_blocking(async move {
            let conn = match pool.get().await {
                Ok(c) => c,
                Err(_) => return 0,
            };
            if apply_session_setup(&conn, &schema).await.is_err() {
                return 0;
            }
            match conn.query_opt(&sql, &[]).await {
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
        let pool = self.pool.clone();
        let project_code = project_code.to_string();
        let model_id = model_id.to_string();
        let rows = rows.to_vec();
        run_blocking(async move {
            let mut client = pool.get().await.map_err(|e| {
                anyhow::anyhow!("flush_chunk_embeddings_copy: native pool acquire failed: {e}")
            })?;
            crate::postgres::bulk_writer::flush_chunk_embeddings_async(
                &mut client,
                &project_code,
                &model_id,
                &rows,
                embedded_at_ms,
            )
            .await
        })
    }

    /// REQ-AXO-901959 — flush a `PgBulkBatch` (graph half: Symbol/Chunk/
    /// IndexedFile/CALLS/CONTAINS) through THIS store's native pool, so the rows
    /// land in the same database the GraphStore reads from — NOT bulk_writer's
    /// global env-resolved pool. Closes the graph half of the linchpin
    /// (REQ-AXO-901877); the embedding half is `flush_chunk_embeddings_copy`
    /// above. The COPY/INSERT path is schema-qualified, so no `search_path`
    /// setup is required. The bulk_writer async core is pool-agnostic (takes a
    /// `&mut Client`), so this is pure routing — the write logic is unchanged.
    pub fn flush_batch_copy(
        &self,
        batch: &crate::postgres::bulk_writer::PgBulkBatch,
    ) -> anyhow::Result<()> {
        if batch.is_empty() {
            return Ok(());
        }
        let pool = self.pool.clone();
        let batch = batch.clone();
        run_blocking(async move {
            let mut client = pool.get().await.map_err(|e| {
                anyhow::anyhow!("flush_batch_copy: native pool acquire failed: {e}")
            })?;
            crate::postgres::bulk_writer::flush_batch_async(&mut client, &batch).await
        })
    }

    /// Execute a (possibly multi-statement) SQL string. Mirrors the plugin's
    /// `pg_execute`: returns `true` on success, `false` on any error.
    pub fn run_execute(&self, sql: &str) -> bool {
        let pool = self.pool.clone();
        let schema = self.schema_search_path.clone();
        let sql = sql.to_string();
        run_blocking(async move {
            let conn = match pool.get().await {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!("native pg execute: pool acquire failed: {e}");
                    return false;
                }
            };
            if let Err(e) = apply_session_setup(&conn, &schema).await {
                tracing::warn!("native pg execute: set search_path failed: {e}");
                return false;
            }
            match conn.batch_execute(&sql).await {
                Ok(_) => true,
                Err(e) => {
                    tracing::warn!("native pg execute: {e} | {sql}");
                    false
                }
            }
        })
    }

    // ===================================================================
    // REQ-AXO-901884 Stage 0 — async-native typed core (ADDITIVE).
    //
    // These expose the already-async internals DIRECTLY: no `run_blocking`
    // runtime-hop, no `render_pg_value` → Vec<Vec<String>> stringification.
    // Callers in async contexts `.await` real `tokio_postgres::Row`s and a
    // structured `PgError` (SQLSTATE/message/hint). The sync `run_*` +
    // `render_pg_value` facade above stays byte-identical until every
    // consumer is migrated off it (stages 1-6), then it is deleted.
    // ===================================================================

    /// Acquire a pooled connection + apply the per-connection session setup
    /// (search_path + statement_timeout). Shared by the async core methods.
    async fn acquire_and_setup(
        &self,
        stage: &'static str,
        sql: &str,
    ) -> Result<deadpool_postgres::Client, PgError> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| PgError::acquire(stage, sql, e.to_string()))?;
        apply_session_setup(&conn, &self.schema_search_path)
            .await
            .map_err(|e| PgError::from_tokio("set_search_path", sql, &e))?;
        Ok(conn)
    }

    /// Async row query. Returns typed `tokio_postgres::Row`s (callers use
    /// `try_get` / `FromRow`) or a structured `PgError` carrying SQLSTATE.
    pub async fn query(&self, sql: &str) -> Result<Vec<tokio_postgres::Row>, PgError> {
        let conn = self.acquire_and_setup("query", sql).await?;
        conn.query(sql, &[])
            .await
            .map_err(|e| PgError::from_tokio("query", sql, &e))
    }

    /// Async multi-statement execute (BEGIN/…/COMMIT batches, DDL, writes).
    pub async fn execute_batch_async(&self, sql: &str) -> Result<(), PgError> {
        let conn = self.acquire_and_setup("execute", sql).await?;
        conn.batch_execute(sql)
            .await
            .map_err(|e| PgError::from_tokio("execute", sql, &e))
    }

    /// Async ANN (HNSW) read — mirrors `run_ann_query_json` (SET LOCAL
    /// enable_seqscan=off + hnsw.ef_search inside a tx so pgvector picks
    /// `chunk_embedding_hnsw_idx` regardless of table size) but returns typed
    /// rows. `ef_search` clamped to pgvector's accepted `[10, 1000]`.
    pub async fn query_ann(
        &self,
        sql: &str,
        ef_search: u32,
    ) -> Result<Vec<tokio_postgres::Row>, PgError> {
        let ef = ef_search.clamp(10, 1000);
        let mut conn = self.acquire_and_setup("ann_query", sql).await?;
        let tx = conn
            .transaction()
            .await
            .map_err(|e| PgError::from_tokio("ann_begin", sql, &e))?;
        tx.batch_execute(&format!(
            "SET LOCAL enable_seqscan = off; SET LOCAL hnsw.ef_search = {ef}"
        ))
        .await
        .map_err(|e| PgError::from_tokio("ann_set_local", sql, &e))?;
        let rows = tx
            .query(sql, &[])
            .await
            .map_err(|e| PgError::from_tokio("ann_query", sql, &e))?;
        tx.commit()
            .await
            .map_err(|e| PgError::from_tokio("ann_commit", sql, &e))?;
        Ok(rows)
    }
}

/// REQ-AXO-901884 Stage 0 — structured PG error for the async-native core.
/// Carries the same SQLSTATE / severity / message / detail / hint / position
/// the REQ-AXO-129 string envelope (`db_error_envelope`) builds, so the
/// async migration preserves the rich error contract WITHOUT the
/// `{`-prefixed string marshalling. `GraphStore::pg_to_anyhow` maps it onto
/// the canonical "Graph plugin error: …" anyhow message callers expect.
#[derive(Debug, Clone, Default)]
pub struct PgErrorDetail {
    pub code: Option<String>,
    pub severity: Option<String>,
    pub message: String,
    pub detail: Option<String>,
    pub hint: Option<String>,
    pub position: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PgError {
    pub stage: String,
    pub sql_excerpt: String,
    pub detail: PgErrorDetail,
}

impl PgError {
    fn acquire(stage: &str, sql: &str, err: String) -> Self {
        PgError {
            stage: stage.to_string(),
            sql_excerpt: sql.chars().take(240).collect(),
            detail: PgErrorDetail {
                message: err,
                ..Default::default()
            },
        }
    }

    fn from_tokio(stage: &str, sql: &str, err: &tokio_postgres::Error) -> Self {
        let mut detail = PgErrorDetail {
            message: err.to_string(),
            ..Default::default()
        };
        if let Some(db) = err.as_db_error() {
            detail.code = Some(db.code().code().to_string());
            detail.severity = Some(db.severity().to_string());
            detail.message = db.message().to_string();
            detail.detail = db.detail().map(|s| s.to_string());
            detail.hint = db.hint().map(|s| s.to_string());
            detail.position = db.position().map(|p| format!("{p:?}"));
        }
        PgError {
            stage: stage.to_string(),
            sql_excerpt: sql.chars().take(240).collect(),
            detail,
        }
    }
}

impl std::fmt::Display for PgError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.stage, self.detail.message)?;
        if let Some(code) = &self.detail.code {
            write!(f, " [SQLSTATE {code}]")?;
        }
        Ok(())
    }
}

impl std::error::Error for PgError {}

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
    // REQ-AXO-901960 — the temporal family was falling through to the
    // `<unsupported type ...>` sentinel, so e.g. `axon.mcp_friction.last_observed_at`
    // (timestamptz) was unreadable via the `sql` tool — an LLM (or operator)
    // querying any timestamped row got a useless placeholder. Decoded here via
    // tokio-postgres' already-enabled `with-chrono-0_4` feature (no new
    // dependency). timestamptz → RFC3339 so the value round-trips back into a
    // WHERE clause. (numeric remains the ::BIGINT/::TEXT cast workaround pending
    // a `rust_decimal` decision — REQ-AXO-901905 sibling.)
    if ty == Type::TIMESTAMPTZ {
        if let Ok(v) = row.try_get::<_, Option<chrono::DateTime<chrono::Utc>>>(col) {
            return v.map(|t| t.to_rfc3339()).unwrap_or_else(|| "null".into());
        }
    }
    if ty == Type::TIMESTAMP {
        if let Ok(v) = row.try_get::<_, Option<chrono::NaiveDateTime>>(col) {
            return v.map(|t| t.to_string()).unwrap_or_else(|| "null".into());
        }
    }
    if ty == Type::DATE {
        if let Ok(v) = row.try_get::<_, Option<chrono::NaiveDate>>(col) {
            return v.map(|d| d.to_string()).unwrap_or_else(|| "null".into());
        }
    }
    if ty == Type::TIME {
        if let Ok(v) = row.try_get::<_, Option<chrono::NaiveTime>>(col) {
            return v.map(|t| t.to_string()).unwrap_or_else(|| "null".into());
        }
    }
    format!("<unsupported type {}>", ty.name())
}
