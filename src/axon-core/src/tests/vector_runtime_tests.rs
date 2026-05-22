// REQ-AXO-901663 — coverage for LIVE vector_runtime methods.
// `record_*` setters were deleted in slice-5c (zero callers) ; tests
// populate the underlying axon_runtime tables via direct SQL INSERT.

#[cfg(test)]
mod tests {
    use crate::graph::GraphStore;
    use crate::tests::test_helpers::create_test_db;

    fn make_store() -> GraphStore {
        create_test_db().expect("create test db")
    }

    #[test]
    fn latest_vector_worker_fault_returns_none_when_table_empty() {
        let store = make_store();
        let res = store.latest_vector_worker_fault("vector").unwrap();
        assert!(res.is_none(), "empty table → None ; got {res:?}");
    }

    #[test]
    fn latest_vector_worker_fault_returns_most_recent_per_lane() {
        let store = make_store();
        let insert = |id: &str, occurred: i64| {
            format!(
                "INSERT INTO axon_runtime.VectorWorkerFault \
                 (fault_id, lane, worker_id, fatal_stage, fatal_reason_raw, fatal_class, provider, batch_id, texts_count, input_bytes, vram_used_mb, occurred_at_ms, restart_attempt) \
                 VALUES ('{id}', 'vector', 1, 'stage_b2', 'demo', 'demo', 'cpu', 'b-x', 4, 1024, 0, {occurred}, 0)"
            )
        };
        store.execute(&insert("f-old", 10)).unwrap();
        store.execute(&insert("f-new", 20)).unwrap();
        let res = store
            .latest_vector_worker_fault("vector")
            .unwrap()
            .expect("fault present");
        assert_eq!(res.fault_id, "f-new");
        assert_eq!(res.occurred_at_ms, 20);
    }

    #[test]
    fn latest_vector_worker_fault_scopes_by_lane() {
        let store = make_store();
        let insert = |id: &str, lane: &str, occurred: i64| {
            format!(
                "INSERT INTO axon_runtime.VectorWorkerFault \
                 (fault_id, lane, worker_id, fatal_stage, fatal_reason_raw, fatal_class, provider, batch_id, texts_count, input_bytes, vram_used_mb, occurred_at_ms, restart_attempt) \
                 VALUES ('{id}', '{lane}', 1, 'stage_b2', 'demo', 'demo', 'cpu', 'b-x', 4, 1024, 0, {occurred}, 0)"
            )
        };
        store.execute(&insert("f-vector", "vector", 10)).unwrap();
        store.execute(&insert("f-graph", "graph", 20)).unwrap();
        let vector_fault = store
            .latest_vector_worker_fault("vector")
            .unwrap()
            .expect("vector lane fault present");
        assert_eq!(vector_fault.fault_id, "f-vector");
        let graph_fault = store
            .latest_vector_worker_fault("graph")
            .unwrap()
            .expect("graph lane fault present");
        assert_eq!(graph_fault.fault_id, "f-graph");
    }

    #[test]
    fn vector_lane_state_record_returns_none_when_empty() {
        let store = make_store();
        let res = store.vector_lane_state_record("vector").unwrap();
        assert!(res.is_none());
    }

    #[test]
    fn vector_lane_state_record_round_trip() {
        let store = make_store();
        store
            .execute(
                "INSERT INTO axon_runtime.VectorLaneState \
                 (lane, state, reason, updated_at_ms, worker_id, restart_attempt, last_success_at_ms, last_fault_id) \
                 VALUES ('vector', 'running', NULL, 1, 1, 0, NULL, NULL)",
            )
            .unwrap();
        let res = store
            .vector_lane_state_record("vector")
            .unwrap()
            .expect("lane present");
        assert_eq!(res.lane, "vector");
        assert_eq!(res.state, "running");
        assert_eq!(res.worker_id, Some(1));
    }
}
