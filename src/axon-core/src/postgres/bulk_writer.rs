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

use crate::graph_ingestion::async_writer::ChunkEmbeddingPersistRow;

/// Re-export so external integration tests can construct flush
/// payloads without needing access to the `pub(crate)` async_writer
/// module. Production callers (e.g. `update_chunk_embeddings`) still
/// reach the type through `crate::graph_ingestion::async_writer`.
pub use crate::graph_ingestion::async_writer::ChunkEmbeddingPersistRow as BulkWriterChunkEmbeddingRow;

static RUNTIME: OnceLock<Runtime> = OnceLock::new();
static POOL: OnceLock<Pool> = OnceLock::new();
static VECTOR_TYPE: OnceLock<Type> = OnceLock::new();

/// `AXON_BULK_WRITER_ENABLED` opt-in. Default OFF preserves the legacy
/// `upsert_chunk_embedding_sql` path bit-for-bit so the existing test
/// suite stays green; only PG bench cells flip it on.
pub fn bulk_writer_enabled() -> bool {
    std::env::var("AXON_BULK_WRITER_ENABLED")
        .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
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
    for key in [
        "AXON_LIVE_DATABASE_URL",
        "AXON_DEV_DATABASE_URL",
        "DATABASE_URL",
    ] {
        if let Ok(v) = std::env::var(key) {
            if !v.trim().is_empty() {
                return Ok(v);
            }
        }
    }
    Err(anyhow!(
        "bulk_writer requires AXON_LIVE_DATABASE_URL / AXON_DEV_DATABASE_URL / DATABASE_URL"
    ))
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

/// Sync entrypoint called by `update_chunk_embeddings` under PG when
/// `AXON_BULK_WRITER_ENABLED=true`. Idempotent on chunk_id+model_id
/// via `ON CONFLICT … DO UPDATE` so retried flushes converge.
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

async fn flush_chunk_embeddings_async(
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

    // Stage in a TEMP table mirroring public.ChunkEmbedding so we can
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

    tx.batch_execute(
        "INSERT INTO public.ChunkEmbedding \
            (chunk_id, model_id, project_code, source_hash, embedding, embedded_at_ms) \
         SELECT chunk_id, model_id, project_code, source_hash, embedding, embedded_at_ms \
         FROM _bulk_chunk_embedding_stage \
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bulk_writer_disabled_by_default() {
        // Sanity gate: env var unset == OFF. Other tests may set the
        // env, so this only asserts the contract for unset / falsey.
        std::env::remove_var("AXON_BULK_WRITER_ENABLED");
        assert!(!bulk_writer_enabled());
        std::env::set_var("AXON_BULK_WRITER_ENABLED", "0");
        assert!(!bulk_writer_enabled());
        std::env::remove_var("AXON_BULK_WRITER_ENABLED");
    }

    #[test]
    fn bulk_writer_truthy_values_enable() {
        for v in ["1", "true", "TRUE", "yes", "on"] {
            std::env::set_var("AXON_BULK_WRITER_ENABLED", v);
            assert!(bulk_writer_enabled(), "value {v:?} should enable");
        }
        std::env::remove_var("AXON_BULK_WRITER_ENABLED");
    }

    #[test]
    fn flush_chunk_embeddings_on_empty_input_is_noop() {
        // No DB connection required — empty fast path returns Ok.
        let res = flush_chunk_embeddings("AXO", "model", &[], 0);
        assert!(res.is_ok(), "empty flush must not touch the DB");
    }
}
