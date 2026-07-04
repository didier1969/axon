//! REQ-AXO-238 micro-bench: bulk_writer flush_batch cross-table
//! throughput.
//!
//! Marked `#[ignore]`. Requirements:
//!   1. Docker runtime available (testcontainers spawns
//!      `axon-test/age-pgvector:pg17`).
//!   2. `libaxon_plugin_postgres.so` already built — the test does not
//!      shell out to cargo.
//!
//! Measures the bulk_writer atomic flush_batch path under load on a
//! realistic-shaped producer batch:
//!   - 1000 Symbol rows (with full-dim embeddings)
//!   - 1000 Chunk rows
//!   - 2000 CONTAINS rows
//!   - 1000 CALLS rows
//!   - 100  CALLS_NIF rows
//!
//! Total 5100 rows folded into one tx. Reports wall time + rows/s.
//! Companion to `pg_bulk_writer_bench.rs` (single-table ChunkEmbedding
//! micro-bench, 2033 rows/s on 5K dim-1024 per commit b08c0aa).

use std::sync::Mutex;
use std::thread::sleep;
use std::time::{Duration, Instant};

use axon_core::graph::GraphStore;
use axon_core::postgres::bulk_writer::{
    self, BulkWriterChunkRow, BulkWriterRelationRow, BulkWriterSymbolRow, PgBulkBatch,
};
use testcontainers::core::{ContainerPort, WaitFor};
use testcontainers::runners::SyncRunner;
use testcontainers::{GenericImage, ImageExt};

static ENV_LOCK: Mutex<()> = Mutex::new(());

const SYMBOL_COUNT: usize = 1_000;
const CHUNK_COUNT: usize = 1_000;
const CONTAINS_COUNT: usize = 2_000;
const CALLS_COUNT: usize = 1_000;
const CALLS_NIF_COUNT: usize = 100;

fn full_dim_embedding(seed: usize) -> Vec<f32> {
    let mut v = vec![0.0_f32; axon_core::embedding_contract::DIMENSION];
    for i in 0..16 {
        v[i] = ((seed.wrapping_mul(1103515245).wrapping_add(12345 + i)) as f32 / 1e6).sin();
    }
    v
}

fn build_batch() -> PgBulkBatch {
    let symbols: Vec<BulkWriterSymbolRow> = (0..SYMBOL_COUNT)
        .map(|i| BulkWriterSymbolRow {
            symbol_id: format!("AXO::sym::{i:06}"),
            name: format!("symbol_{i}"),
            kind: "function".to_string(),
            tested: i % 2 == 0,
            is_public: i % 3 == 0,
            is_nif: false,
            is_unsafe: false,
            project_code: "AXO".to_string(),
            embedding: Some(full_dim_embedding(i)),
            cyclomatic_complexity: None,
        })
        .collect();
    let chunks: Vec<BulkWriterChunkRow> = (0..CHUNK_COUNT)
        .map(|i| BulkWriterChunkRow {
            chunk_id: format!("AXO::chunk::{i:06}"),
            source_type: "symbol".to_string(),
            source_id: format!("AXO::sym::{i:06}"),
            project_code: "AXO".to_string(),
            file_path: format!("/tmp/file_{}.rs", i % 100),
            kind: "function".to_string(),
            content: format!("fn symbol_{i}() {{ /* {i} */ }}"),
            content_hash: format!("h-{i:08x}"),
            start_line: (i * 10) as i64,
            end_line: ((i + 1) * 10) as i64,
            part_index: 0,
            part_count: 1,
            chunk_path: format!("/tmp/file_{}.rs#symbol_{i}", i % 100),
            token_count: None,
        })
        .collect();
    let contains: Vec<BulkWriterRelationRow> = (0..CONTAINS_COUNT)
        .map(|i| BulkWriterRelationRow {
            source_id: format!("/tmp/file_{}.rs", i % 100),
            target_id: format!("AXO::sym::{:06}", i % SYMBOL_COUNT),
            project_code: "AXO".to_string(),
        })
        .collect();
    let calls: Vec<BulkWriterRelationRow> = (0..CALLS_COUNT)
        .map(|i| BulkWriterRelationRow {
            source_id: format!("AXO::sym::{:06}", i % SYMBOL_COUNT),
            target_id: format!("AXO::sym::{:06}", (i + 1) % SYMBOL_COUNT),
            project_code: "AXO".to_string(),
        })
        .collect();
    let calls_nif: Vec<BulkWriterRelationRow> = (0..CALLS_NIF_COUNT)
        .map(|i| BulkWriterRelationRow {
            source_id: format!("AXO::sym::{:06}", i % SYMBOL_COUNT),
            target_id: format!("AXO::nif::erlang_{i:03}"),
            project_code: "AXO".to_string(),
        })
        .collect();

    PgBulkBatch {
        symbols,
        chunks,
        contains,
        calls,
        calls_nif,
        other_edges: Vec::new(),
        indexed_files: Vec::new(),
        project_code: "AXO".to_string(),
    }
}

#[test]
#[ignore = "requires docker; opt-in via `cargo test -- --ignored`"]
fn bench_flush_batch_cross_table_throughput() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    let container = GenericImage::new("axon-test/age-pgvector", "pg17")
        .with_exposed_port(ContainerPort::Tcp(5432))
        .with_wait_for(WaitFor::message_on_stderr(
            "database system is ready to accept connections",
        ))
        .with_env_var("POSTGRES_PASSWORD", "axon_batch_bench_pw")
        .with_env_var("POSTGRES_DB", "axon_batch_bench")
        .with_env_var("POSTGRES_USER", "postgres")
        .start()
        .expect("start container");
    let port = container
        .get_host_port_ipv4(5432)
        .expect("ephemeral host port");
    let url = format!("postgres://postgres:axon_batch_bench_pw@127.0.0.1:{port}/axon_batch_bench");

    std::env::set_var("AXON_DB_BACKEND", "postgres");
    std::env::set_var("AXON_LIVE_DATABASE_URL", &url);
    std::env::remove_var("AXON_DEV_DATABASE_URL");
    std::env::set_var("AXON_BULK_WRITER_ENABLED", "1");

    let mut last_err = None;
    let mut store = None;
    for _ in 0..10 {
        match GraphStore::new("/tmp/axon-pg-batch-bench-unused") {
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

    let batch = build_batch();
    let total_rows = batch.row_count();

    let t0 = Instant::now();
    bulk_writer::flush_batch(&batch).expect("flush_batch should succeed");
    let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let rps = total_rows as f64 / (elapsed_ms / 1000.0);

    // Sanity-check counts post-flush.
    let symbols = store
        .query_count("SELECT count(*)::BIGINT FROM public.Symbol WHERE project_code='AXO'")
        .expect("count Symbol");
    let chunks = store
        .query_count("SELECT count(*)::BIGINT FROM public.Chunk WHERE project_code='AXO'")
        .expect("count Chunk");

    assert_eq!(
        symbols as usize, SYMBOL_COUNT,
        "Symbol count must match input"
    );
    assert_eq!(chunks as usize, CHUNK_COUNT, "Chunk count must match input");

    eprintln!();
    eprintln!(
        "=== REQ-AXO-238 flush_batch micro-bench ({} rows total) ===",
        total_rows
    );
    eprintln!(
        "  Symbol={} (with full-dim embeddings)  Chunk={}  CONTAINS={}  CALLS={}  CALLS_NIF={}",
        SYMBOL_COUNT, CHUNK_COUNT, CONTAINS_COUNT, CALLS_COUNT, CALLS_NIF_COUNT
    );
    eprintln!("  COPY BINARY x5 + ON CONFLICT merge x5, all in one transaction:");
    eprintln!("    {elapsed_ms:.0} ms wall = {rps:.0} rows/s aggregate");
    eprintln!();

    drop(store);

    std::env::remove_var("AXON_DB_BACKEND");
    std::env::remove_var("AXON_LIVE_DATABASE_URL");
    std::env::remove_var("AXON_BULK_WRITER_ENABLED");

    assert!(elapsed_ms > 0.0);
}
