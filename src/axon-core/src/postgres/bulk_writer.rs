//! REQ-AXO-238: PostgreSQL bulk writer using COPY BINARY.
//!
//! Replaces the per-row `INSERT ... ON CONFLICT` SQL emitter on the
//! ChunkEmbedding hot path with a single COPY BINARY into a temp
//! staging table + a single `INSERT ... SELECT ... ON CONFLICT DO
//! UPDATE` merge. Per VAL-AXO-044 the writer mutex is the dominant
//! bottleneck under PG; bulk-loading 10K embeddings in one COPY
//! removes most of the per-row overhead.
//!
//! ## Two routing paths (REQ-AXO-901959)
//!
//! - **Live pipeline (canonical):** the A3/B3 hot path writes via
//!   `NativePgCtx::flush_batch_copy` / `flush_chunk_embeddings_copy`
//!   (postgres/native.rs), which acquire a client from the GRAPHSTORE'S OWN
//!   native pool and call the client-based `flush_*_async` cores here. The
//!   pool is lifetime-scoped to the store, so the COPY lands in the same DB
//!   the store reads from (correct under per-test isolation) and is dropped
//!   with the store (no connection leak). This is the only path the indexer
//!   uses; it never touches the `static POOL` below.
//! - **Standalone (bench + integration tests):** the sync `flush_*`
//!   entrypoints drive a process-global `OnceLock<Runtime>` + `OnceLock<Pool>`
//!   resolved once from the env (`AXON_LIVE_DATABASE_URL` → `AXON_DEV_…` →
//!   `DATABASE_URL`, via `postgres::resolve_database_url`). Used only by
//!   `axon-bench-writer` (sets its own URL) and the `tests/pg_bulk_writer*.rs`
//!   integration tests (single shared test DB), which legitimately have no
//!   GraphStore. NOT the live write path — do not route store writes here.
//!
//! Surface (sync, standalone):
//! - [`bulk_writer_enabled`]: reads `AXON_BULK_WRITER_ENABLED`.
//! - [`flush_chunk_embeddings`] / [`flush_symbols`] / [`flush_chunks`] /
//!   [`flush_batch`]: block the caller until the COPY + merge transaction
//!   commits, over the global `OnceLock` pool described above.
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

use crate::graph_ingestion::rows::{ChunkEmbeddingPersistRow, ChunkRow, RelationRow, SymbolRow};

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

/// REQ-AXO-902198 residual — process-global count of rows dropped by ANY of the
/// bisection paths below (chunks / symbols / edges / indexed_files / chunk_embeddings)
/// since process start. Surfaced read-only via `poison_rows_dropped_count` so an MCP
/// tool (`embedding_status`) can expose it — a bisection drop is silent recovery
/// (the batch lands, the drain never freezes), which is exactly the kind of fact an
/// operator needs visibility into, not just a `log::warn!` line.
static POISON_ROWS_DROPPED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Total rows dropped via poison-row bisection since process start (REQ-AXO-902198).
pub fn poison_rows_dropped_count() -> u64 {
    POISON_ROWS_DROPPED.load(std::sync::atomic::Ordering::Relaxed)
}

/// Tri-state override of the adaptive COPY dispatch (REQ-AXO-901881 W3 #34,
/// VAL-AXO-067). `Some(true)` = force COPY BINARY for every flush (bench /
/// explicit opt-in); `Some(false)` = force per-row INSERT for every flush;
/// `None` (unset) = adaptive — COPY only when the batch is large enough to
/// amortise its fixed setup cost (see [`should_use_bulk_writer`]).
pub fn bulk_writer_override() -> Option<bool> {
    std::env::var("AXON_BULK_WRITER_ENABLED").ok().map(|v| {
        matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
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
    if rows.is_empty() {
        return Ok(());
    }
    // Idempotent guard: ensure pgvector's `vector` type is reachable
    // for this session. The bulk_writer pool is independent from the
    // FFI plugin pool. We run CREATE EXTENSION + the type lookup +
    // the search_path adjustment OUTSIDE the bulk transaction, ONCE for
    // the whole call (not repeated per bisection probe below).
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

    // REQ-AXO-902198 residual — happy path: the whole batch in one tx (unchanged
    // cost). On a DATA poison, bisect so one bad embedding row can't freeze the
    // whole batch (this was the most exposed of the 5 original COPY sites: it had
    // neither a NUL pre-filter nor bisection before this residual slice).
    match copy_chunk_embeddings_tx(client, project_code, model_id, rows, embedded_at_ms, vec_type.clone()).await {
        Ok(()) => return Ok(()),
        Err(e) => match pg_sqlstate(&e).as_deref().map(classify_copy_error) {
            Some(CopyErrorClass::Other) | None => return Err(e),
            Some(CopyErrorClass::Transient) | Some(CopyErrorClass::Data) => {}
        },
    }
    let mut planner = BisectPlanner::new(rows.len());
    while let Some(range) = planner.next() {
        let mut tries = 0u32;
        loop {
            match copy_chunk_embeddings_tx(
                client,
                project_code,
                model_id,
                &rows[range.clone()],
                embedded_at_ms,
                vec_type.clone(),
            )
            .await
            {
                Ok(()) => break,
                Err(e) => match pg_sqlstate(&e).as_deref().map(classify_copy_error) {
                    Some(CopyErrorClass::Transient) if tries < BISECT_MAX_TRANSIENT_RETRIES => {
                        tries += 1;
                        continue;
                    }
                    Some(CopyErrorClass::Data) => {
                        planner.on_data(range.clone());
                        break;
                    }
                    _ => return Err(e),
                },
            }
        }
    }
    let poison = planner.into_poison();
    if !poison.is_empty() {
        let dropped: Vec<&str> = poison.iter().map(|&i| rows[i].chunk_id.as_str()).collect();
        log::warn!(
            "flush_chunk_embeddings: dropped {} poison embedding row(s) via bisection so the batch could land (REQ-AXO-902198): {:?}",
            poison.len(),
            dropped
        );
        POISON_ROWS_DROPPED.fetch_add(poison.len() as u64, std::sync::atomic::Ordering::Relaxed);
    }
    Ok(())
}

/// One ChunkEmbedding COPY attempt in its OWN transaction — the bisection probe.
async fn copy_chunk_embeddings_tx(
    client: &mut deadpool_postgres::Client,
    project_code: &str,
    model_id: &str,
    rows: &[ChunkEmbeddingPersistRow],
    embedded_at_ms: i64,
    vec_type: Type,
) -> Result<()> {
    let vec_schema = vec_type.schema().to_string();

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
    let tx = client.transaction().await.context("bulk_writer begin tx")?;
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
        // REQ-AXO-902198 residual (pre-filter) — strip NUL like the other 4 COPY
        // sites; a 0x00 in an id/hash field would abort the whole batch at
        // finish() before bisection even gets a chance to isolate it.
        let chunk_id = strip_nul(&row.chunk_id);
        let model_id_s = strip_nul(model_id);
        let pcode = strip_nul(project_code);
        let source_hash = strip_nul(&row.source_hash);
        writer
            .as_mut()
            .write(&[
                &chunk_id as &(dyn tokio_postgres::types::ToSql + Sync),
                &model_id_s,
                &pcode,
                &source_hash,
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

// ── REQ-AXO-902198 — poison-pill bisection (pure decision core) ───────────────
// Generalises the 902197 NUL-strip: instead of sanitising ONE known class (0x00),
// isolate ANY poison row so a single bad row can never freeze the whole COPY batch
// (the frozen-drain incident). Split into a PURE decision core (classification +
// bisection planner — unit-tested without a live DB, practice-128 decision/driver
// split) and a thin async I/O driver per table. `BisectPlanner`/`classify_copy_error`
// are the one chokepoint every resilient flush below drives (`flush_chunks_async`,
// `flush_symbols_resilient_async`, `flush_edges_resilient_async`,
// `flush_indexed_files_resilient_async`, `flush_chunk_embeddings_async`) — a fully
// generic combinator over an async closure was considered (the true DRY endpoint,
// per the operator's "5 near-identical COPY sites" observation) but rejected: each
// table's happy-path fn already opens its OWN transaction and captures different
// borrowed state (vec_type, project_code for the FK-ordered purge), and threading
// that through a generic `Fn(&mut Client, &[T]) -> BoxFuture<Result<()>>` adds real
// async-lifetime/boxing complexity for a 5-call-site win — the decision CORE is
// already shared (no duplicated classification/bisection logic), only the thin
// per-table I/O driver repeats, which mirrors how flush_chunks_async was already
// built. Cross-table drain (`flush_batch_async`) routes the Data-failure fallback
// through IndexedFile(+purge) → Symbol → Chunk → Edge, in FK-parent-first order,
// instead of retrying the whole structural core atomically (which only helped when
// the poison happened to be in chunks).

/// How a failed COPY should be reacted to, derived from the PG SQLSTATE.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CopyErrorClass {
    /// Serialization / deadlock / lock-timeout / connection — the DATA is fine; retry.
    Transient,
    /// The row itself is bad (bad byte, constraint) — bisect to isolate the poison.
    Data,
    /// Anything else (syntax, permission, …) — not a poison row; propagate unchanged.
    Other,
}

/// Classify a PG SQLSTATE for the bisection. Frozen list (REQ-AXO-902198):
/// transient = 40001/40P01 (serialization/deadlock), 55P03 (lock_not_available),
/// 57014 (query_canceled/statement_timeout), 08xxx (connection); data = the 22xxx
/// (data exception, incl. 22021 invalid byte / 22P02 text-repr / COPY errors) and
/// 23xxx (integrity constraint: FK/unique/check/not-null) families.
fn classify_copy_error(sqlstate: &str) -> CopyErrorClass {
    match sqlstate {
        "40001" | "40P01" | "55P03" | "57014" => CopyErrorClass::Transient,
        s if s.starts_with("08") => CopyErrorClass::Transient,
        s if s.starts_with("22") || s.starts_with("23") => CopyErrorClass::Data,
        _ => CopyErrorClass::Other,
    }
}

/// Extract the PG SQLSTATE from an anyhow error chain (the tokio_postgres::Error is
/// wrapped via `.context(...)`), if any.
fn pg_sqlstate(err: &anyhow::Error) -> Option<String> {
    err.chain()
        .find_map(|e| e.downcast_ref::<tokio_postgres::Error>())
        .and_then(|e| e.code())
        .map(|c| c.code().to_string())
}

/// Pure bisection planner: a work-stack of row ranges to probe + the isolated poison
/// indices. The async driver pops a range (`next`), probes that slice's COPY in its own
/// tx, and reports a DATA failure (`on_data`, which splits or records a singleton poison)
/// or does nothing on success. O(k·log n) probes for k poison rows in n; the happy path
/// (probe(0..n)==Ok) is a single probe with zero drops.
#[derive(Debug)]
struct BisectPlanner {
    stack: Vec<std::ops::Range<usize>>,
    poison: Vec<usize>,
}

impl BisectPlanner {
    fn new(total: usize) -> Self {
        BisectPlanner {
            stack: if total == 0 { Vec::new() } else { vec![0..total] },
            poison: Vec::new(),
        }
    }

    fn next(&mut self) -> Option<std::ops::Range<usize>> {
        self.stack.pop()
    }

    /// The COPY of `range` failed with a DATA error: a singleton IS the poison row;
    /// otherwise split in half (push hi then lo so lo is probed first — deterministic).
    fn on_data(&mut self, range: std::ops::Range<usize>) {
        if range.len() <= 1 {
            if range.len() == 1 {
                self.poison.push(range.start);
            }
            return;
        }
        let mid = range.start + range.len() / 2;
        self.stack.push(mid..range.end);
        self.stack.push(range.start..mid);
    }

    fn into_poison(mut self) -> Vec<usize> {
        self.poison.sort_unstable();
        self.poison
    }
}

/// One chunk COPY attempt in its OWN transaction (rollback-isolated) — the bisection probe.
async fn copy_chunks_tx(client: &mut deadpool_postgres::Client, rows: &[ChunkRow]) -> Result<()> {
    let tx = client
        .transaction()
        .await
        .context("bulk_writer Chunk begin tx")?;
    copy_chunks_in_tx(&tx, rows).await?;
    tx.commit().await.context("bulk_writer Chunk commit")?;
    Ok(())
}

/// Max same-slice retries on a TRANSIENT fault before propagating (a persistent
/// lock/deadlock is NOT a poison row — never silently drop it).
const BISECT_MAX_TRANSIENT_RETRIES: u32 = 3;

async fn flush_chunks_async(
    client: &mut deadpool_postgres::Client,
    rows: &[ChunkRow],
) -> Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    // Happy path: the whole batch in one tx (unchanged cost — one probe).
    match copy_chunks_tx(client, rows).await {
        Ok(()) => return Ok(()),
        Err(e) => match pg_sqlstate(&e).as_deref().map(classify_copy_error) {
            // Not a poison row (syntax/permission/unknown) → propagate unchanged.
            Some(CopyErrorClass::Other) | None => return Err(e),
            // Transient or Data → fall through to the resilient bisection loop.
            Some(CopyErrorClass::Transient) | Some(CopyErrorClass::Data) => {}
        },
    }
    // Resilient path (REQ-AXO-902198): isolate poison rows by bisection; each
    // (sub-)batch in its own tx; transient faults retry the same slice.
    let mut planner = BisectPlanner::new(rows.len());
    while let Some(range) = planner.next() {
        let mut tries = 0u32;
        loop {
            match copy_chunks_tx(client, &rows[range.clone()]).await {
                Ok(()) => break, // this slice landed
                Err(e) => match pg_sqlstate(&e).as_deref().map(classify_copy_error) {
                    Some(CopyErrorClass::Transient) if tries < BISECT_MAX_TRANSIENT_RETRIES => {
                        tries += 1;
                        continue;
                    }
                    Some(CopyErrorClass::Data) => {
                        planner.on_data(range.clone());
                        break;
                    }
                    // Other, or a transient that outlasted the retries → propagate
                    // (never silently drop a row that isn't provably poison).
                    _ => return Err(e),
                },
            }
        }
    }
    let poison = planner.into_poison();
    if !poison.is_empty() {
        let dropped: Vec<&str> = poison.iter().map(|&i| rows[i].chunk_id.as_str()).collect();
        log::warn!(
            "flush_chunks: dropped {} poison chunk row(s) via bisection so the batch could land (REQ-AXO-902198): {:?}",
            poison.len(),
            dropped
        );
        POISON_ROWS_DROPPED.fetch_add(poison.len() as u64, std::sync::atomic::Ordering::Relaxed);
    }
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
    /// REQ-AXO-901493 — every OTHER parser-emitted, `relation_table`-mapped
    /// edge kind (IMPLEMENTS / IMPORTS / USES / EXTENDS / READS / DECLARES /
    /// EXPOSES / TESTS), carried generically as `(relation_type, row)`. The A3
    /// write path used to match only CALLS/CALLS_NIF and drop the rest
    /// (`_ => {}`), so the IST graph held 0 of these edge classes. The COPY
    /// merge (`copy_edges_in_tx`) is already relation-type-generic.
    pub other_edges: Vec<(String, RelationRow)>,
    pub indexed_files: Vec<(String, String, i64, i64, i64)>,
    /// REQ-AXO-901860 — the single project_code this batch belongs to
    /// (A3 groups by resolved project_code, one group per flush). The
    /// writer uses it to UPSERT the `axon.Project` FK parent first and to
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
            && self.other_edges.is_empty()
            && self.indexed_files.is_empty()
    }

    pub fn row_count(&self) -> usize {
        self.symbols.len()
            + self.chunks.len()
            + self.contains.len()
            + self.calls.len()
            + self.calls_nif.len()
            + self.other_edges.len()
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

// REQ-AXO-901959 — exposed to the GraphStore's native ctx so the live graph
// write routes through the STORE's own pool (correct DB + lifetime-scoped),
// not this module's global env-resolved `POOL`. Mirrors the embedding half
// already closed by `NativePgCtx::flush_chunk_embeddings_copy`.
// REQ-AXO-901959 — exposed to the GraphStore's native ctx. REQ-AXO-902198 — the live drain
// entrypoint: try the fast atomic path; on a DATA poison (a bad row PG rejects), fall back to
// a resilient mode that ISOLATES the poison so the batch still lands and the indexer drain
// never freezes (the 902197 incident class, generalised). NUL is already stripped from every
// free-text field pre-COPY (defense line 1); this is the backstop (defense line 2) for any
// other poison (encoding / constraint). Transient faults propagate — the FVQ retry contract
// re-drives the file.
pub(crate) async fn flush_batch_async(
    client: &mut deadpool_postgres::Client,
    batch: &PgBulkBatch,
) -> Result<()> {
    match flush_batch_tx(client, batch).await {
        Ok(()) => Ok(()),
        Err(e) => match pg_sqlstate(&e).as_deref().map(classify_copy_error) {
            Some(CopyErrorClass::Data) => {
                log::warn!(
                    "bulk_writer batch atomic flush hit a DATA poison ({e:#}); retrying table-by-table with bisection so one bad row can't freeze the drain (REQ-AXO-902198)"
                );
                // REQ-AXO-902198 residual — the atomic structural-core retry (a second
                // all-or-nothing IndexedFile+Symbol+Edge attempt) only ever helped when the
                // poison happened to live in `chunks` (the one table it excluded); a poison
                // Symbol/Edge/IndexedFile row would fail identically the second time and
                // propagate with the whole batch dropped. Route each table through its OWN
                // bisected resilient flush instead, in FK-parent-first order: IndexedFile
                // (Chunk's FK parent) → Symbol → Chunk → Edge (last, since edges reference
                // symbol/file ids). Every clean row across every table lands; only the
                // provably-poisoned ones are dropped (and counted, see poison_rows_dropped).
                if !batch.project_code.is_empty() {
                    let now_ms = chrono::Utc::now().timestamp_millis();
                    client
                        .execute(
                            "INSERT INTO axon.Project (code, enrolled_at_ms) VALUES ($1, $2) \
                             ON CONFLICT (code) DO NOTHING",
                            &[&batch.project_code, &now_ms],
                        )
                        .await
                        .context("bulk_writer resilient ensure axon.Project FK parent")?;
                }
                if !batch.indexed_files.is_empty() {
                    flush_indexed_files_resilient_async(client, &batch.indexed_files, &batch.project_code).await?;
                }
                if !batch.symbols.is_empty() {
                    let vec_type = vector_type(client).await?;
                    flush_symbols_resilient_async(client, &batch.symbols, vec_type).await?;
                }
                if !batch.chunks.is_empty() {
                    flush_chunks_async(client, &batch.chunks).await?;
                }
                let has_edges = !batch.contains.is_empty()
                    || !batch.calls.is_empty()
                    || !batch.calls_nif.is_empty()
                    || !batch.other_edges.is_empty();
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
                    for (rel, r) in &batch.other_edges {
                        edge_rows.push((rel.as_str(), r));
                    }
                    flush_edges_resilient_async(client, &edge_rows).await?;
                }
                Ok(())
            }
            // Transient (lock/deadlock/connection) or Other → propagate unchanged.
            _ => Err(e),
        },
    }
}

/// The batch flush transaction — the happy path, everything in one atomic tx. On a
/// DATA-poison failure, `flush_batch_async` falls back to the table-by-table resilient
/// flushes below instead of retrying this function (REQ-AXO-902198 residual).
async fn flush_batch_tx(
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
    // axon.Project; Chunk additionally FKs ist.IndexedFile(path). A file that
    // reaches A3 before the scanner enrolled it (the bootstrap walk feeds A1
    // directly) used to fail the Symbol/Chunk insert with a FK violation,
    // aborting the tx and poisoning the pooled connection — a 25P02 cascade
    // that blocked embeddings, the heartbeat UPSERT, and dashboard_state for
    // every project sharing the connection. The writer now owns its FK
    // parents: ensure axon.Project, then ist.IndexedFile, before the children.
    if !batch.project_code.is_empty() {
        let now_ms = chrono::Utc::now().timestamp_millis();
        tx.execute(
            "INSERT INTO axon.Project (code, enrolled_at_ms) VALUES ($1, $2) \
             ON CONFLICT (code) DO NOTHING",
            &[&batch.project_code, &now_ms],
        )
        .await
        .context("bulk_writer batch ensure axon.Project FK parent")?;
    }
    if !batch.indexed_files.is_empty() {
        // REQ-AXO-902011 — re-index-safe purge BEFORE the COPY merge, same tx:
        // an edited-in-place file (renamed/removed symbol, fewer chunk parts)
        // must not leave orphan Symbol/Chunk/Edge/ChunkEmbedding behind.
        purge_reindexed_files_in_tx(&tx, &batch.indexed_files, &batch.project_code).await?;
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
        || !batch.calls_nif.is_empty()
        || !batch.other_edges.is_empty();
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
        // REQ-AXO-901493 — generic mapped edges (IMPLEMENTS/IMPORTS/USES/...).
        for (rel, r) in &batch.other_edges {
            edge_rows.push((rel.as_str(), r));
        }
        copy_edges_in_tx(&tx, &edge_rows).await?;
    }

    tx.commit().await.context("bulk_writer batch commit")?;
    Ok(())
}

/// REQ-AXO-902011 — purge a re-indexed file's OWNED graph rows inside the bulk
/// tx so editing a file in place (renamed/removed symbol, fewer chunk parts)
/// leaves no orphan rows. Owned = its Chunks (by `file_path`), their
/// ChunkEmbeddings, this file's Symbols (CONTAINS targets) and OUTBOUND edges
/// (`source_id = path`). Deliberately NOT deleted: inbound edges
/// (`target_id IN <this file's symbols>` — owned by CALLER files absent from
/// this batch, so deleting them would lose valid edges on every re-index) and
/// IndexedFile (re-UPSERTed by `copy_indexed_files_in_tx` right after).
/// `project_code` is included on the Chunk predicates so the delete rides
/// `chunk_project_file_idx (project_code, file_path)` instead of a seq-scan on
/// the hot write path; the Edge/Symbol deletes ride `edge_fwd_idx (source_id…)`.
async fn purge_reindexed_files_in_tx(
    tx: &deadpool_postgres::Transaction<'_>,
    indexed_files: &[(String, String, i64, i64, i64)],
    project_code: &str,
) -> Result<()> {
    for (path, ..) in indexed_files {
        let chunk_params: [&(dyn tokio_postgres::types::ToSql + Sync); 2] = [path, &project_code];
        tx.execute(
            "DELETE FROM ist.ChunkEmbedding WHERE chunk_id IN \
                 (SELECT id FROM ist.Chunk WHERE project_code = $2 AND file_path = $1)",
            &chunk_params,
        )
        .await
        .context("purge_reindexed: ChunkEmbedding")?;
        tx.execute(
            "DELETE FROM ist.Chunk WHERE project_code = $2 AND file_path = $1",
            &chunk_params,
        )
        .await
        .context("purge_reindexed: Chunk")?;
        let path_params: [&(dyn tokio_postgres::types::ToSql + Sync); 1] = [path];
        tx.execute(
            "DELETE FROM ist.Symbol WHERE id IN \
                 (SELECT target_id FROM ist.Edge WHERE source_id = $1 \
                  AND relation_type = 'CONTAINS')",
            &path_params,
        )
        .await
        .context("purge_reindexed: Symbol")?;
        // REQ-AXO-902204 — also purge the OUTBOUND calls of THIS file's symbols. A CALLS edge
        // is sourced from the CALLER's symbol id (`…file.rs::method`), NOT the file path, so the
        // `source_id = path` delete below never reached them: a call REMOVED from the code left a
        // stale edge surviving every re-index. Delete edges sourced from any symbol this file
        // CONTAINS (the CONTAINS edges still exist here — the path-sourced delete runs next), so
        // re-parse re-writes a fresh outbound call set. Inbound edges (owned by caller files) stay.
        tx.execute(
            "DELETE FROM ist.Edge WHERE source_id IN \
                 (SELECT target_id FROM ist.Edge WHERE source_id = $1 \
                  AND relation_type = 'CONTAINS')",
            &path_params,
        )
        .await
        .context("purge_reindexed: outbound edges of file's symbols (REQ-AXO-902204)")?;
        tx.execute(
            "DELETE FROM ist.Edge WHERE source_id = $1",
            &path_params,
        )
        .await
        .context("purge_reindexed: Edge")?;
    }
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
            embedding {schema}.vector({dim}),\
            cyclomatic_complexity INTEGER\
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
                  (id, name, kind, tested, is_public, is_nif, is_unsafe, project_code, embedding, cyclomatic_complexity) \
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
        Type::INT4,
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
        // REQ-AXO-902198 (pre-filter) — strip NUL from every free-text field, like chunks
        // (902197). A 0x00 in a symbol id/name/kind (parser artifact) would abort the whole
        // COPY BINARY batch at finish() → freeze the indexer drain (files stuck 'discovered').
        let id = strip_nul(&row.symbol_id);
        let name = strip_nul(&row.name);
        let kind = strip_nul(&row.kind);
        let pcode = strip_nul(&row.project_code);
        writer
            .as_mut()
            .write(&[
                &id as &(dyn tokio_postgres::types::ToSql + Sync),
                &name,
                &kind,
                &row.tested,
                &row.is_public,
                &row.is_nif,
                &row.is_unsafe,
                &pcode,
                &embed_opt,
                &row.cyclomatic_complexity,
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
            (id, name, kind, tested, is_public, is_nif, is_unsafe, project_code, embedding, cyclomatic_complexity) \
         SELECT DISTINCT ON (id) \
                id, name, kind, tested, is_public, is_nif, is_unsafe, project_code, embedding, cyclomatic_complexity \
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
            embedding = EXCLUDED.embedding, \
            cyclomatic_complexity = EXCLUDED.cyclomatic_complexity",
    )
    .await
    .context("bulk_writer Symbol stage merge (batch)")?;
    Ok(())
}

/// One Symbol COPY attempt in its OWN transaction — the bisection probe.
async fn copy_symbols_tx(
    client: &mut deadpool_postgres::Client,
    rows: &[SymbolRow],
    vec_type: Type,
) -> Result<()> {
    let tx = client
        .transaction()
        .await
        .context("bulk_writer Symbol begin tx (resilient)")?;
    copy_symbols_in_tx(&tx, rows, vec_type).await?;
    tx.commit().await.context("bulk_writer Symbol commit (resilient)")?;
    Ok(())
}

/// REQ-AXO-902198 residual — Symbol counterpart of `flush_chunks_async`: try the
/// whole batch in one tx (happy path, zero overhead); on a DATA poison, bisect to
/// isolate and drop exactly the poisoned row(s) so every clean symbol still lands.
async fn flush_symbols_resilient_async(
    client: &mut deadpool_postgres::Client,
    rows: &[SymbolRow],
    vec_type: Type,
) -> Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    match copy_symbols_tx(client, rows, vec_type.clone()).await {
        Ok(()) => return Ok(()),
        Err(e) => match pg_sqlstate(&e).as_deref().map(classify_copy_error) {
            Some(CopyErrorClass::Other) | None => return Err(e),
            Some(CopyErrorClass::Transient) | Some(CopyErrorClass::Data) => {}
        },
    }
    let mut planner = BisectPlanner::new(rows.len());
    while let Some(range) = planner.next() {
        let mut tries = 0u32;
        loop {
            match copy_symbols_tx(client, &rows[range.clone()], vec_type.clone()).await {
                Ok(()) => break,
                Err(e) => match pg_sqlstate(&e).as_deref().map(classify_copy_error) {
                    Some(CopyErrorClass::Transient) if tries < BISECT_MAX_TRANSIENT_RETRIES => {
                        tries += 1;
                        continue;
                    }
                    Some(CopyErrorClass::Data) => {
                        planner.on_data(range.clone());
                        break;
                    }
                    _ => return Err(e),
                },
            }
        }
    }
    let poison = planner.into_poison();
    if !poison.is_empty() {
        let dropped: Vec<&str> = poison.iter().map(|&i| rows[i].symbol_id.as_str()).collect();
        log::warn!(
            "flush_symbols: dropped {} poison symbol row(s) via bisection so the batch could land (REQ-AXO-902198): {:?}",
            poison.len(),
            dropped
        );
        POISON_ROWS_DROPPED.fetch_add(poison.len() as u64, std::sync::atomic::Ordering::Relaxed);
    }
    Ok(())
}

/// REQ-AXO-902197 — PostgreSQL `text` columns reject NUL (`0x00`) with
/// "invalid byte sequence for encoding UTF8". A single chunk carrying a `0x00`
/// (a parser artifact or a fused-chunk boundary) makes the WHOLE `COPY … BINARY`
/// batch abort at `finish()`, which freezes the indexer drain — files never
/// leave `discovered`. Strip NULs so one bad chunk can't poison the batch.
/// Cheap: borrows the input unchanged (no allocation) when it is clean (the norm).
fn strip_nul(s: &str) -> std::borrow::Cow<'_, str> {
    if s.as_bytes().contains(&0) {
        std::borrow::Cow::Owned(s.replace('\0', ""))
    } else {
        std::borrow::Cow::Borrowed(s)
    }
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
        Type::INT4,
    ];
    let writer = BinaryCopyInWriter::new(copy_sink, &column_types);
    pin_mut!(writer);
    for row in rows {
        let tc: Option<i32> = row.token_count.map(|v| v as i32);
        // REQ-AXO-902197 — strip NUL from every free-text field before the COPY;
        // PG rejects 0x00 and a single bad chunk aborts the whole batch (drain freeze).
        let chunk_id = strip_nul(&row.chunk_id);
        let source_id = strip_nul(&row.source_id);
        let file_path = strip_nul(&row.file_path);
        let kind = strip_nul(&row.kind);
        let content = strip_nul(&row.content);
        let content_hash = strip_nul(&row.content_hash);
        let chunk_path = strip_nul(&row.chunk_path);
        writer
            .as_mut()
            .write(&[
                &chunk_id as &(dyn tokio_postgres::types::ToSql + Sync),
                &row.source_type,
                &source_id,
                &row.project_code,
                &file_path,
                &kind,
                &content,
                &content_hash,
                &row.start_line,
                &row.end_line,
                &row.part_index,
                &row.part_count,
                &chunk_path,
                &tc,
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
        // REQ-AXO-902198 (pre-filter) — strip NUL from the parser-derived id fields; a 0x00
        // would abort the whole edge COPY batch → freeze the drain (rel_type is controlled).
        let source_id = strip_nul(&row.source_id);
        let target_id = strip_nul(&row.target_id);
        let pcode = strip_nul(&row.project_code);
        let rel = rel_type.to_string();
        writer
            .as_mut()
            .write(&[
                &source_id as &(dyn tokio_postgres::types::ToSql + Sync),
                &target_id,
                &rel,
                &pcode,
                &now_ms,
            ])
            .await
            .context("bulk_writer edge copy row write")?;
    }
    writer
        .finish()
        .await
        .context("bulk_writer edge copy_in finish")?;

    let merge_sql = "INSERT INTO ist.edge (source_id, target_id, relation_type, project_code, created_at_ms) \
         SELECT source_id, target_id, relation_type, project_code, created_at_ms FROM _bulk_edge_stage \
         ON CONFLICT (source_id, target_id, relation_type, project_code) DO NOTHING";
    tx.batch_execute(merge_sql)
        .await
        .context("bulk_writer edge stage merge")?;
    Ok(())
}

/// One Edge COPY attempt in its OWN transaction — the bisection probe.
async fn copy_edges_tx(
    client: &mut deadpool_postgres::Client,
    rows: &[(&str, &RelationRow)],
) -> Result<()> {
    let tx = client
        .transaction()
        .await
        .context("bulk_writer edge begin tx (resilient)")?;
    copy_edges_in_tx(&tx, rows).await?;
    tx.commit().await.context("bulk_writer edge commit (resilient)")?;
    Ok(())
}

/// REQ-AXO-902198 residual — Edge counterpart of `flush_chunks_async`. Edges carry no
/// FK to Symbol/IndexedFile (a dangling edge is tolerated — see `copy_edges_in_tx`'s
/// `ON CONFLICT DO NOTHING`), so bisecting them independently of Symbol/IndexedFile is
/// safe; the only ordering requirement (edges land AFTER their endpoints exist so a
/// fresh IST read sees a consistent graph) is preserved by `flush_batch_async` calling
/// this LAST.
async fn flush_edges_resilient_async(
    client: &mut deadpool_postgres::Client,
    rows: &[(&str, &RelationRow)],
) -> Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    match copy_edges_tx(client, rows).await {
        Ok(()) => return Ok(()),
        Err(e) => match pg_sqlstate(&e).as_deref().map(classify_copy_error) {
            Some(CopyErrorClass::Other) | None => return Err(e),
            Some(CopyErrorClass::Transient) | Some(CopyErrorClass::Data) => {}
        },
    }
    let mut planner = BisectPlanner::new(rows.len());
    while let Some(range) = planner.next() {
        let mut tries = 0u32;
        loop {
            match copy_edges_tx(client, &rows[range.clone()]).await {
                Ok(()) => break,
                Err(e) => match pg_sqlstate(&e).as_deref().map(classify_copy_error) {
                    Some(CopyErrorClass::Transient) if tries < BISECT_MAX_TRANSIENT_RETRIES => {
                        tries += 1;
                        continue;
                    }
                    Some(CopyErrorClass::Data) => {
                        planner.on_data(range.clone());
                        break;
                    }
                    _ => return Err(e),
                },
            }
        }
    }
    let poison = planner.into_poison();
    if !poison.is_empty() {
        let dropped: Vec<String> = poison
            .iter()
            .map(|&i| format!("{}->{}", rows[i].1.source_id, rows[i].1.target_id))
            .collect();
        log::warn!(
            "flush_edges: dropped {} poison edge row(s) via bisection so the batch could land (REQ-AXO-902198): {:?}",
            poison.len(),
            dropped
        );
        POISON_ROWS_DROPPED.fetch_add(poison.len() as u64, std::sync::atomic::Ordering::Relaxed);
    }
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
        // REQ-AXO-902198 (pre-filter) — strip NUL from path/hash so a 0x00 can't abort the
        // IndexedFile COPY batch → freeze the drain.
        let path_s = strip_nul(path);
        let hash_s = strip_nul(hash);
        writer
            .as_mut()
            .write(&[
                &path_s as &(dyn tokio_postgres::types::ToSql + Sync),
                &hash_s,
                ts,
                mtime,
                size,
            ])
            .await
            .context("bulk_writer indexedfile copy row write")?;
    }
    writer
        .finish()
        .await
        .context("bulk_writer indexedfile copy_in finish")?;

    // REQ-AXO-901860: project_code is a NOT NULL FK to axon.Project. A3 owns
    // its FK parents (the axon.Project row is UPSERTed first in
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
    // REQ-AXO-901897 (DBQ slice 1) — A3 batch hot path stamps 'parsed' (A-graph
    // done), not legacy 'indexed'. 'parsed' is an A-DONE state: it leaves the
    // claimable index/feeder set and feeds the dedup cache at next boot. The
    // claim lease is released (lease_until_ms=0).
    let merge_sql = "INSERT INTO indexedfile \
             (path, project_code, content_hash, last_seen_ms, status, mtime_ms, size_bytes, lease_until_ms) \
         SELECT DISTINCT ON (s.path) \
                s.path, $1, s.content_hash, s.last_seen_ms, 'parsed', s.mtime_ms, s.size_bytes, 0 \
             FROM _bulk_indexedfile_stage s \
         ORDER BY s.path, s.last_seen_ms DESC \
         ON CONFLICT (path) DO UPDATE SET \
             content_hash    = EXCLUDED.content_hash, \
             last_seen_ms    = EXCLUDED.last_seen_ms, \
             mtime_ms        = EXCLUDED.mtime_ms, \
             size_bytes      = EXCLUDED.size_bytes, \
             status          = 'parsed', \
             retry_count     = 0, \
             last_attempt_ms = NULL, \
             lease_until_ms  = 0";
    tx.execute(merge_sql, &[&project_code])
        .await
        .context("bulk_writer indexedfile stage merge")?;
    Ok(())
}

/// One (purge + IndexedFile COPY) attempt in its OWN transaction — the bisection
/// probe. Purge and copy MUST stay paired per slice: `purge_reindexed_files_in_tx`
/// deletes THIS slice's owned Chunk/Symbol/Edge rows before its IndexedFile row is
/// re-upserted, exactly like the original single-tx `flush_batch_tx` did for the
/// whole batch.
async fn copy_indexed_files_tx(
    client: &mut deadpool_postgres::Client,
    rows: &[(String, String, i64, i64, i64)],
    project_code: &str,
) -> Result<()> {
    let tx = client
        .transaction()
        .await
        .context("bulk_writer indexedfile begin tx (resilient)")?;
    purge_reindexed_files_in_tx(&tx, rows, project_code).await?;
    copy_indexed_files_in_tx(&tx, rows, project_code).await?;
    tx.commit().await.context("bulk_writer indexedfile commit (resilient)")?;
    Ok(())
}

/// REQ-AXO-902198 residual — IndexedFile counterpart of `flush_chunks_async`. Lands
/// FIRST in `flush_batch_async`'s resilient fallback (FK parent for Chunk).
async fn flush_indexed_files_resilient_async(
    client: &mut deadpool_postgres::Client,
    rows: &[(String, String, i64, i64, i64)],
    project_code: &str,
) -> Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    match copy_indexed_files_tx(client, rows, project_code).await {
        Ok(()) => return Ok(()),
        Err(e) => match pg_sqlstate(&e).as_deref().map(classify_copy_error) {
            Some(CopyErrorClass::Other) | None => return Err(e),
            Some(CopyErrorClass::Transient) | Some(CopyErrorClass::Data) => {}
        },
    }
    let mut planner = BisectPlanner::new(rows.len());
    while let Some(range) = planner.next() {
        let mut tries = 0u32;
        loop {
            match copy_indexed_files_tx(client, &rows[range.clone()], project_code).await {
                Ok(()) => break,
                Err(e) => match pg_sqlstate(&e).as_deref().map(classify_copy_error) {
                    Some(CopyErrorClass::Transient) if tries < BISECT_MAX_TRANSIENT_RETRIES => {
                        tries += 1;
                        continue;
                    }
                    Some(CopyErrorClass::Data) => {
                        planner.on_data(range.clone());
                        break;
                    }
                    _ => return Err(e),
                },
            }
        }
    }
    let poison = planner.into_poison();
    if !poison.is_empty() {
        let dropped: Vec<&str> = poison.iter().map(|&i| rows[i].0.as_str()).collect();
        log::warn!(
            "flush_indexed_files: dropped {} poison indexed-file row(s) via bisection so the batch could land (REQ-AXO-902198): {:?}",
            poison.len(),
            dropped
        );
        POISON_ROWS_DROPPED.fetch_add(poison.len() as u64, std::sync::atomic::Ordering::Relaxed);
    }
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
        assert!(
            !should_use_bulk_writer(t - 1),
            "small flush stays on INSERT"
        );
        assert!(should_use_bulk_writer(t), "batch at threshold uses COPY");
        assert!(should_use_bulk_writer(t + 10_000), "huge batch uses COPY");

        // Explicit override wins over batch size, both directions.
        std::env::set_var("AXON_BULK_WRITER_ENABLED", "true");
        assert!(
            should_use_bulk_writer(1),
            "force-on uses COPY even for 1 row"
        );
        std::env::set_var("AXON_BULK_WRITER_ENABLED", "0");
        assert!(
            !should_use_bulk_writer(1_000_000),
            "force-off never uses COPY"
        );
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
    fn strip_nul_removes_nul_bytes_and_borrows_when_clean() {
        // REQ-AXO-902197 — clean text is borrowed (no alloc); NULs are stripped so
        // the COPY batch never aborts on 0x00 (the frozen-drain incident).
        assert!(matches!(strip_nul("clean body"), std::borrow::Cow::Borrowed(_)));
        assert_eq!(strip_nul("clean body"), "clean body");
        assert!(matches!(strip_nul("a\0b\0c"), std::borrow::Cow::Owned(_)));
        assert_eq!(strip_nul("a\0b\0c"), "abc");
        assert_eq!(strip_nul("\0"), "");
    }

    #[test]
    fn classify_copy_error_maps_sqlstate_families() {
        // REQ-AXO-902198 — Transient: the data is fine, retry the batch.
        assert_eq!(classify_copy_error("40001"), CopyErrorClass::Transient); // serialization
        assert_eq!(classify_copy_error("40P01"), CopyErrorClass::Transient); // deadlock
        assert_eq!(classify_copy_error("55P03"), CopyErrorClass::Transient); // lock_not_available
        assert_eq!(classify_copy_error("57014"), CopyErrorClass::Transient); // statement_timeout
        assert_eq!(classify_copy_error("08006"), CopyErrorClass::Transient); // connection_failure
        // Data: the row is poison → bisect to isolate it.
        assert_eq!(classify_copy_error("22021"), CopyErrorClass::Data); // invalid byte (the 0x00 class)
        assert_eq!(classify_copy_error("22P02"), CopyErrorClass::Data); // invalid text representation
        assert_eq!(classify_copy_error("23505"), CopyErrorClass::Data); // unique_violation
        assert_eq!(classify_copy_error("23503"), CopyErrorClass::Data); // foreign_key_violation
        assert_eq!(classify_copy_error("23502"), CopyErrorClass::Data); // not_null_violation
        assert_eq!(classify_copy_error("23514"), CopyErrorClass::Data); // check_violation
        // Other: not a poison row → propagate unchanged.
        assert_eq!(classify_copy_error("42601"), CopyErrorClass::Other); // syntax_error
        assert_eq!(classify_copy_error("42501"), CopyErrorClass::Other); // insufficient_privilege
    }

    /// Drive the pure `BisectPlanner` with a known poison set: a probed slice "fails DATA"
    /// iff it contains a poison index (else it lands). The resilient flush relies on this
    /// isolating EXACTLY the poison indices while every clean row lands.
    fn run_planner(total: usize, poison_set: &[usize]) -> Vec<usize> {
        let mut pl = BisectPlanner::new(total);
        while let Some(r) = pl.next() {
            if poison_set.iter().any(|p| r.contains(p)) {
                pl.on_data(r);
            }
        }
        pl.into_poison()
    }

    #[test]
    fn bisect_planner_isolates_exactly_the_poison_rows() {
        // Happy path: no poison → one probe, zero drops.
        assert_eq!(run_planner(8, &[]), Vec::<usize>::new());
        assert_eq!(run_planner(8, &[3]), vec![3]); // single, middle
        assert_eq!(run_planner(8, &[0, 7]), vec![0, 7]); // both ends
        assert_eq!(run_planner(10, &[2, 3, 8]), vec![2, 3, 8]); // several, non-power-of-two
        assert_eq!(run_planner(1, &[0]), vec![0]); // singleton poison
        assert_eq!(run_planner(0, &[]), Vec::<usize>::new()); // empty
        assert_eq!(run_planner(4, &[0, 1, 2, 3]), vec![0, 1, 2, 3]); // all poison
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
                cyclomatic_complexity: None,
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
            // REQ-AXO-901493 — generic mapped-edge bucket counts too.
            other_edges: vec![(
                "IMPLEMENTS".to_string(),
                RelationRow {
                    source_id: "s1".to_string(),
                    target_id: "Trait".to_string(),
                    project_code: "AXO".to_string(),
                },
            )],
            indexed_files: vec![],
            project_code: "AXO".to_string(),
        };
        assert!(!b.is_empty());
        assert_eq!(b.row_count(), 5);
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
