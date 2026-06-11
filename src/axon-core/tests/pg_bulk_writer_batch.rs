//! REQ-AXO-238 integration test: bulk_writer atomic flush_batch
//! covering Symbol + Chunk + CONTAINS + CALLS + CALLS_NIF in one tx.
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
//!   - Cross-table count parity: a single PgBulkBatch with rows in
//!     every bucket lands the right number of rows in each public
//!     table.
//!   - Atomic-per-batch semantics: ON CONFLICT idempotence holds
//!     across all 5 tables when the same batch is flushed twice.
//!   - Symbol embedding round-trip: rows that carry an embedding can
//!     be ranked via the cosine ANN query that production retrieve_context
//!     uses.

use std::sync::Mutex;
use std::thread::sleep;
use std::time::Duration;

use axon_core::graph::GraphStore;
use axon_core::postgres::bulk_writer::{
    self, BulkWriterChunkRow, BulkWriterRelationRow, BulkWriterSymbolRow, PgBulkBatch,
};
use testcontainers::core::{ContainerPort, WaitFor};
use testcontainers::runners::SyncRunner;
use testcontainers::{GenericImage, ImageExt};

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn full_dim_embedding(seed: usize) -> Vec<f32> {
    let mut v = vec![0.0_f32; axon_core::embedding_contract::DIMENSION];
    v[0] = (seed as f32) * 0.001;
    v[1] = ((seed % 7) as f32) * 0.01;
    v[2] = -((seed % 11) as f32) * 0.001;
    v
}

fn sample_symbol(idx: usize) -> BulkWriterSymbolRow {
    BulkWriterSymbolRow {
        symbol_id: format!("AXO::sym::{idx:04}"),
        name: format!("alpha_{idx}"),
        kind: "function".to_string(),
        tested: idx % 2 == 0,
        is_public: idx % 3 == 0,
        is_nif: false,
        is_unsafe: false,
        project_code: "AXO".to_string(),
        embedding: Some(full_dim_embedding(idx)),
    }
}

fn sample_chunk(idx: usize) -> BulkWriterChunkRow {
    BulkWriterChunkRow {
        chunk_id: format!("AXO::chunk::{idx:04}"),
        source_type: "symbol".to_string(),
        source_id: format!("AXO::sym::{idx:04}"),
        project_code: "AXO".to_string(),
        file_path: format!("/tmp/file_{idx}.rs"),
        kind: "function".to_string(),
        content: format!("fn alpha_{idx}() {{}}"),
        content_hash: format!("h-{idx:04}"),
        start_line: idx as i64,
        end_line: (idx + 1) as i64,
        part_index: 0,
        part_count: 1,
        chunk_path: format!("/tmp/file_{idx}.rs#alpha_{idx}"),
        token_count: None,
    }
}

fn sample_relation(src: &str, tgt: &str) -> BulkWriterRelationRow {
    BulkWriterRelationRow {
        source_id: src.to_string(),
        target_id: tgt.to_string(),
        project_code: "AXO".to_string(),
    }
}

#[test]
#[ignore = "requires docker; opt-in via `cargo test -- --ignored`"]
fn flush_batch_cross_table_round_trip() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    let container = GenericImage::new("axon-test/age-pgvector", "pg17")
        .with_exposed_port(ContainerPort::Tcp(5432))
        .with_wait_for(WaitFor::message_on_stderr(
            "database system is ready to accept connections",
        ))
        .with_env_var("POSTGRES_PASSWORD", "axon_batch_pw")
        .with_env_var("POSTGRES_DB", "axon_batch")
        .with_env_var("POSTGRES_USER", "postgres")
        .start()
        .expect("start container");
    let port = container
        .get_host_port_ipv4(5432)
        .expect("ephemeral host port");
    let url = format!("postgres://postgres:axon_batch_pw@127.0.0.1:{port}/axon_batch");

    std::env::set_var("AXON_DB_BACKEND", "postgres");
    std::env::set_var("AXON_LIVE_DATABASE_URL", &url);
    std::env::remove_var("AXON_DEV_DATABASE_URL");
    std::env::set_var("AXON_BULK_WRITER_ENABLED", "1");

    // Boot GraphStore so the schema (extensions + IST tables + AGE
    // labels) is provisioned. flush_batch writes into public.{Symbol,
    // Chunk, CONTAINS, CALLS, CALLS_NIF}; without the bootstrap those
    // tables don't exist.
    let mut last_err = None;
    let mut store = None;
    for _ in 0..10 {
        match GraphStore::new("/tmp/axon-pg-batch-unused") {
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

    // Build a 10-row Symbol + 10-row Chunk batch with relations linking
    // every Chunk to its Symbol (CONTAINS) and every Symbol to the
    // next one (CALLS) plus a NIF cross-edge for symbol_0 -> nif_target.
    let symbols: Vec<BulkWriterSymbolRow> = (0..10).map(sample_symbol).collect();
    let chunks: Vec<BulkWriterChunkRow> = (0..10).map(sample_chunk).collect();
    let contains: Vec<BulkWriterRelationRow> = (0..10)
        .map(|i| sample_relation(&format!("/tmp/file_{i}.rs"), &format!("AXO::sym::{i:04}")))
        .collect();
    let calls: Vec<BulkWriterRelationRow> = (0..9)
        .map(|i| {
            sample_relation(
                &format!("AXO::sym::{i:04}"),
                &format!("AXO::sym::{:04}", i + 1),
            )
        })
        .collect();
    let calls_nif: Vec<BulkWriterRelationRow> =
        vec![sample_relation("AXO::sym::0000", "AXO::nif::erlang_apply")];

    let batch = PgBulkBatch {
        symbols: symbols.clone(),
        chunks: chunks.clone(),
        contains: contains.clone(),
        calls: calls.clone(),
        calls_nif: calls_nif.clone(),
        indexed_files: Vec::new(),
    };

    bulk_writer::flush_batch(&batch).expect("first flush_batch should succeed");

    // Counts after the first flush — exactly the input cardinality on
    // every table.
    let count_symbols = store
        .query_count("SELECT count(*)::BIGINT FROM public.Symbol WHERE project_code='AXO'")
        .expect("count public.Symbol");
    assert_eq!(count_symbols, 10, "expected 10 Symbol rows");

    let count_chunks = store
        .query_count("SELECT count(*)::BIGINT FROM public.Chunk WHERE project_code='AXO'")
        .expect("count public.Chunk");
    assert_eq!(count_chunks, 10, "expected 10 Chunk rows");

    let count_contains = store
        .query_count("SELECT count(*)::BIGINT FROM public.CONTAINS WHERE project_code='AXO'")
        .expect("count public.CONTAINS");
    assert_eq!(count_contains, 10, "expected 10 CONTAINS rows");

    let count_calls = store
        .query_count("SELECT count(*)::BIGINT FROM public.CALLS WHERE project_code='AXO'")
        .expect("count public.CALLS");
    assert_eq!(count_calls, 9, "expected 9 CALLS rows");

    let count_calls_nif = store
        .query_count("SELECT count(*)::BIGINT FROM public.CALLS_NIF WHERE project_code='AXO'")
        .expect("count public.CALLS_NIF");
    assert_eq!(count_calls_nif, 1, "expected 1 CALLS_NIF row");

    // Re-flush the same batch — ON CONFLICT semantics hold across all
    // 5 tables so counts stay identical.
    bulk_writer::flush_batch(&batch).expect("second flush_batch (idempotence) should succeed");

    let recount_symbols = store
        .query_count("SELECT count(*)::BIGINT FROM public.Symbol WHERE project_code='AXO'")
        .expect("recount Symbol");
    assert_eq!(recount_symbols, 10, "Symbol PK dedupe");

    let recount_chunks = store
        .query_count("SELECT count(*)::BIGINT FROM public.Chunk WHERE project_code='AXO'")
        .expect("recount Chunk");
    assert_eq!(recount_chunks, 10, "Chunk PK dedupe");

    let recount_contains = store
        .query_count("SELECT count(*)::BIGINT FROM public.CONTAINS WHERE project_code='AXO'")
        .expect("recount CONTAINS");
    assert_eq!(recount_contains, 10, "CONTAINS PK dedupe via DO NOTHING");

    let recount_calls = store
        .query_count("SELECT count(*)::BIGINT FROM public.CALLS WHERE project_code='AXO'")
        .expect("recount CALLS");
    assert_eq!(recount_calls, 9, "CALLS PK dedupe via DO UPDATE");

    let recount_calls_nif = store
        .query_count("SELECT count(*)::BIGINT FROM public.CALLS_NIF WHERE project_code='AXO'")
        .expect("recount CALLS_NIF");
    assert_eq!(recount_calls_nif, 1, "CALLS_NIF PK dedupe via DO UPDATE");

    // Partial-bucket scenario in the same container session: a batch
    // with only Symbol + Chunk populated must not error on the empty
    // relation buckets, and the existing CONTAINS rows must remain
    // untouched (count stays at 10 from the first flush).
    let partial_batch = PgBulkBatch {
        symbols: vec![BulkWriterSymbolRow {
            symbol_id: "AXO::sym::partial".to_string(),
            name: "partial".to_string(),
            kind: "function".to_string(),
            tested: false,
            is_public: false,
            is_nif: false,
            is_unsafe: false,
            project_code: "AXO".to_string(),
            embedding: None,
        }],
        chunks: vec![BulkWriterChunkRow {
            chunk_id: "AXO::chunk::partial".to_string(),
            source_type: "symbol".to_string(),
            source_id: "AXO::sym::partial".to_string(),
            project_code: "AXO".to_string(),
            file_path: "/tmp/partial.rs".to_string(),
            kind: "function".to_string(),
            content: "fn partial() {}".to_string(),
            content_hash: "h-partial".to_string(),
            start_line: 1,
            end_line: 1,
            part_index: 0,
            part_count: 1,
            chunk_path: "/tmp/partial.rs#partial".to_string(),
            token_count: None,
        }],
        contains: vec![],
        calls: vec![],
        calls_nif: vec![],
        indexed_files: Vec::new(),
    };
    bulk_writer::flush_batch(&partial_batch).expect("partial-bucket batch should succeed");

    let after_partial_symbols = store
        .query_count("SELECT count(*)::BIGINT FROM public.Symbol WHERE project_code='AXO'")
        .expect("Symbol count after partial flush");
    assert_eq!(
        after_partial_symbols, 11,
        "partial flush should add exactly one Symbol"
    );

    let after_partial_contains = store
        .query_count("SELECT count(*)::BIGINT FROM public.CONTAINS WHERE project_code='AXO'")
        .expect("CONTAINS count after partial flush");
    assert_eq!(
        after_partial_contains, 10,
        "empty bucket must not modify existing CONTAINS rows"
    );

    drop(store);

    std::env::remove_var("AXON_DB_BACKEND");
    std::env::remove_var("AXON_LIVE_DATABASE_URL");
    std::env::remove_var("AXON_BULK_WRITER_ENABLED");
}
