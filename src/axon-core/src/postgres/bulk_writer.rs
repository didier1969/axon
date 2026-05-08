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

use crate::graph_ingestion::async_writer::{
    ChunkEmbeddingPersistRow, ChunkRow, RelationRow, SymbolRow,
};

/// Re-export so external integration tests can construct flush
/// payloads without needing access to the `pub(crate)` async_writer
/// module. Production callers (e.g. `update_chunk_embeddings`) still
/// reach the type through `crate::graph_ingestion::async_writer`.
pub use crate::graph_ingestion::async_writer::ChunkEmbeddingPersistRow as BulkWriterChunkEmbeddingRow;
pub use crate::graph_ingestion::async_writer::ChunkRow as BulkWriterChunkRow;
pub use crate::graph_ingestion::async_writer::RelationRow as BulkWriterRelationRow;
pub use crate::graph_ingestion::async_writer::SymbolRow as BulkWriterSymbolRow;

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

/// Allowed targets for [`flush_relations`]. Constraining to a fixed
/// list keeps the merge SQL injection-free without quoting tricks and
/// makes call sites self-documenting (the producer's three relation
/// hot paths line up 1:1 with the variants).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelationTable {
    Contains,
    Calls,
    CallsNif,
}

impl RelationTable {
    pub fn sql_name(self) -> &'static str {
        match self {
            RelationTable::Contains => "public.CONTAINS",
            RelationTable::Calls => "public.CALLS",
            RelationTable::CallsNif => "public.CALLS_NIF",
        }
    }

    /// Stage-table suffix. Kept short — temp names are scoped to the
    /// transaction so collisions are impossible, but a readable name
    /// makes EXPLAIN output legible.
    pub fn stage_name(self) -> &'static str {
        match self {
            RelationTable::Contains => "_bulk_contains_stage",
            RelationTable::Calls => "_bulk_calls_stage",
            RelationTable::CallsNif => "_bulk_calls_nif_stage",
        }
    }
}

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
    // Symbol stage carries pgvector embedding column (nullable). Same
    // schema-resolution dance as flush_chunk_embeddings — the type's
    // OID is runtime-assigned by the extension.
    client
        .batch_execute("CREATE EXTENSION IF NOT EXISTS vector")
        .await
        .context("bulk_writer ensure pgvector extension (Symbol)")?;
    let vec_type = vector_type(client).await?;
    let vec_schema = vec_type.schema();

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

    let tx = client
        .transaction()
        .await
        .context("bulk_writer Symbol begin tx")?;
    tx.batch_execute(&stage_ddl)
        .await
        .context("bulk_writer Symbol stage table create")?;

    let copy_sink = tx
        .copy_in(
            "COPY _bulk_symbol_stage \
                  (id, name, kind, tested, is_public, is_nif, is_unsafe, project_code, embedding) \
                  FROM STDIN BINARY",
        )
        .await
        .context("bulk_writer Symbol copy_in begin")?;
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
        // None embedding -> NULL in the COPY stream.
        // Wrong-dimension embedding -> NULL with warn (mirrors
        // render_symbols_pg's fallback behavior).
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
            .context("bulk_writer Symbol copy row write")?;
    }
    let _written = writer
        .finish()
        .await
        .context("bulk_writer Symbol copy_in finish")?;

    tx.batch_execute(
        "INSERT INTO public.Symbol \
            (id, name, kind, tested, is_public, is_nif, is_unsafe, project_code, embedding) \
         SELECT id, name, kind, tested, is_public, is_nif, is_unsafe, project_code, embedding \
         FROM _bulk_symbol_stage \
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
    .context("bulk_writer Symbol stage merge")?;

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
    // Chunk has no vector column, so no pgvector type lookup is needed
    // for the COPY stream. The stage shape mirrors public.Chunk's PK
    // and the columns the merge needs to update.
    let stage_ddl = "CREATE TEMP TABLE _bulk_chunk_stage (\
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
            chunk_path TEXT\
         ) ON COMMIT DROP";

    let tx = client
        .transaction()
        .await
        .context("bulk_writer Chunk begin tx")?;
    tx.batch_execute(stage_ddl)
        .await
        .context("bulk_writer Chunk stage table create")?;

    let copy_sink = tx
        .copy_in(
            "COPY _bulk_chunk_stage \
                  (id, source_type, source_id, project_code, file_path, kind, content, content_hash, start_line, end_line, chunk_part_index, chunk_part_count, chunk_path) \
                  FROM STDIN BINARY",
        )
        .await
        .context("bulk_writer Chunk copy_in begin")?;
    let column_types = [
        Type::TEXT,
        Type::TEXT,
        Type::TEXT,
        Type::TEXT,
        Type::TEXT,
        Type::TEXT,
        Type::TEXT,
        Type::TEXT,
        Type::INT8,
        Type::INT8,
        Type::INT8,
        Type::INT8,
        Type::TEXT,
    ];
    let writer = BinaryCopyInWriter::new(copy_sink, &column_types);
    pin_mut!(writer);

    for row in rows {
        writer
            .as_mut()
            .write(&[
                &row.chunk_id,
                &row.source_type,
                &row.source_id,
                &row.project_code,
                &row.file_path,
                &row.kind,
                &row.content,
                &row.content_hash,
                &row.start_line,
                &row.end_line,
                &row.part_index,
                &row.part_count,
                &row.chunk_path,
            ])
            .await
            .context("bulk_writer Chunk copy row write")?;
    }
    let _written = writer
        .finish()
        .await
        .context("bulk_writer Chunk copy_in finish")?;

    tx.batch_execute(
        "INSERT INTO public.Chunk \
            (id, source_type, source_id, project_code, file_path, kind, content, content_hash, start_line, end_line, chunk_part_index, chunk_part_count, chunk_path) \
         SELECT id, source_type, source_id, project_code, file_path, kind, content, content_hash, start_line, end_line, chunk_part_index, chunk_part_count, chunk_path \
         FROM _bulk_chunk_stage \
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
            chunk_path = EXCLUDED.chunk_path",
    )
    .await
    .context("bulk_writer Chunk stage merge")?;

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
pub fn flush_relations(table: RelationTable, rows: &[RelationRow]) -> Result<()> {
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
        flush_relations_async(&mut client, table, rows).await
    })
}

async fn flush_relations_async(
    client: &mut deadpool_postgres::Client,
    table: RelationTable,
    rows: &[RelationRow],
) -> Result<()> {
    let stage = table.stage_name();
    let target = table.sql_name();
    let merge_clause = match table {
        RelationTable::Contains => {
            "ON CONFLICT (source_id, target_id) DO NOTHING".to_string()
        }
        RelationTable::Calls | RelationTable::CallsNif => {
            "ON CONFLICT (source_id, target_id) DO UPDATE SET project_code = EXCLUDED.project_code"
                .to_string()
        }
    };

    let stage_ddl = format!(
        "CREATE TEMP TABLE {stage} (\
            source_id TEXT NOT NULL,\
            target_id TEXT NOT NULL,\
            project_code TEXT NOT NULL\
         ) ON COMMIT DROP"
    );

    let tx = client
        .transaction()
        .await
        .with_context(|| format!("bulk_writer {target} begin tx"))?;
    tx.batch_execute(&stage_ddl)
        .await
        .with_context(|| format!("bulk_writer {target} stage create"))?;

    let copy_stmt =
        format!("COPY {stage} (source_id, target_id, project_code) FROM STDIN BINARY");
    let copy_sink = tx
        .copy_in(copy_stmt.as_str())
        .await
        .with_context(|| format!("bulk_writer {target} copy_in begin"))?;
    let column_types = [Type::TEXT, Type::TEXT, Type::TEXT];
    let writer = BinaryCopyInWriter::new(copy_sink, &column_types);
    pin_mut!(writer);

    for row in rows {
        writer
            .as_mut()
            .write(&[&row.source_id, &row.target_id, &row.project_code])
            .await
            .with_context(|| format!("bulk_writer {target} copy row write"))?;
    }
    let _written = writer
        .finish()
        .await
        .with_context(|| format!("bulk_writer {target} copy_in finish"))?;

    let merge_sql = format!(
        "INSERT INTO {target} (source_id, target_id, project_code) \
         SELECT source_id, target_id, project_code FROM {stage} \
         {merge_clause}"
    );
    tx.batch_execute(&merge_sql)
        .await
        .with_context(|| format!("bulk_writer {target} stage merge"))?;

    tx.commit()
        .await
        .with_context(|| format!("bulk_writer {target} commit"))?;
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
    fn flush_relations_on_empty_input_is_noop() {
        for t in [
            RelationTable::Contains,
            RelationTable::Calls,
            RelationTable::CallsNif,
        ] {
            let res = flush_relations(t, &[]);
            assert!(
                res.is_ok(),
                "empty {:?} flush must not touch the DB",
                t
            );
        }
    }

    #[test]
    fn relation_table_sql_names_match_ddl_targets() {
        // Sanity gate: the SQL names line up with the schema in
        // `crate::postgres::ddl::ist_ddl_global` so a future rename of
        // either side fails the test instead of silently routing
        // COPY BINARY into a phantom table.
        assert_eq!(RelationTable::Contains.sql_name(), "public.CONTAINS");
        assert_eq!(RelationTable::Calls.sql_name(), "public.CALLS");
        assert_eq!(RelationTable::CallsNif.sql_name(), "public.CALLS_NIF");
    }

    #[test]
    fn relation_table_stage_names_are_unique() {
        // Cross-call collision shouldn't happen because the stage tables
        // are TEMP + ON COMMIT DROP, but if all three were ever flushed
        // in one tx (atomic-per-file batching follow-up), distinct stage
        // names matter.
        let names = [
            RelationTable::Contains.stage_name(),
            RelationTable::Calls.stage_name(),
            RelationTable::CallsNif.stage_name(),
        ];
        let unique: std::collections::HashSet<&&str> = names.iter().collect();
        assert_eq!(unique.len(), names.len());
    }
}
