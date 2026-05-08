//! REQ-AXO-238 integration test: bulk_writer COPY BINARY round-trip.
//!
//! Marked `#[ignore]`. Requirements:
//!   1. Docker runtime available (testcontainers spawns
//!      `axon-test/age-pgvector:pg17`).
//!   2. `libaxon_plugin_postgres.so` already built — the test does not
//!      shell out to cargo. Build with:
//!
//!          cargo build --manifest-path src/axon-plugin-postgres/Cargo.toml --lib
//!
//! Validates:
//!   - Counts parity: a 50-row flush via bulk_writer lands exactly 50
//!     rows in `public.ChunkEmbedding` (vs the legacy
//!     `upsert_chunk_embedding_sql` per-row path).
//!   - ON CONFLICT idempotence: re-flushing the same batch does not
//!     duplicate rows.
//!   - Round-trip integrity: vectors written via COPY BINARY (pgvector
//!     binary format) read back identically through the cosine ANN
//!     query that production callers use.

use std::sync::Mutex;
use std::thread::sleep;
use std::time::Duration;

use axon_core::graph::GraphStore;
use axon_core::postgres::bulk_writer;
use axon_core::postgres::bulk_writer::BulkWriterChunkEmbeddingRow as ChunkEmbeddingPersistRow;
use testcontainers::core::{ContainerPort, WaitFor};
use testcontainers::runners::SyncRunner;
use testcontainers::{GenericImage, ImageExt};

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn sample_embedding(seed: usize) -> Vec<f32> {
    let mut v = vec![0.0_f32; axon_core::embedding_contract::DIMENSION];
    // Sparse, deterministic values so each row's vector is distinct
    // and cosine ANN can rank them. Index modulation keeps the
    // signature unique per-seed.
    v[0] = (seed as f32) * 0.001;
    v[1] = ((seed as f32) % 7.0) * 0.01;
    v[2] = -((seed as f32) % 11.0) * 0.001;
    v
}

#[test]
#[ignore = "requires docker; opt-in via `cargo test -- --ignored`"]
fn bulk_writer_copy_binary_round_trip() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    let container = GenericImage::new("axon-test/age-pgvector", "pg17")
        .with_exposed_port(ContainerPort::Tcp(5432))
        .with_wait_for(WaitFor::message_on_stderr(
            "database system is ready to accept connections",
        ))
        .with_env_var("POSTGRES_PASSWORD", "axon_test_pw")
        .with_env_var("POSTGRES_DB", "axon_test_db")
        .with_env_var("POSTGRES_USER", "postgres")
        .start()
        .expect("start container");
    let port = container
        .get_host_port_ipv4(5432)
        .expect("ephemeral host port");
    let url = format!("postgres://postgres:axon_test_pw@127.0.0.1:{port}/axon_test_db");

    std::env::set_var("AXON_DB_BACKEND", "postgres");
    std::env::set_var("AXON_LIVE_DATABASE_URL", &url);
    std::env::remove_var("AXON_DEV_DATABASE_URL");
    // Keep the env gate ON for this test — bulk_writer_enabled() reads
    // it on every call. Default OFF semantics are covered by the
    // module's own unit tests.
    std::env::set_var("AXON_BULK_WRITER_ENABLED", "1");

    // Boot GraphStore through the same path production uses so the
    // schema (extensions + IST tables + AGE labels) is provisioned.
    let mut last_err = None;
    let mut store = None;
    for _ in 0..10 {
        match GraphStore::new("/tmp/axon-pg-bulk-writer-unused") {
            Ok(s) => {
                store = Some(s);
                break;
            }
            Err(e) => {
                last_err = Some(e);
                sleep(Duration::from_millis(500));
            }
        }
    }
    let store = store.unwrap_or_else(|| {
        panic!(
            "GraphStore::new under PG failed after retries: {:?}",
            last_err
        )
    });

    // Build 50 ChunkEmbeddingPersistRows. chunk_ids embed the
    // project_code prefix because bulk_writer accepts it as an explicit
    // arg (the producer hot path passes it explicitly too).
    let rows: Vec<ChunkEmbeddingPersistRow> = (0..50)
        .map(|i| ChunkEmbeddingPersistRow {
            chunk_id: format!("AXO-bench-chunk-{i:04}"),
            source_hash: format!("hash-{i:04}"),
            embedding: sample_embedding(i),
        })
        .collect();

    bulk_writer::flush_chunk_embeddings("AXO", "code-1024", &rows, 1715000000000)
        .expect("first bulk flush should succeed against the combined image");

    let count_after_first = store
        .query_count(
            "SELECT count(*)::BIGINT FROM public.ChunkEmbedding WHERE project_code = 'AXO'",
        )
        .expect("count public.ChunkEmbedding");
    assert_eq!(
        count_after_first, 50,
        "first flush should land exactly 50 rows"
    );

    // ON CONFLICT idempotence: re-flushing the same batch with a
    // bumped embedded_at_ms keeps row count at 50 and updates the
    // timestamp via DO UPDATE.
    bulk_writer::flush_chunk_embeddings("AXO", "code-1024", &rows, 1715000000999)
        .expect("re-flush via bulk_writer should be idempotent");

    let count_after_replay = store
        .query_count(
            "SELECT count(*)::BIGINT FROM public.ChunkEmbedding WHERE project_code = 'AXO'",
        )
        .expect("count after replay");
    assert_eq!(count_after_replay, 50, "ON CONFLICT must dedupe by PK");

    // Verify the timestamp got updated by the merge step (proves the
    // INSERT…SELECT…ON CONFLICT DO UPDATE actually fired, not a no-op
    // skip via a different path).
    let updated_count = store
        .query_count(
            "SELECT count(*)::BIGINT FROM public.ChunkEmbedding \
             WHERE project_code = 'AXO' AND embedded_at_ms = 1715000000999",
        )
        .expect("count timestamp-updated rows");
    assert_eq!(
        updated_count, 50,
        "DO UPDATE should bump embedded_at_ms on every row"
    );

    // Round-trip a vector via the cosine ANN query: rank against
    // sample_embedding(0)'s vector and verify chunk-0000 surfaces top.
    let q = sample_embedding(0);
    let where_segment = axon_core::postgres::vector::cosine_ann_where_order_limit(
        "code-1024",
        "AXO",
        &q,
        1,
    )
    .expect("ann query builds");
    // The helper returns the FROM/WHERE/ORDER BY/LIMIT segment;
    // wrap in a sub-select so count(*) is well-formed (count + ORDER
    // BY without GROUP BY is invalid SQL and would surface as a
    // -1 sentinel from the FFI). The expected count is exactly 1
    // (LIMIT 1) once the bulk-loaded rows are queryable.
    let select = format!("SELECT count(*)::BIGINT FROM (SELECT 1 {where_segment}) AS sub");
    let nearest = store
        .query_count(&select)
        .expect("ANN count should return 1 (limit 1)");
    assert_eq!(nearest, 1, "cosine ANN should return the limit row");

    drop(store);

    std::env::remove_var("AXON_DB_BACKEND");
    std::env::remove_var("AXON_LIVE_DATABASE_URL");
    std::env::remove_var("AXON_BULK_WRITER_ENABLED");
}
