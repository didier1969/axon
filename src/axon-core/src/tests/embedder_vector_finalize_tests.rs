#[cfg(test)]
mod tests {
    use crate::embedder::VectorPersistPlan;
    use crate::graph_ingestion::{FileVectorizationWork, VectorBatchRun};

    #[test]
    fn embedder_vector_finalize_public_plan_syncs_batch_run_counts() {
        let mut plan = VectorPersistPlan {
            updates: vec![
                ("chunk-a".to_string(), "hash-a".to_string(), vec![0.1, 0.2]),
                ("chunk-b".to_string(), "hash-b".to_string(), vec![0.3, 0.4]),
            ],
            completed_works: vec![],
            next_active_after_failure: vec![],
            touched_works: vec![
                FileVectorizationWork {
                    file_path: "src/a.rs".to_string(),
                    resumed_after_interactive_pause: false,
                },
                FileVectorizationWork {
                    file_path: "src/b.rs".to_string(),
                    resumed_after_interactive_pause: true,
                },
            ],
            batch_run: VectorBatchRun {
                run_id: "run-1".to_string(),
                prepare_started_at_ms: 0,
                prepare_finished_at_ms: 0,
                ready_enqueued_at_ms: 0,
                started_at_ms: 0,
                finished_at_ms: 0,
                gpu_started_at_ms: 0,
                gpu_finished_at_ms: 0,
                persist_enqueued_at_ms: 0,
                persist_started_at_ms: 0,
                persist_finished_at_ms: 0,
                finalize_enqueued_at_ms: 0,
                finalize_finished_at_ms: 0,
                provider: "cpu".to_string(),
                runner_kind: "unit".to_string(),
                model_id: "model".to_string(),
                chunk_count: 0,
                file_count: 0,
                input_bytes: 0,
                total_tokens: 0,
                max_item_tokens: 0,
                avg_item_tokens: 0.0,
                micro_batch_count: 0,
                max_micro_batch_tokens: 0,
                avg_micro_batch_tokens: 0.0,
                effective_vector_workers_admitted: 0,
                ready_queue_depth_at_gpu_start: 0,
                prepare_inflight_at_gpu_start: 0,
                ready_queue_chunks_at_gpu_start: 0,
                prepare_inflight_chunks_at_gpu_start: 0,
                vector_worker_admission_reason: "test".to_string(),
                allowed_gpu_workers: 0,
                batch_wait_for_ready_ms: 0,
                persist_queue_wait_ms: 0,
                finalize_queue_wait_ms: 0,
                batch_lane: "default".to_string(),
                batch_shape: "single".to_string(),
                lane_small_max_tokens: 0,
                lane_medium_max_tokens: 0,
                fetch_ms: 0,
                embed_ms: 0,
                db_write_ms: 0,
                mark_done_ms: 0,
                success: true,
                error_reason: None,
            },
        };

        let (chunk_count, file_count) = plan.sync_batch_run_counts();

        assert_eq!((chunk_count, file_count), (2, 2));
        assert_eq!(plan.batch_run.chunk_count, 2);
        assert_eq!(plan.batch_run.file_count, 2);
    }
}
