// Copyright (c) Didier Stadelmann. All rights reserved.

#[cfg(test)]
mod tests {
    use crate::graph_ingestion::{
        FileVectorizationLeaseSnapshot, FileVectorizationWork, VectorBatchRun,
        VectorPersistOutboxPayload, VectorPersistOutboxUpdate,
    };

    #[test]
    fn graph_ingestion_public_reexports_preserve_vector_payload_contract() {
        let payload = VectorPersistOutboxPayload {
            updates: vec![VectorPersistOutboxUpdate {
                chunk_id: "chunk-1".to_string(),
                source_hash: "hash-1".to_string(),
                vector: vec![0.1, 0.2, 0.3],
            }],
            completed_works: vec![FileVectorizationWork {
                file_path: "src/lib.rs".to_string(),
                resumed_after_interactive_pause: false,
            }],
            completed_lease_snapshots: vec![FileVectorizationLeaseSnapshot {
                file_path: "src/lib.rs".to_string(),
                claim_token: "claim-1".to_string(),
                lease_epoch: 7,
            }],
            batch_run: VectorBatchRun {
                run_id: "run-1".to_string(),
                prepare_started_at_ms: 1,
                prepare_finished_at_ms: 2,
                ready_enqueued_at_ms: 3,
                started_at_ms: 4,
                finished_at_ms: 5,
                gpu_started_at_ms: 6,
                gpu_finished_at_ms: 7,
                persist_enqueued_at_ms: 8,
                persist_started_at_ms: 9,
                persist_finished_at_ms: 10,
                finalize_enqueued_at_ms: 11,
                finalize_finished_at_ms: 12,
                provider: "cpu".to_string(),
                runner_kind: "unit".to_string(),
                model_id: "model-1".to_string(),
                chunk_count: 1,
                file_count: 1,
                input_bytes: 128,
                total_tokens: 16,
                max_item_tokens: 16,
                avg_item_tokens: 16.0,
                micro_batch_count: 1,
                max_micro_batch_tokens: 16,
                avg_micro_batch_tokens: 16.0,
                effective_vector_workers_admitted: 1,
                ready_queue_depth_at_gpu_start: 0,
                prepare_inflight_at_gpu_start: 0,
                ready_queue_chunks_at_gpu_start: 1,
                prepare_inflight_chunks_at_gpu_start: 0,
                vector_worker_admission_reason: "test".to_string(),
                allowed_gpu_workers: 0,
                batch_wait_for_ready_ms: 0,
                persist_queue_wait_ms: 0,
                finalize_queue_wait_ms: 0,
                batch_lane: "default".to_string(),
                batch_shape: "single".to_string(),
                lane_small_max_tokens: 32,
                lane_medium_max_tokens: 64,
                fetch_ms: 13,
                embed_ms: 14,
                db_write_ms: 15,
                mark_done_ms: 16,
                success: true,
                error_reason: None,
            },
        };

        let json = serde_json::to_value(&payload).expect("payload should serialize");
        assert_eq!(json["completed_works"][0]["file_path"], "src/lib.rs");
        assert_eq!(json["completed_lease_snapshots"][0]["lease_epoch"], 7);
        assert_eq!(json["batch_run"]["provider"], "cpu");
        assert_eq!(json["updates"][0]["chunk_id"], "chunk-1");
    }

    #[test]
    fn graph_ingestion_vectorization_queue_surface_stays_constructive_on_empty_store() {
        let store = crate::tests::test_helpers::create_test_db().expect("test db");
        assert_eq!(
            store
                .fetch_file_vectorization_queue_counts()
                .expect("queue counts"),
            (0, 0)
        );
        assert_eq!(
            store
                .fetch_claimable_file_vectorization_queue_count()
                .expect("claimable count"),
            0
        );
    }

    #[test]
    fn graph_ingestion_graph_projection_queue_surface_stays_constructive_on_empty_store() {
        let store = crate::tests::test_helpers::create_test_db().expect("test db");
        assert_eq!(
            store
                .fetch_graph_projection_queue_counts()
                .expect("graph projection queue counts"),
            (0, 0)
        );
        assert!(store
            .fetch_pending_graph_projection_work(8)
            .expect("pending graph projection work")
            .is_empty());
    }

    // REQ-AXO-269 v1 unit-level coverage: prove that the underlying primitive
    // `mark_graph_projection_work_done` cleanly drains a queued+inflight set
    // back to (0, 0). This is the exact call the embedder.rs short-circuit
    // makes when `graph_store.skip_sql_relations()` returns true (PG mode).
    //
    // The bench harness can't easily exercise this path under PG-AGE-only
    // (autoconfig sets graph_workers=0 under tight VRAM, see VAL-AXO-061;
    // forcing AXON_GRAPH_WORKERS=1 timed out the harness, see VAL-AXO-062).
    // This test closes the validation loop at the primitive layer where the
    // DuckDB test fixture works fine.
    #[test]
    fn req_axo_269_v1_mark_graph_projection_work_done_drains_queue_to_zero() {
        use crate::graph_ingestion::GraphProjectionWork;
        let store = crate::tests::test_helpers::create_test_db().expect("test db");
        // Enqueue one work item then fetch+mark in one cycle. The
        // multi-row fetch path has a separate concern (DuckDB AND/OR
        // precedence under UPDATE); this test focuses on REQ-AXO-269 v1's
        // contract: given a non-empty `pending` batch + skip_sql_relations,
        // the primitive `mark_graph_projection_work_done` removes the
        // inflight rows it was given.
        store
            .enqueue_graph_projection_refresh("file", "src/foo.rs", 2)
            .expect("enqueue");
        let (queued, inflight) = store
            .fetch_graph_projection_queue_counts()
            .expect("counts");
        assert_eq!(queued, 1, "after enqueue queued=1");
        assert_eq!(inflight, 0, "after enqueue inflight=0");

        // fetch_pending pulls into 'inflight' — same code path the
        // graph_worker_loop uses upstream of REQ-269 v1's short-circuit.
        let pending = store
            .fetch_pending_graph_projection_work(8)
            .expect("fetch pending");
        assert_eq!(pending.len(), 1);
        let (queued, inflight) = store
            .fetch_graph_projection_queue_counts()
            .expect("counts mid-flight");
        assert_eq!(queued, 0, "after fetch_pending queued=0");
        assert_eq!(inflight, 1, "after fetch_pending inflight=1");

        // REQ-269 v1: short-circuit calls mark_graph_projection_work_done
        // on the entire pending batch. Prove it drains.
        let work_owned: Vec<GraphProjectionWork> = pending;
        store
            .mark_graph_projection_work_done(&work_owned)
            .expect("mark done");
        let (queued, inflight) = store
            .fetch_graph_projection_queue_counts()
            .expect("counts post-drain");
        assert_eq!(queued, 0, "queued must be 0 after mark_done");
        assert_eq!(inflight, 0, "inflight must be 0 after mark_done");
    }

    #[test]
    fn graph_ingestion_vector_persist_outbox_surface_stays_constructive_on_empty_store() {
        let store = crate::tests::test_helpers::create_test_db().expect("test db");
        assert_eq!(
            store
                .fetch_vector_persist_outbox_counts()
                .expect("vector persist outbox counts"),
            (0, 0)
        );
        assert!(store
            .fetch_pending_vector_persist_outbox_work(8)
            .expect("pending vector persist outbox work")
            .is_empty());
    }

    #[test]
    fn graph_ingestion_file_ingress_surface_stays_constructive_on_empty_store() {
        let store = crate::tests::test_helpers::create_test_db().expect("test db");
        assert!(store
            .fetch_file_ingress_row("src/lib.rs")
            .expect("file ingress row")
            .is_none());
        assert!(store
            .fetch_file_ingress_rows(&["src/lib.rs".to_string()])
            .expect("file ingress rows")
            .is_empty());
    }
}
