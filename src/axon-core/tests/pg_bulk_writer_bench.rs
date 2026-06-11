//! REQ-AXO-238 micro-bench: bulk_writer COPY BINARY ChunkEmbedding throughput.
//!
//! Marked `#[ignore]`. Requirements:
//!   1. Docker runtime available (testcontainers spawns
//!      `axon-test/age-pgvector:pg17`).
//!   2. `libaxon_plugin_postgres.so` already built — the test does not
//!      shell out to cargo.
//!
//! Measures the bulk_writer COPY BINARY path in isolation: 5000
//! synthetic ChunkEmbeddingPersistRows (1024-dim) flushed in one call.
//! No comparison against the legacy per-row INSERT path — operator
//! ruled it out (already known slow by construction; the bench just
//! confirms bulk_writer hits its target ms/row on a fresh PG).

use std::sync::Mutex;
use std::thread::sleep;
use std::time::{Duration, Instant};

use axon_core::graph::GraphStore;
use axon_core::postgres::bulk_writer;
use axon_core::postgres::bulk_writer::BulkWriterChunkEmbeddingRow as ChunkEmbeddingPersistRow;
use testcontainers::core::{ContainerPort, WaitFor};
use testcontainers::runners::SyncRunner;
use testcontainers::{GenericImage, ImageExt};

static ENV_LOCK: Mutex<()> = Mutex::new(());

const ROW_COUNT: usize = 5_000;

fn synth_embedding(seed: usize) -> Vec<f32> {
    let mut v = vec![0.0_f32; axon_core::embedding_contract::DIMENSION];
    for i in 0..16 {
        v[i] = ((seed.wrapping_mul(1103515245).wrapping_add(12345 + i)) as f32 / 1e6).sin();
    }
    v
}

fn rows() -> Vec<ChunkEmbeddingPersistRow> {
    (0..ROW_COUNT)
        .map(|i| ChunkEmbeddingPersistRow {
            chunk_id: format!("AXO-bench-chunk-{i:05}"),
            source_hash: format!("hash-{i:08x}"),
            embedding: synth_embedding(i),
        })
        .collect()
}

#[test]
#[ignore = "requires docker; opt-in via `cargo test -- --ignored`"]
fn bench_bulk_writer_chunk_embedding_throughput() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    let container = GenericImage::new("axon-test/age-pgvector", "pg17")
        .with_exposed_port(ContainerPort::Tcp(5432))
        .with_wait_for(WaitFor::message_on_stderr(
            "database system is ready to accept connections",
        ))
        .with_env_var("POSTGRES_PASSWORD", "axon_bench_pw")
        .with_env_var("POSTGRES_DB", "axon_bench")
        .with_env_var("POSTGRES_USER", "postgres")
        .start()
        .expect("start container");
    let port = container
        .get_host_port_ipv4(5432)
        .expect("ephemeral host port");
    let url = format!("postgres://postgres:axon_bench_pw@127.0.0.1:{port}/axon_bench");

    std::env::set_var("AXON_DB_BACKEND", "postgres");
    std::env::set_var("AXON_LIVE_DATABASE_URL", &url);
    std::env::remove_var("AXON_DEV_DATABASE_URL");
    std::env::set_var("AXON_BULK_WRITER_ENABLED", "1");

    // GraphStore::new triggers bootstrap_global_pg_schema (provisions
    // public.ChunkEmbedding + extensions). Required before bulk_writer
    // can flush.
    let mut last_err = None;
    let mut store = None;
    for _ in 0..10 {
        match GraphStore::new("/tmp/axon-pg-bench-unused") {
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

    let row_set = rows();
    let now_ms = 1715000000000_i64;

    // Flush 5000 rows via bulk_writer COPY BINARY.
    let t0 = Instant::now();
    bulk_writer::flush_chunk_embeddings("AXO", "code-1024", &row_set, now_ms)
        .expect("bulk_writer flush should succeed");
    let bulk_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let bulk_rps = ROW_COUNT as f64 / (bulk_ms / 1000.0);

    let bulk_count = store
        .query_count("SELECT count(*)::BIGINT FROM public.ChunkEmbedding WHERE project_code='AXO'")
        .expect("bulk row count");
    assert_eq!(
        bulk_count as usize, ROW_COUNT,
        "bulk_writer must land all 5000 rows"
    );

    eprintln!();
    eprintln!(
        "=== REQ-AXO-238 bulk_writer micro-bench ({} rows, dim={}) ===",
        ROW_COUNT,
        axon_core::embedding_contract::DIMENSION
    );
    eprintln!("  COPY BINARY + temp staging + ON CONFLICT merge:");
    eprintln!("    {bulk_ms:.0} ms total = {bulk_rps:.0} rows/s");
    eprintln!();

    drop(store);

    std::env::remove_var("AXON_DB_BACKEND");
    std::env::remove_var("AXON_LIVE_DATABASE_URL");
    std::env::remove_var("AXON_BULK_WRITER_ENABLED");

    assert!(bulk_ms > 0.0);
}
