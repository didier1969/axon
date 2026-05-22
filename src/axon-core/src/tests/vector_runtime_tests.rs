// REQ-AXO-901669 — coverage for LIVE vector_runtime methods.
//
// The `axon_runtime` schema (`VectorWorkerFault`, `VectorLaneState`, …) is
// already bootstrapped by `GraphStore::new` → `bootstrap_global_pg_schema`
// → `generate_global_schema` (which includes `db/ddl/02_axon_runtime.sql`
// via `include_str!`). The schema is therefore present when tests run.
//
// The real failure mode for the prior `#[ignore]` round-trip tests was
// **shared-PG contamination** : cargo runs `--lib` tests in parallel
// against the same dev PG instance, and `axon_runtime.*` tables also
// accumulate state from live/indexer runs. Hardcoded `lane = "vector"`
// labels collided both with sibling tests and with persisted telemetry.
//
// The fix : each test scopes its rows behind a unique lane label
// (`test_helpers::unique_test_scope`). Asserting emptiness, recency, and
// per-lane filtering then holds independently of other parallel tests
// and of any persisted live state.

#[cfg(test)]
mod tests {
    use crate::graph::GraphStore;
    use crate::tests::test_helpers::{create_test_db, unique_test_scope};

    fn make_store() -> GraphStore {
        create_test_db().expect("create test db")
    }

    #[test]
    fn latest_vector_worker_fault_returns_none_when_table_empty() {
        let store = make_store();
        let lane = unique_test_scope("vrt-empty");
        let res = store
            .latest_vector_worker_fault(&lane)
            .expect("query VectorWorkerFault");
        assert!(res.is_none(), "fresh lane → None ; got {res:?}");
    }

    #[test]
    fn latest_vector_worker_fault_returns_most_recent_per_lane() {
        let store = make_store();
        let lane = unique_test_scope("vrt-recent");
        let fault_old = format!("{lane}-old");
        let fault_new = format!("{lane}-new");
        let insert = |fault_id: &str, occurred: i64| {
            format!(
                "INSERT INTO axon_runtime.VectorWorkerFault \
                 (fault_id, lane, worker_id, fatal_stage, fatal_reason_raw, fatal_class, provider, batch_id, texts_count, input_bytes, vram_used_mb, occurred_at_ms, restart_attempt) \
                 VALUES ('{fault_id}', '{lane}', 1, 'stage_b2', 'demo', 'demo', 'cpu', 'b-x', 4, 1024, 0, {occurred}, 0)"
            )
        };
        store.execute(&insert(&fault_old, 10)).unwrap();
        store.execute(&insert(&fault_new, 20)).unwrap();
        let res = store
            .latest_vector_worker_fault(&lane)
            .unwrap()
            .expect("fault present");
        assert_eq!(res.fault_id, fault_new);
        assert_eq!(res.occurred_at_ms, 20);
    }

    #[test]
    fn latest_vector_worker_fault_scopes_by_lane() {
        let store = make_store();
        let lane_a = unique_test_scope("vrt-scope-a");
        let lane_b = unique_test_scope("vrt-scope-b");
        let fault_a = format!("{lane_a}-id");
        let fault_b = format!("{lane_b}-id");
        let insert = |fault_id: &str, lane: &str, occurred: i64| {
            format!(
                "INSERT INTO axon_runtime.VectorWorkerFault \
                 (fault_id, lane, worker_id, fatal_stage, fatal_reason_raw, fatal_class, provider, batch_id, texts_count, input_bytes, vram_used_mb, occurred_at_ms, restart_attempt) \
                 VALUES ('{fault_id}', '{lane}', 1, 'stage_b2', 'demo', 'demo', 'cpu', 'b-x', 4, 1024, 0, {occurred}, 0)"
            )
        };
        store.execute(&insert(&fault_a, &lane_a, 10)).unwrap();
        store.execute(&insert(&fault_b, &lane_b, 20)).unwrap();
        let fault_for_a = store
            .latest_vector_worker_fault(&lane_a)
            .unwrap()
            .expect("lane_a fault present");
        assert_eq!(fault_for_a.fault_id, fault_a);
        let fault_for_b = store
            .latest_vector_worker_fault(&lane_b)
            .unwrap()
            .expect("lane_b fault present");
        assert_eq!(fault_for_b.fault_id, fault_b);
    }

    #[test]
    fn vector_lane_state_record_returns_none_when_empty() {
        let store = make_store();
        let lane = unique_test_scope("vls-empty");
        let res = store
            .vector_lane_state_record(&lane)
            .expect("query VectorLaneState");
        assert!(res.is_none(), "fresh lane → None ; got {res:?}");
    }

    #[test]
    fn vector_lane_state_record_round_trip() {
        let store = make_store();
        let lane = unique_test_scope("vls-rt");
        store
            .execute(&format!(
                "INSERT INTO axon_runtime.VectorLaneState \
                 (lane, state, reason, updated_at_ms, worker_id, restart_attempt, last_success_at_ms, last_fault_id) \
                 VALUES ('{lane}', 'running', NULL, 1, 1, 0, NULL, NULL)"
            ))
            .unwrap();
        let res = store
            .vector_lane_state_record(&lane)
            .unwrap()
            .expect("lane present");
        assert_eq!(res.lane, lane);
        assert_eq!(res.state, "running");
        assert_eq!(res.worker_id, Some(1));
    }
}
