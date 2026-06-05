//! REQ-AXO-238: PostgreSQL bulk writer using COPY BINARY.
//!
//! Replaces the per-row `INSERT ... ON CONFLICT` SQL emitter on the
//! ChunkEmbedding hot path with a single COPY BINARY into a temp
//! staging table + a single `INSERT ... SELECT ... ON CONFLICT DO
//! UPDATE` merge. Per VAL-AXO-044 the writer mutex is the dominant
//! bottleneck under PG; bulk-loading 10K embeddings in one COPY
//! removes most of the per-row overhead.
//!
//! Surface (sync):
//! - [`bulk_writer_enabled`]: reads `AXON_BULK_WRITER_ENABLED`.
//! - [`flush_chunk_embeddings`]: blocks the caller until the COPY +
//!   merge transaction commits. Internally drives a private
//!   `tokio::Runtime` + `deadpool_postgres::Pool`. Both are lazy
//!   `OnceLock`s so the first call pays the construction cost; later
//!   calls reuse the same pool.
//!
//! The pool reads `AXON_LIVE_DATABASE_URL` first, then
//! `AXON_DEV_DATABASE_URL`, then `DATABASE_URL`. Mirrors the
//! plugin's resolution order so the bulk_writer connects to the same
//! instance the FFI plugin already targets.
//!
//! pgvector's `vector` type has a runtime-assigned OID, so the type
//! is looked up via `pg_type` once per pool initialisation and cached
//! in [`VectorType`] for `BinaryCopyInWriter::new` to use.

use std::sync::OnceLock;

use anyhow::{anyhow, Context, Result};
use deadpool_postgres::{Config, ManagerConfig, Pool, RecyclingMethod, Runtime as DpRuntime};
use futures_util::pin_mut;
use pgvector::Vector;
use tokio::runtime::{Builder as RtBuilder, Runtime};
use tokio_postgres::binary_copy::BinaryCopyInWriter;
use tokio_postgres::types::{Kind, Type};
use tokio_postgres::NoTls;

use crate::graph_ingestion::rows::{
    ChunkEmbeddingPersistRow, ChunkRow, RelationRow, SymbolRow,
};

/// Re-export so external integration tests can construct flush
/// payloads. The carrier lives in `crate::graph_ingestion::rows`; the
/// production embedding writer (`upsert_chunk_embedding_v2_batch`) builds
/// it and flushes via `NativePgCtx::flush_chunk_embeddings_copy`.
pub use crate::graph_ingestion::rows::ChunkEmbeddingPersistRow as BulkWriterChunkEmbeddingRow;
pub use crate::graph_ingestion::rows::ChunkRow as BulkWriterChunkRow;
pub use crate::graph_ingestion::rows::RelationRow as BulkWriterRelationRow;
pub use crate::graph_ingestion::rows::SymbolRow as BulkWriterSymbolRow;

static RUNTIME: OnceLock<Runtime> = OnceLock::new();
static POOL: OnceLock<Pool> = OnceLock::new();
static VECTOR_TYPE: OnceLock<Type> = OnceLock::new();

/// Tri-state override of the adaptive COPY dispatch (REQ-AXO-901881 W3 #34,
/// VAL-AXO-067). `Some(true)` = force COPY BINARY for every flush (bench /
/// explicit opt-in); `Some(false)` = force per-row INSERT for every flush;
/// `None` (unset) = adaptive — COPY only when the batch is large enough to
/// amortise its fixed setup cost (see [`should_use_bulk_writer`]).
pub fn bulk_writer_override() -> Option<bool> {
    std::env::var("AXON_BULK_WRITER_ENABLED")
        .ok()
        .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
}

/// Back-compat predicate: `true` iff COPY is *force-enabled* via env. The
/// adaptive default (env unset) reports `false` here, so the live dispatch
/// MUST use [`should_use_bulk_writer`] which also honours batch size.
pub fn bulk_writer_enabled() -> bool {
    matches!(bulk_writer_override(), Some(true))
}

/// VAL-AXO-067 crossover. COPY BINARY's fixed setup cost (temp staging
/// table DDL + COPY + `INSERT … SELECT … ON CONFLICT` + cleanup) *regresses*
/// throughput vs per-row INSERT for the small ~5-row LINGER flushes of
/// steady-state cruise (measured −18%, peak 78→50 ch/s) but wins by ~an
/// order of magnitude once the batch amortises it (the deferred one-shot
/// full-IST load drains 187K embeddings in large batches). The default
/// sits with a safe margin above the ~50–100-row modelled crossover and
/// well below the 1024-row proven-win point. Tunable via
/// `AXON_BULK_WRITER_MIN_ROWS` for bench sweeps.
pub const BULK_WRITER_MIN_PROFITABLE_ROWS_DEFAULT: usize = 256;

/// Resolve the adaptive COPY threshold (env override → default).
pub fn bulk_writer_min_profitable_rows() -> usize {
    std::env::var("AXON_BULK_WRITER_MIN_ROWS")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(BULK_WRITER_MIN_PROFITABLE_ROWS_DEFAULT)
}

/// Adaptive dispatch (REQ-AXO-901881 W3 #34): pick COPY BINARY only when it
/// pays off. An explicit env override wins; otherwise the decision gates on
/// `row_count` so steady-state cruise stays on per-row INSERT (no VAL-AXO-067
/// regression) while bulk loads route through COPY.
pub fn should_use_bulk_writer(row_count: usize) -> bool {
    match bulk_writer_override() {
        Some(forced) => forced,
        None => row_count >= bulk_writer_min_profitable_rows(),
    }
}

fn runtime() -> Result<&'static Runtime> {
    if let Some(rt) = RUNTIME.get() {
        return Ok(rt);
    }
    let rt = RtBuilder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .thread_name("axon-bulk-writer")
        .build()
        .context("bulk_writer tokio runtime build failed")?;
    Ok(RUNTIME.get_or_init(|| rt))
}

fn resolve_database_url() -> Result<String> {
    // REQ-AXO-901881 W2 (#17) — delegate to THE canonical resolver. This was
    // one of 4 divergent copies (its own comment documents the REQ-AXO-315
    // dev→live leak that the drift caused); resolution now lives only in
    // postgres::resolve_database_url.
    crate::postgres::resolve_database_url(None)
}

fn pool() -> Result<&'static Pool> {
    if let Some(p) = POOL.get() {
        return Ok(p);
    }
    let url = resolve_database_url()?;
    let mut cfg = Config::new();
    cfg.url = Some(url);
    cfg.manager = Some(ManagerConfig {
        recycling_method: RecyclingMethod::Fast,
    });
    let p = cfg
        .create_pool(Some(DpRuntime::Tokio1), NoTls)
        .context("bulk_writer pool creation failed")?;
    Ok(POOL.get_or_init(|| p))
}

async fn vector_type(client: &mut deadpool_postgres::Client) -> Result<Type> {
    if let Some(t) = VECTOR_TYPE.get() {
        return Ok(t.clone());
    }
    // pgvector docs (postgres_ext/vector.rs#tests): look up the
    // `vector` type's OID dynamically because the type is registered
    // by the extension at runtime, not the postgres core protocol.
    let row = client
        .query_one(
            "SELECT pg_type.oid AS oid, nspname AS schema \
             FROM pg_type \
             INNER JOIN pg_namespace ON pg_namespace.oid = pg_type.typnamespace \
             WHERE typname = $1",
            &[&"vector"],
        )
        .await
        .context("pgvector type lookup failed (extension not loaded?)")?;
    let oid: tokio_postgres::types::Oid = row.get("oid");
    let schema: String = row.get("schema");
    let t = Type::new("vector".to_string(), oid, Kind::Simple, schema);
    let _ = VECTOR_TYPE.set(t.clone());
    Ok(t)
}

/// Sync entrypoint over this module's own global pool — used by the
/// standalone `axon-bench-writer` binary (which sets its own DB URL). The
/// production pipeline routes through `NativePgCtx::flush_chunk_embeddings_copy`
/// (this module's `flush_chunk_embeddings_async` on the store's own pool).
/// Idempotent on chunk_id+model_id via `ON CONFLICT … DO UPDATE`.
pub fn flush_chunk_embeddings(
    project_code: &str,
    model_id: &str,
    rows: &[ChunkEmbeddingPersistRow],
    embedded_at_ms: i64,
) -> Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let rt = runtime()?;
    let pool = pool()?;
    rt.block_on(async {
        let mut client = pool
            .get()
            .await
            .context("bulk_writer pool acquire failed")?;
        flush_chunk_embeddings_async(&mut client, project_code, model_id, rows, embedded_at_ms)
            .await
    })
}

pub(crate) async fn flush_chunk_embeddings_async(
    client: &mut deadpool_postgres::Client,
    project_code: &str,
    model_id: &str,
    rows: &[ChunkEmbeddingPersistRow],
    embedded_at_ms: i64,
) -> Result<()> {
    // Idempotent guard: ensure pgvector's `vector` type is reachable
    // for this session. The bulk_writer pool is independent from the
    // FFI plugin pool. We run CREATE EXTENSION + the type lookup +
    // the search_path adjustment OUTSIDE the bulk transaction.
    client
        .batch_execute("CREATE EXTENSION IF NOT EXISTS vector")
        .await
        .context("bulk_writer ensure pgvector extension")?;

    // Diagnostic: confirm the extension is actually registered before
    // we issue any DDL that references the `vector` type. Some test
    // environments have pgvector available in pg_available_extensions
    // but unable to install due to permission or path issues; without
    // this check the failure surfaces as a confusing
    // "type vector does not exist" inside the TEMP TABLE create.
    let ext_check = client
        .query_one(
            "SELECT count(*)::BIGINT FROM pg_extension WHERE extname='vector'",
            &[],
        )
        .await
        .context("bulk_writer pg_extension probe")?;
    let ext_count: i64 = ext_check.get(0);
    if ext_count == 0 {
        return Err(anyhow!(
            "bulk_writer: CREATE EXTENSION vector reported success but \
             pg_extension table shows 0 rows for extname='vector'. \
             Verify pgvector is installed in the running image \
             (combined axon-test/age-pgvector should ship it). \
             current_database / current_user can be checked via \
             AXON_LIVE_DATABASE_URL."
        ));
    }

    let vec_type = vector_type(client).await?;
    let vec_schema = vec_type.schema();

    // Stage in a TEMP table mirroring ist.ChunkEmbedding so we can
    // ON CONFLICT-merge after COPY BINARY. COPY itself doesn't accept
    // ON CONFLICT semantics. The temp is dropped on tx commit so
    // there's no cross-call visibility / clean-up concern.
    //
    // Schema-qualify the `vector(...)` type with the schema returned
    // by `pg_namespace` so the parser resolves it regardless of the
    // session's `search_path`. tokio_postgres / deadpool may reset
    // SET locals at transaction boundaries, and the combined
    // axon-test/age-pgvector image installs pgvector in `public` —
    // grabbing the schema dynamically keeps this resilient if a
    // future image moves it to `extensions` (PG14+ default).
    let stage_ddl = format!(
        "CREATE TEMP TABLE _bulk_chunk_embedding_stage (\
            chunk_id TEXT NOT NULL,\
            model_id TEXT NOT NULL,\
            project_code TEXT NOT NULL,\
            source_hash TEXT NOT NULL,\
            embedding {schema}.vector({dim}),\
            embedded_at_ms BIGINT NOT NULL\
         ) ON COMMIT DROP",
        schema = vec_schema,
        dim = crate::embedding_contract::DIMENSION,
    );

    // Single transaction so the stage table, COPY, and merge are
    // atomic — a crash mid-merge rolls back cleanly and the FVQ
    // retry contract restores the file.
    let tx = client
        .transaction()
        .await
        .context("bulk_writer begin tx")?;
    tx.batch_execute(&stage_ddl)
        .await
        .context("bulk_writer stage table create")?;

    let copy_sink = tx
        .copy_in(
            "COPY _bulk_chunk_embedding_stage \
                  (chunk_id, model_id, project_code, source_hash, embedding, embedded_at_ms) \
                  FROM STDIN BINARY",
        )
        .await
        .context("bulk_writer copy_in begin")?;
    let column_types = [
        Type::TEXT,
        Type::TEXT,
        Type::TEXT,
        Type::TEXT,
        vec_type,
        Type::INT8,
    ];
    let writer = BinaryCopyInWriter::new(copy_sink, &column_types);
    pin_mut!(writer);

    for row in rows {
        let v = Vector::from(row.embedding.clone());
        writer
            .as_mut()
            .write(&[
                &row.chunk_id,
                &model_id,
                &project_code,
                &row.source_hash,
                &v,
                &embedded_at_ms,
            ])
            .await
            .context("bulk_writer copy row write")?;
    }
    let _rows_written = writer
        .finish()
        .await
        .context("bulk_writer copy_in finish")?;

    // REQ-AXO-901884 — JOIN ist.Chunk so a chunk_id deleted by re-index churn
    // between the demand-pull SELECT and this merge is skipped (no FK 23503 ->
    // no tx abort -> no pooled-conn poison), instead of aborting the whole batch.
    // DISTINCT ON (chunk_id, model_id) collapses any duplicate staging rows so
    // the merge cannot affect the same ON CONFLICT target twice (SQLSTATE 21000);
    // last embedded_at_ms wins (rows for one chunk are identical anyway). Keeps
    // the COPY path self-defending regardless of caller-side dedup.
    tx.batch_execute(
        "INSERT INTO ist.ChunkEmbedding \
            (chunk_id, model_id, project_code, source_hash, embedding, embedded_at_ms) \
         SELECT DISTINCT ON (s.chunk_id, s.model_id) \
                s.chunk_id, s.model_id, s.project_code, s.source_hash, s.embedding, s.embedded_at_ms \
         FROM _bulk_chunk_embedding_stage s \
         JOIN ist.Chunk c ON c.id = s.chunk_id \
         ORDER BY s.chunk_id, s.model_id, s.embedded_at_ms DESC \
         ON CONFLICT (chunk_id, model_id) DO UPDATE SET \
            project_code = EXCLUDED.project_code, \
            source_hash = EXCLUDED.source_hash, \
            embedding = EXCLUDED.embedding, \
            embedded_at_ms = EXCLUDED.embedded_at_ms",
    )
    .await
    .context("bulk_writer stage merge")?;

    tx.commit().await.context("bulk_writer commit")?;
    Ok(())
}

// RelationTable enum removed — legacy per-type CONTAINS/CALLS/CALLS_NIF
// tables retired. All edges go through unified ist.Edge via
// copy_edges_in_tx (REQ-AXO-901747).

/// Sync entrypoint for the producer hot path: flush a Symbol batch
/// via COPY BINARY into a temp staging table + ON CONFLICT merge.
///
/// Mirrors `flush_chunk_embeddings`'s contract:
///   - opens its own transaction (atomic per-call),
///   - idempotent on the PK `id` so retries converge,
///   - default-OFF when `AXON_BULK_WRITER_ENABLED` is unset (the call
///     site is responsible for the env check; this entrypoint always
///     flushes when invoked).
pub fn flush_symbols(rows: &[SymbolRow]) -> Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let rt = runtime()?;
    let pool = pool()?;
    rt.block_on(async {
        let mut client = pool
            .get()
            .await
            .context("bulk_writer pool acquire failed")?;
        flush_symbols_async(&mut client, rows).await
    })
}

async fn flush_symbols_async(
    client: &mut deadpool_postgres::Client,
    rows: &[SymbolRow],
) -> Result<()> {
    client
        .batch_execute("CREATE EXTENSION IF NOT EXISTS vector")
        .await
        .context("bulk_writer ensure pgvector extension (Symbol)")?;
    let vec_type = vector_type(client).await?;
    let tx = client
        .transaction()
        .await
        .context("bulk_writer Symbol begin tx")?;
    copy_symbols_in_tx(&tx, rows, vec_type).await?;
    tx.commit().await.context("bulk_writer Symbol commit")?;
    Ok(())
}

/// Sync entrypoint for the producer Chunk hot path. Same contract as
/// `flush_symbols`: own transaction, idempotent on PK `id`, no env
/// check (caller decides via `bulk_writer_enabled()`).
pub fn flush_chunks(rows: &[ChunkRow]) -> Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let rt = runtime()?;
    let pool = pool()?;
    rt.block_on(async {
        let mut client = pool
            .get()
            .await
            .context("bulk_writer pool acquire failed")?;
        flush_chunks_async(&mut client, rows).await
    })
}

async fn flush_chunks_async(
    client: &mut deadpool_postgres::Client,
    rows: &[ChunkRow],
) -> Result<()> {
    let tx = client
        .transaction()
        .await
        .context("bulk_writer Chunk begin tx")?;
    copy_chunks_in_tx(&tx, rows).await?;
    tx.commit().await.context("bulk_writer Chunk commit")?;
    Ok(())
}

/// Sync entrypoint for the producer relation hot path. Targets one of
/// CONTAINS / CALLS / CALLS_NIF (selected by `table`). All three share
/// the same `(source_id, target_id, project_code)` triple shape and
/// the same PK `(source_id, target_id)`.
///
/// Merge semantics:
///   - CONTAINS: `ON CONFLICT DO NOTHING` — preserves the legacy
///     `insert_unique_relation_queries` behavior (additive, no overwrite).
///   - CALLS / CALLS_NIF: `ON CONFLICT DO UPDATE SET project_code = EXCLUDED.project_code`
///     — preserves the legacy DELETE-then-INSERT "replace" behavior. With
///     `(source_id, target_id)` as PK and `project_code` as the only
///     non-PK column, the DO UPDATE is a no-op when the value matches
///     and an actual update otherwise; both outcomes are equivalent to
///     the legacy DELETE+INSERT for a single producer batch.
// flush_relations removed — legacy per-type relation tables retired.
// Use flush_batch with PgBulkBatch.

/// Cross-table atomic flush. One transaction covers Symbol, Chunk, and
/// the three relation tables. A crash mid-flush rolls back every table
/// cleanly; the FVQ retry contract restores the file from scratch.
///
/// Per-table flush_* callers retain their per-call atomicity; this
/// entrypoint is the producer hot path under
/// `backend_is_pg && bulk_writer_enabled`. ChunkEmbedding stays on its
/// own dedicated entrypoint (`flush_chunk_embeddings`) because the
/// vectorization lane writes embeddings out-of-band after the producer
/// commits.
#[derive(Debug, Default, Clone)]
pub struct PgBulkBatch {
    pub symbols: Vec<SymbolRow>,
    pub chunks: Vec<ChunkRow>,
    pub contains: Vec<RelationRow>,
    pub calls: Vec<RelationRow>,
    pub calls_nif: Vec<RelationRow>,
    pub indexed_files: Vec<(String, String, i64, i64, i64)>,
    /// REQ-AXO-901860 — the single project_code this batch belongs to
    /// (A3 groups by resolved project_code, one group per flush). The
    /// writer uses it to UPSERT the `ist.Project` FK parent first and to
    /// stamp `ist.IndexedFile.project_code` directly, so a file reaching
    /// A3 before the scanner enrolled it still satisfies the NOT NULL FK
    /// instead of poisoning the whole writer transaction with a FK
    /// violation (25P02 cascade). Empty only for the legacy single-row
    /// entrypoints that never carry a batch.
    pub project_code: String,
}

impl PgBulkBatch {
    pub fn is_empty(&self) -> bool {
        self.symbols.is_empty()
            && self.chunks.is_empty()
            && self.contains.is_empty()
            && self.calls.is_empty()
            && self.calls_nif.is_empty()
            && self.indexed_files.is_empty()
    }

    pub fn row_count(&self) -> usize {
        self.symbols.len()
            + self.chunks.len()
            + self.contains.len()
            + self.calls.len()
            + self.calls_nif.len()
            + self.indexed_files.len()
    }
}

/// Sync entrypoint that flushes a `PgBulkBatch` atomically. All five
/// table writes share one transaction so a single producer batch
/// either lands fully or rolls back fully.
pub fn flush_batch(batch: &PgBulkBatch) -> Result<()> {
    if batch.is_empty() {
        return Ok(());
    }
    let rt = runtime()?;
    let pool = pool()?;
    rt.block_on(async {
        let mut client = pool
            .get()
            .await
            .context("bulk_writer pool acquire failed")?;
        flush_batch_async(&mut client, batch).await
    })
}

async fn flush_batch_async(
    client: &mut deadpool_postgres::Client,
    batch: &PgBulkBatch,
) -> Result<()> {
    // Pre-tx: ensure pgvector extension + cache the runtime-assigned
    // OID once. Both are idempotent and stay outside the bulk tx so a
    // failed extension load doesn't poison the whole batch.
    if !batch.symbols.is_empty() {
        client
            .batch_execute("CREATE EXTENSION IF NOT EXISTS vector")
            .await
            .context("bulk_writer batch ensure pgvector extension")?;
    }
    let vec_type_opt: Option<Type> = if batch.symbols.is_empty() {
        None
    } else {
        Some(vector_type(client).await?)
    };

    let tx = client
        .transaction()
        .await
        .context("bulk_writer batch begin tx")?;

    // REQ-AXO-901860 — guarantee the FK parents exist before any child row.
    // Symbol / Chunk / IndexedFile all carry a NOT NULL project_code FK to
    // ist.Project; Chunk additionally FKs ist.IndexedFile(path). A file that
    // reaches A3 before the scanner enrolled it (the bootstrap walk feeds A1
    // directly) used to fail the Symbol/Chunk insert with a FK violation,
    // aborting the tx and poisoning the pooled connection — a 25P02 cascade
    // that blocked embeddings, the heartbeat UPSERT, and dashboard_state for
    // every project sharing the connection. The writer now owns its FK
    // parents: ensure ist.Project, then ist.IndexedFile, before the children.
    if !batch.project_code.is_empty() {
        let now_ms = chrono::Utc::now().timestamp_millis();
        tx.execute(
            "INSERT INTO Project (code, enrolled_at_ms) VALUES ($1, $2) \
             ON CONFLICT (code) DO NOTHING",
            &[&batch.project_code, &now_ms],
        )
        .await
        .context("bulk_writer batch ensure ist.Project FK parent")?;
    }
    if !batch.indexed_files.is_empty() {
        copy_indexed_files_in_tx(&tx, &batch.indexed_files, &batch.project_code).await?;
    }
    if !batch.symbols.is_empty() {
        let vec_type = vec_type_opt
            .as_ref()
            .expect("vec_type set when symbols.is_empty == false")
            .clone();
        copy_symbols_in_tx(&tx, &batch.symbols, vec_type).await?;
    }
    if !batch.chunks.is_empty() {
        copy_chunks_in_tx(&tx, &batch.chunks).await?;
    }
    // REQ-AXO-901747 — unified Edge table (post-MIL-AXO-017).
    let has_edges = !batch.contains.is_empty()
        || !batch.calls.is_empty()
        || !batch.calls_nif.is_empty();
    if has_edges {
        let mut edge_rows: Vec<(&str, &RelationRow)> = Vec::new();
        for r in &batch.contains {
            edge_rows.push(("CONTAINS", r));
        }
        for r in &batch.calls {
            edge_rows.push(("CALLS", r));
        }
        for r in &batch.calls_nif {
            edge_rows.push(("CALLS_NIF", r));
        }
        copy_edges_in_tx(&tx, &edge_rows).await?;
    }

    tx.commit().await.context("bulk_writer batch commit")?;
    Ok(())
}

async fn copy_symbols_in_tx(
    tx: &deadpool_postgres::Transaction<'_>,
    rows: &[SymbolRow],
    vec_type: Type,
) -> Result<()> {
    let vec_schema = vec_type.schema().to_string();
    let stage_ddl = format!(
        "CREATE TEMP TABLE _bulk_symbol_stage (\
            id TEXT NOT NULL,\
            name TEXT NOT NULL,\
            kind TEXT,\
            tested BOOLEAN NOT NULL,\
            is_public BOOLEAN NOT NULL,\
            is_nif BOOLEAN NOT NULL,\
            is_unsafe BOOLEAN NOT NULL,\
            project_code TEXT NOT NULL,\
            embedding {schema}.vector({dim})\
         ) ON COMMIT DROP",
        schema = vec_schema,
        dim = crate::embedding_contract::DIMENSION,
    );
    tx.batch_execute(&stage_ddl)
        .await
        .context("bulk_writer Symbol stage create (batch)")?;

    let copy_sink = tx
        .copy_in(
            "COPY _bulk_symbol_stage \
                  (id, name, kind, tested, is_public, is_nif, is_unsafe, project_code, embedding) \
                  FROM STDIN BINARY",
        )
        .await
        .context("bulk_writer Symbol copy_in begin (batch)")?;
    let column_types = [
        Type::TEXT,
        Type::TEXT,
        Type::TEXT,
        Type::BOOL,
        Type::BOOL,
        Type::BOOL,
        Type::BOOL,
        Type::TEXT,
        vec_type,
    ];
    let writer = BinaryCopyInWriter::new(copy_sink, &column_types);
    pin_mut!(writer);
    for row in rows {
        let embed_opt: Option<Vector> = match row.embedding.as_ref() {
            None => None,
            Some(v) => {
                if v.len() == crate::embedding_contract::DIMENSION {
                    Some(Vector::from(v.clone()))
                } else {
                    log::warn!(
                        "bulk_writer Symbol embedding dim mismatch for {}: expected {}, got {}; falling back to NULL",
                        row.symbol_id,
                        crate::embedding_contract::DIMENSION,
                        v.len()
                    );
                    None
                }
            }
        };
        writer
            .as_mut()
            .write(&[
                &row.symbol_id,
                &row.name,
                &row.kind,
                &row.tested,
                &row.is_public,
                &row.is_nif,
                &row.is_unsafe,
                &row.project_code,
                &embed_opt,
            ])
            .await
            .context("bulk_writer Symbol copy row write (batch)")?;
    }
    let _written = writer
        .finish()
        .await
        .context("bulk_writer Symbol copy_in finish (batch)")?;

    // REQ-AXO-901884 — DISTINCT ON (id) collapses duplicate staging rows so the
    // merge cannot affect the same ON CONFLICT target twice (SQLSTATE 21000); an
    // A3 batch can carry the same symbol id more than once. Rows for one id are
    // identical, so any winner is correct.
    tx.batch_execute(
        "INSERT INTO ist.Symbol \
            (id, name, kind, tested, is_public, is_nif, is_unsafe, project_code, embedding) \
         SELECT DISTINCT ON (id) \
                id, name, kind, tested, is_public, is_nif, is_unsafe, project_code, embedding \
         FROM _bulk_symbol_stage \
         ORDER BY id \
         ON CONFLICT (id) DO UPDATE SET \
            name = EXCLUDED.name, \
            kind = EXCLUDED.kind, \
            tested = EXCLUDED.tested, \
            is_public = EXCLUDED.is_public, \
            is_nif = EXCLUDED.is_nif, \
            is_unsafe = EXCLUDED.is_unsafe, \
            project_code = EXCLUDED.project_code, \
            embedding = EXCLUDED.embedding",
    )
    .await
    .context("bulk_writer Symbol stage merge (batch)")?;
    Ok(())
}

async fn copy_chunks_in_tx(
    tx: &deadpool_postgres::Transaction<'_>,
    rows: &[ChunkRow],
) -> Result<()> {
    tx.batch_execute(
        "CREATE TEMP TABLE _bulk_chunk_stage (\
            id TEXT NOT NULL,\
            source_type TEXT,\
            source_id TEXT,\
            project_code TEXT NOT NULL,\
            file_path TEXT,\
            kind TEXT,\
            content TEXT,\
            content_hash TEXT,\
            start_line BIGINT,\
            end_line BIGINT,\
            chunk_part_index BIGINT,\
            chunk_part_count BIGINT,\
            chunk_path TEXT,\
            token_count INTEGER\
         ) ON COMMIT DROP",
    )
    .await
    .context("bulk_writer Chunk stage create (batch)")?;

    let copy_sink = tx
        .copy_in(
            "COPY _bulk_chunk_stage \
                  (id, source_type, source_id, project_code, file_path, kind, content, content_hash, start_line, end_line, chunk_part_index, chunk_part_count, chunk_path, token_count) \
                  FROM STDIN BINARY",
        )
        .await
        .context("bulk_writer Chunk copy_in begin (batch)")?;
    let column_types = [
        Type::TEXT, Type::TEXT, Type::TEXT, Type::TEXT, Type::TEXT,
        Type::TEXT, Type::TEXT, Type::TEXT, Type::INT8, Type::INT8,
        Type::INT8, Type::INT8, Type::TEXT, Type::INT4,
    ];
    let writer = BinaryCopyInWriter::new(copy_sink, &column_types);
    pin_mut!(writer);
    for row in rows {
        let tc: Option<i32> = row.token_count.map(|v| v as i32);
        writer
            .as_mut()
            .write(&[
                &row.chunk_id as &(dyn tokio_postgres::types::ToSql + Sync),
                &row.source_type, &row.source_id, &row.project_code,
                &row.file_path, &row.kind, &row.content, &row.content_hash,
                &row.start_line, &row.end_line, &row.part_index,
                &row.part_count, &row.chunk_path, &tc,
            ])
            .await
            .context("bulk_writer Chunk copy row write (batch)")?;
    }
    let _written = writer
        .finish()
        .await
        .context("bulk_writer Chunk copy_in finish (batch)")?;

    // REQ-AXO-901884 — DISTINCT ON (id) collapses duplicate staging rows so the
    // merge cannot affect the same ON CONFLICT target twice (SQLSTATE 21000).
    // Confirmed in dev: A3 batches carry the same chunk id more than once
    // ("bulk_writer Chunk stage merge" 21000). Rows for one id are identical.
    tx.batch_execute(
        "INSERT INTO ist.Chunk \
            (id, source_type, source_id, project_code, file_path, kind, content, content_hash, start_line, end_line, chunk_part_index, chunk_part_count, chunk_path, token_count) \
         SELECT DISTINCT ON (id) \
                id, source_type, source_id, project_code, file_path, kind, content, content_hash, start_line, end_line, chunk_part_index, chunk_part_count, chunk_path, token_count \
         FROM _bulk_chunk_stage \
         ORDER BY id \
         ON CONFLICT (id) DO UPDATE SET \
            source_type = EXCLUDED.source_type, \
            source_id = EXCLUDED.source_id, \
            project_code = EXCLUDED.project_code, \
            file_path = EXCLUDED.file_path, \
            kind = EXCLUDED.kind, \
            content = EXCLUDED.content, \
            content_hash = EXCLUDED.content_hash, \
            start_line = EXCLUDED.start_line, \
            end_line = EXCLUDED.end_line, \
            chunk_part_index = EXCLUDED.chunk_part_index, \
            chunk_part_count = EXCLUDED.chunk_part_count, \
            chunk_path = EXCLUDED.chunk_path, \
            token_count = EXCLUDED.token_count, \
            embed_status = CASE \
                WHEN Chunk.content_hash IS DISTINCT FROM EXCLUDED.content_hash \
                THEN 'pending' ELSE Chunk.embed_status END",
    )
    .await
    .context("bulk_writer Chunk stage merge (batch)")?;
    Ok(())
}

/// REQ-AXO-901747 — COPY BINARY into unified `ist.Edge`.
async fn copy_edges_in_tx(
    tx: &deadpool_postgres::Transaction<'_>,
    rows: &[(&str, &RelationRow)],
) -> Result<()> {
    let stage_ddl = "CREATE TEMP TABLE _bulk_edge_stage (\
            source_id TEXT NOT NULL,\
            target_id TEXT NOT NULL,\
            relation_type TEXT NOT NULL,\
            project_code TEXT NOT NULL,\
            created_at_ms BIGINT NOT NULL\
         ) ON COMMIT DROP";
    tx.batch_execute(stage_ddl)
        .await
        .context("bulk_writer edge stage create")?;

    let copy_stmt = "COPY _bulk_edge_stage (source_id, target_id, relation_type, project_code, created_at_ms) FROM STDIN BINARY";
    let copy_sink = tx
        .copy_in(copy_stmt)
        .await
        .context("bulk_writer edge copy_in begin")?;
    let column_types = [Type::TEXT, Type::TEXT, Type::TEXT, Type::TEXT, Type::INT8];
    let writer = BinaryCopyInWriter::new(copy_sink, &column_types);
    pin_mut!(writer);
    let now_ms = chrono::Utc::now().timestamp_millis();
    for (rel_type, row) in rows {
        writer
            .as_mut()
            .write(&[
                &row.source_id as &(dyn tokio_postgres::types::ToSql + Sync),
                &row.target_id,
                &rel_type.to_string(),
                &row.project_code,
                &now_ms,
            ])
            .await
            .context("bulk_writer edge copy row write")?;
    }
    writer.finish().await.context("bulk_writer edge copy_in finish")?;

    let merge_sql = "INSERT INTO ist.edge (source_id, target_id, relation_type, project_code, created_at_ms) \
         SELECT source_id, target_id, relation_type, project_code, created_at_ms FROM _bulk_edge_stage \
         ON CONFLICT (source_id, target_id, relation_type, project_code) DO NOTHING";
    tx.batch_execute(merge_sql)
        .await
        .context("bulk_writer edge stage merge")?;
    Ok(())
}

/// REQ-AXO-901747 — COPY BINARY for IndexedFile rows.
async fn copy_indexed_files_in_tx(
    tx: &deadpool_postgres::Transaction<'_>,
    rows: &[(String, String, i64, i64, i64)],
    project_code: &str,
) -> Result<()> {
    let stage_ddl = "CREATE TEMP TABLE _bulk_indexedfile_stage (\
            path TEXT NOT NULL,\
            content_hash TEXT NOT NULL,\
            last_seen_ms BIGINT NOT NULL,\
            mtime_ms BIGINT NOT NULL,\
            size_bytes BIGINT NOT NULL\
         ) ON COMMIT DROP";
    tx.batch_execute(stage_ddl)
        .await
        .context("bulk_writer indexedfile stage create")?;

    let copy_stmt = "COPY _bulk_indexedfile_stage (path, content_hash, last_seen_ms, mtime_ms, size_bytes) FROM STDIN BINARY";
    let copy_sink = tx
        .copy_in(copy_stmt)
        .await
        .context("bulk_writer indexedfile copy_in begin")?;
    let column_types = [Type::TEXT, Type::TEXT, Type::INT8, Type::INT8, Type::INT8];
    let writer = BinaryCopyInWriter::new(copy_sink, &column_types);
    pin_mut!(writer);
    for (path, hash, ts, mtime, size) in rows {
        writer
            .as_mut()
            .write(&[
                path as &(dyn tokio_postgres::types::ToSql + Sync),
                hash,
                ts,
                mtime,
                size,
            ])
            .await
            .context("bulk_writer indexedfile copy row write")?;
    }
    writer.finish().await.context("bulk_writer indexedfile copy_in finish")?;

    // REQ-AXO-901860: project_code is a NOT NULL FK to ist.Project. A3 owns
    // its FK parents (the ist.Project row is UPSERTed first in
    // flush_batch_async), so the IndexedFile row is stamped with the batch's
    // resolved project_code directly. The previous JOIN-recovery against an
    // already-discovered IndexedFile row silently DROPPED any file the
    // scanner hadn't enrolled yet — the bootstrap walk feeds A1 directly, so
    // those files reached A3 first and their chunks then failed the
    // chunk_file_path FK. ON CONFLICT keeps an existing row's project_code
    // (DO UPDATE doesn't touch it) so a scanner-discovered file is unaffected.
    // REQ-AXO-901884 — DISTINCT ON (s.path) collapses duplicate staging rows so
    // the merge cannot affect the same ON CONFLICT target twice (SQLSTATE 21000)
    // when a batch re-sees the same path. Keep the latest by last_seen_ms.
    let merge_sql = "INSERT INTO indexedfile \
             (path, project_code, content_hash, last_seen_ms, status, mtime_ms, size_bytes) \
         SELECT DISTINCT ON (s.path) \
                s.path, $1, s.content_hash, s.last_seen_ms, 'indexed', s.mtime_ms, s.size_bytes \
             FROM _bulk_indexedfile_stage s \
         ORDER BY s.path, s.last_seen_ms DESC \
         ON CONFLICT (path) DO UPDATE SET \
             content_hash    = EXCLUDED.content_hash, \
             last_seen_ms    = EXCLUDED.last_seen_ms, \
             mtime_ms        = EXCLUDED.mtime_ms, \
             size_bytes      = EXCLUDED.size_bytes, \
             status          = 'indexed', \
             retry_count     = 0, \
             last_attempt_ms = NULL";
    tx.execute(merge_sql, &[&project_code])
        .await
        .context("bulk_writer indexedfile stage merge")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bulk_writer_env_override_and_adaptive_dispatch() {
        // ONE test owns AXON_BULK_WRITER_ENABLED / AXON_BULK_WRITER_MIN_ROWS.
        // Rust runs tests in parallel threads sharing the process env, so
        // splitting these into separate #[test]s races — a sibling's set_var
        // clobbers this one's assertion mid-run (observed flake on
        // bulk_writer_truthy_values_enable). Kept serial in a single test.
        std::env::remove_var("AXON_BULK_WRITER_ENABLED");
        std::env::remove_var("AXON_BULK_WRITER_MIN_ROWS");

        // Back-compat predicate: unset == OFF, falsey == OFF.
        assert!(!bulk_writer_enabled());
        std::env::set_var("AXON_BULK_WRITER_ENABLED", "0");
        assert!(!bulk_writer_enabled());
        std::env::remove_var("AXON_BULK_WRITER_ENABLED");

        // Truthy values force-enable.
        for v in ["1", "true", "TRUE", "yes", "on"] {
            std::env::set_var("AXON_BULK_WRITER_ENABLED", v);
            assert!(bulk_writer_enabled(), "value {v:?} should enable");
        }
        std::env::remove_var("AXON_BULK_WRITER_ENABLED");

        // Adaptive (env unset): gate on row count around the VAL-AXO-067 crossover.
        let t = bulk_writer_min_profitable_rows();
        assert_eq!(t, BULK_WRITER_MIN_PROFITABLE_ROWS_DEFAULT);
        assert!(!should_use_bulk_writer(t - 1), "small flush stays on INSERT");
        assert!(should_use_bulk_writer(t), "batch at threshold uses COPY");
        assert!(should_use_bulk_writer(t + 10_000), "huge batch uses COPY");

        // Explicit override wins over batch size, both directions.
        std::env::set_var("AXON_BULK_WRITER_ENABLED", "true");
        assert!(should_use_bulk_writer(1), "force-on uses COPY even for 1 row");
        std::env::set_var("AXON_BULK_WRITER_ENABLED", "0");
        assert!(!should_use_bulk_writer(1_000_000), "force-off never uses COPY");
        std::env::remove_var("AXON_BULK_WRITER_ENABLED");

        // Threshold is tunable for bench sweeps.
        std::env::set_var("AXON_BULK_WRITER_MIN_ROWS", "8");
        assert_eq!(bulk_writer_min_profitable_rows(), 8);
        assert!(should_use_bulk_writer(8));
        assert!(!should_use_bulk_writer(7));
        std::env::remove_var("AXON_BULK_WRITER_MIN_ROWS");
    }

    #[test]
    fn flush_chunk_embeddings_on_empty_input_is_noop() {
        // No DB connection required — empty fast path returns Ok.
        let res = flush_chunk_embeddings("AXO", "model", &[], 0);
        assert!(res.is_ok(), "empty flush must not touch the DB");
    }

    #[test]
    fn flush_symbols_on_empty_input_is_noop() {
        // Empty fast path returns Ok without touching the DB or the
        // OnceLock pool. Mirrors flush_chunk_embeddings's contract.
        let res = flush_symbols(&[]);
        assert!(res.is_ok(), "empty Symbol flush must not touch the DB");
    }

    #[test]
    fn flush_chunks_on_empty_input_is_noop() {
        let res = flush_chunks(&[]);
        assert!(res.is_ok(), "empty Chunk flush must not touch the DB");
    }

    #[test]
    fn pg_bulk_batch_default_is_empty() {
        let b = PgBulkBatch::default();
        assert!(b.is_empty());
        assert_eq!(b.row_count(), 0);
    }

    #[test]
    fn pg_bulk_batch_row_count_sums_buckets() {
        let b = PgBulkBatch {
            symbols: vec![SymbolRow {
                symbol_id: "s1".to_string(),
                name: "alpha".to_string(),
                kind: "function".to_string(),
                tested: false,
                is_public: false,
                is_nif: false,
                is_unsafe: false,
                project_code: "AXO".to_string(),
                embedding: None,
            }],
            chunks: vec![ChunkRow {
                chunk_id: "c1".to_string(),
                source_type: "symbol".to_string(),
                source_id: "s1".to_string(),
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
                token_count: Some(11),
            }],
            contains: vec![RelationRow {
                source_id: "/tmp/a.rs".to_string(),
                target_id: "s1".to_string(),
                project_code: "AXO".to_string(),
            }],
            calls: vec![],
            calls_nif: vec![RelationRow {
                source_id: "s1".to_string(),
                target_id: "nif_x".to_string(),
                project_code: "AXO".to_string(),
            }],
            indexed_files: vec![],
            project_code: "AXO".to_string(),
        };
        assert!(!b.is_empty());
        assert_eq!(b.row_count(), 4);
    }

    #[test]
    fn flush_batch_on_empty_input_is_noop() {
        // PgBulkBatch::default() is fully empty. flush_batch returns
        // Ok without touching the runtime/pool OnceLocks — verifying
        // by absence of a runtime panic if AXON_*_DATABASE_URL is unset.
        let res = flush_batch(&PgBulkBatch::default());
        assert!(res.is_ok(), "empty batch flush must not touch the DB");
    }
}
