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

    // REQ-AXO-901653 Slice 3a — `graph_ingestion_graph_projection_queue_surface_stays_constructive_on_empty_store`
    // also removed (depended on `fetch_pending_graph_projection_work` which still
    // CRUDs the legacy table). Slice 3b will delete the method.

    // REQ-AXO-901653 Slice 3a — `req_axo_269_v1_mark_graph_projection_work_done_drains_queue_to_zero`
    // removed. It exercised the legacy `GraphProjectionQueue` enqueue/
    // fetch/mark_done cycle that is structurally obsolete post-MIL-AXO-017
    // (AGE retired) + REQ-AXO-289 (streaming v2 canonical). The
    // `fetch_graph_projection_queue_counts` method now returns (0,0)
    // unconditionally (the table no longer exists in canonical PG schema
    // ; CREATE in graph_bootstrap.rs is DuckDB-era residue cleaned in
    // Slice 4). The three "*_surface_stays_constructive_on_empty_store"
    // tests below still pass because they assert (0,0) on empty store.

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
