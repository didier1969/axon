//! REQ-AXO-902198 — real-PG fault-injection for the resilient drain flush.
//!
//! Validates the ONE link the unit tests can't reach: that `pg_sqlstate` extracts
//! the SQLSTATE from a REAL `tokio_postgres` error chain, so `classify_copy_error`
//! sees a DATA poison, the resilient fallback triggers, and the bisection ISOLATES
//! the poison row — the batch lands instead of freezing the drain.
//!
//! Runs against the real dev PG (no docker). Requires AXON_DEV_DATABASE_URL, e.g.:
//!   AXON_DEV_DATABASE_URL=postgres://axon@127.0.0.1:44144/axon_dev \
//!     cargo test --manifest-path src/axon-core/Cargo.toml \
//!       --test pg_bulk_writer_poison_faultinj -- --ignored --nocapture
//!
//! Poison = a chunk whose `file_path` has NO ist.IndexedFile parent → FK violation
//! (SQLSTATE 23503) on the Chunk merge. The atomic flush aborts on it; the resilient
//! fallback commits the structural core (incl. the VALID file's IndexedFile) then
//! bisects the chunks: the valid chunk lands, the FK-poison chunk is dropped.

use axon_core::postgres::bulk_writer::{
    self, BulkWriterChunkRow, BulkWriterRelationRow, BulkWriterSymbolRow, PgBulkBatch,
};

const P: &str = "AXO";
const TAG: &str = "faultinj_902198";

fn sym(id: &str) -> BulkWriterSymbolRow {
    BulkWriterSymbolRow {
        symbol_id: id.to_string(),
        name: "faultinj_sym".to_string(),
        kind: "function".to_string(),
        tested: false,
        is_public: false,
        is_nif: false,
        is_unsafe: false,
        project_code: P.to_string(),
        embedding: None,
    }
}

fn chunk(id: &str, file_path: &str) -> BulkWriterChunkRow {
    BulkWriterChunkRow {
        chunk_id: id.to_string(),
        source_type: "symbol".to_string(),
        source_id: format!("AXO::{TAG}::sym"),
        project_code: P.to_string(),
        file_path: file_path.to_string(),
        kind: "function".to_string(),
        content: "fn faultinj() {}".to_string(),
        content_hash: format!("h-{id}"),
        start_line: 1,
        end_line: 2,
        part_index: 0,
        part_count: 1,
        chunk_path: format!("{file_path}#faultinj"),
        token_count: None,
    }
}

#[test]
#[ignore = "requires AXON_DEV_DATABASE_URL (real dev PG); run with -- --ignored"]
fn resilient_flush_isolates_fk_poison_chunk_instead_of_freezing() {
    let url = std::env::var("AXON_DEV_DATABASE_URL")
        .expect("AXON_DEV_DATABASE_URL must point at the dev PG");
    std::env::set_var("AXON_DB_BACKEND", "postgres");
    std::env::set_var("DATABASE_URL", &url); // bulk_writer global pool resolves this
    std::env::set_var("AXON_BULK_WRITER_ENABLED", "1");
    std::env::remove_var("AXON_LIVE_DATABASE_URL");

    let valid_path = format!("/tmp/{TAG}_valid.rs");
    let bogus_path = format!("/tmp/{TAG}_BOGUS.rs"); // NOT in indexed_files → FK-poison

    let valid_chunk_id = format!("AXO::{TAG}::chunk_valid");
    let poison_chunk_id = format!("AXO::{TAG}::chunk_poison");

    let batch = PgBulkBatch {
        symbols: vec![sym(&format!("AXO::{TAG}::sym"))],
        chunks: vec![
            chunk(&valid_chunk_id, &valid_path), // FK parent created below
            chunk(&poison_chunk_id, &bogus_path), // FK parent MISSING → poison
        ],
        indexed_files: vec![(valid_path.clone(), format!("h-{TAG}"), 0, 0, 0)],
        project_code: P.to_string(),
        ..Default::default()
    };

    // THE ASSERTION: without the fix the FK-poison makes the whole COPY batch abort →
    // flush_batch returns Err (drain freezes). With the resilient fallback it returns Ok:
    // pg_sqlstate found 23503 → classify=Data → structural core committed → chunks bisected.
    bulk_writer::flush_batch(&batch)
        .expect("resilient flush must NOT freeze/err on an FK-poison chunk (REQ-AXO-902198)");

    // Prove isolation, not error-swallowing: the valid chunk landed, the poison did not.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let (client, conn) = tokio_postgres::connect(&url, tokio_postgres::NoTls)
            .await
            .expect("connect dev PG");
        tokio::spawn(async move {
            let _ = conn.await;
        });
        let valid_present: i64 = client
            .query_one(
                "SELECT count(*) FROM ist.chunk WHERE id = $1",
                &[&valid_chunk_id],
            )
            .await
            .unwrap()
            .get(0);
        let poison_present: i64 = client
            .query_one(
                "SELECT count(*) FROM ist.chunk WHERE id = $1",
                &[&poison_chunk_id],
            )
            .await
            .unwrap()
            .get(0);

        assert_eq!(valid_present, 1, "the clean chunk must land");
        assert_eq!(poison_present, 0, "the FK-poison chunk must be isolated/dropped");

        // Cleanup — leave the dev DB as we found it.
        for id in [&valid_chunk_id, &poison_chunk_id] {
            let _ = client
                .execute("DELETE FROM ist.chunk WHERE id = $1", &[id])
                .await;
        }
        let _ = client
            .execute(
                "DELETE FROM ist.symbol WHERE id = $1",
                &[&format!("AXO::{TAG}::sym")],
            )
            .await;
        let _ = client
            .execute("DELETE FROM ist.indexedfile WHERE path = $1", &[&valid_path])
            .await;
    });
}

#[test]
#[ignore = "requires AXON_DEV_DATABASE_URL (real dev PG); run with -- --ignored"]
fn reindex_purges_stale_outbound_call_edge() {
    // REQ-AXO-902204 — re-indexing a file must drop its symbols' OUTBOUND CALLS edges so a
    // call removed from the code doesn't leave a stale edge surviving forever. The old purge
    // only deleted `source_id = <file path>` edges (CONTAINS), never `source_id = <method id>`.
    let url = std::env::var("AXON_DEV_DATABASE_URL")
        .expect("AXON_DEV_DATABASE_URL must point at the dev PG");
    std::env::set_var("AXON_DB_BACKEND", "postgres");
    std::env::set_var("DATABASE_URL", &url);
    std::env::set_var("AXON_BULK_WRITER_ENABLED", "1");
    std::env::remove_var("AXON_LIVE_DATABASE_URL");

    let tag = "purge_902204";
    let file = format!("/tmp/{tag}_file.rs");
    let caller = format!("AXO::{tag}::caller");
    let callee = format!("AXO::{tag}::callee");

    let contains = BulkWriterRelationRow {
        source_id: file.clone(),
        target_id: caller.clone(),
        project_code: P.to_string(),
    };
    let base = |calls: Vec<BulkWriterRelationRow>| PgBulkBatch {
        symbols: vec![sym(&caller), sym(&callee)],
        chunks: vec![chunk(&format!("AXO::{tag}::chunk"), &file)],
        contains: vec![contains.clone()],
        calls,
        indexed_files: vec![(file.clone(), format!("h-{tag}"), 0, 0, 0)],
        project_code: P.to_string(),
        ..Default::default()
    };

    // Index #1 — the caller HAS a call to callee.
    let call = BulkWriterRelationRow {
        source_id: caller.clone(),
        target_id: callee.clone(),
        project_code: P.to_string(),
    };
    bulk_writer::flush_batch(&base(vec![call])).expect("first index");

    // NOTE: bulk_writer::flush_batch drives its OWN runtime, so every flush_batch call must
    // stay OUTSIDE a block_on (nested runtime → panic). Queries run in their own block_on.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let count_call = |caller: String, callee: String| {
        let url = url.clone();
        async move {
            let (client, conn) = tokio_postgres::connect(&url, tokio_postgres::NoTls)
                .await
                .unwrap();
            tokio::spawn(async move {
                let _ = conn.await;
            });
            let n: i64 = client
                .query_one(
                    "SELECT count(*) FROM ist.edge WHERE source_id=$1 AND target_id=$2 AND relation_type='CALLS'",
                    &[&caller, &callee],
                )
                .await
                .unwrap()
                .get(0);
            n
        }
    };

    let after_first = rt.block_on(count_call(caller.clone(), callee.clone()));
    assert_eq!(after_first, 1, "the call edge must exist after index #1");

    // Index #2 — the call was REMOVED from the code (empty calls). The re-index purge must drop
    // the stale S->T edge; without REQ-AXO-902204 it would survive. OUTSIDE block_on.
    bulk_writer::flush_batch(&base(vec![])).expect("re-index");

    let after_reindex = rt.block_on(count_call(caller.clone(), callee.clone()));
    assert_eq!(
        after_reindex, 0,
        "the stale call edge must be purged on re-index (REQ-AXO-902204)"
    );

    // Cleanup.
    rt.block_on(async {
        let (client, conn) = tokio_postgres::connect(&url, tokio_postgres::NoTls)
            .await
            .unwrap();
        tokio::spawn(async move {
            let _ = conn.await;
        });
        for id in [&caller, &callee] {
            let _ = client
                .execute("DELETE FROM ist.edge WHERE source_id=$1 OR target_id=$1", &[id])
                .await;
            let _ = client.execute("DELETE FROM ist.symbol WHERE id=$1", &[id]).await;
        }
        let _ = client.execute("DELETE FROM ist.edge WHERE source_id=$1", &[&file]).await;
        let _ = client
            .execute("DELETE FROM ist.chunk WHERE id=$1", &[&format!("AXO::{tag}::chunk")])
            .await;
        let _ = client.execute("DELETE FROM ist.indexedfile WHERE path=$1", &[&file]).await;
    });
}
