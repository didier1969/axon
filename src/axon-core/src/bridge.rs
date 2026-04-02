// Copyright (c) Didier Stadelmann. All rights reserved.

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum BridgeEvent {
    SystemReady {
        start_time_utc: String,
    },
    ScanStarted {
        total_files: usize,
    },
    ProjectScanStarted {
        project: String,
        total_files: usize,
    },
    FileIndexed {
        path: String,
        status: String,
        error_reason: String,
        symbol_count: usize,
        relation_count: usize,
        file_count: usize,
        entry_points: usize,
        security_score: usize,
        coverage_score: usize,
        #[serde(default)]
        taint_paths: String,
        trace_id: String,
        t0: i64,
        t1: i64,
        t2: i64,
        t3: i64,
        t4: i64,
    },
    RuntimeTelemetry {
        budget_bytes: u64,
        reserved_bytes: u64,
        exhaustion_ratio: f64,
        reserved_task_count: usize,
        anonymous_trace_reserved_tasks: usize,
        anonymous_trace_admissions_total: u64,
        reservation_release_misses_total: u64,
        queue_depth: usize,
        claim_mode: String,
        service_pressure: String,
        oversized_refusals_total: u64,
        degraded_mode_entries_total: u64,
        guard_hits: u64,
        guard_misses: u64,
        guard_bypassed_total: u64,
        guard_hydrated_entries: u64,
        guard_hydration_duration_ms: u64,
        ingress_enabled: bool,
        ingress_buffered_entries: usize,
        ingress_subtree_hints: usize,
        ingress_collapsed_total: u64,
        ingress_flush_count: u64,
        ingress_last_flush_duration_ms: u64,
        ingress_last_promoted_count: u64,
        cpu_load: f64,
        ram_load: f64,
        io_wait: f64,
        host_state: String,
        host_guidance_slots: usize,
        rss_bytes: u64,
        rss_anon_bytes: u64,
        rss_file_bytes: u64,
        rss_shmem_bytes: u64,
        db_file_bytes: u64,
        db_wal_bytes: u64,
        db_total_bytes: u64,
        duckdb_memory_bytes: u64,
        duckdb_temporary_bytes: u64,
    },
    ScanComplete {
        total_files: usize,
        duration_ms: u64,
    },
    Heartbeat,
}

#[cfg(test)]
mod tests {
    use super::BridgeEvent;

    #[test]
    fn runtime_telemetry_bridge_event_serializes_with_expected_shape() {
        let payload = BridgeEvent::RuntimeTelemetry {
            budget_bytes: 1_024,
            reserved_bytes: 256,
            exhaustion_ratio: 0.25,
            reserved_task_count: 2,
            anonymous_trace_reserved_tasks: 1,
            anonymous_trace_admissions_total: 9,
            reservation_release_misses_total: 3,
            queue_depth: 3,
            claim_mode: "guarded".to_string(),
            service_pressure: "degraded".to_string(),
            oversized_refusals_total: 7,
            degraded_mode_entries_total: 3,
            guard_hits: 9,
            guard_misses: 4,
            guard_bypassed_total: 2,
            guard_hydrated_entries: 512,
            guard_hydration_duration_ms: 18,
            ingress_enabled: true,
            ingress_buffered_entries: 12,
            ingress_subtree_hints: 2,
            ingress_collapsed_total: 19,
            ingress_flush_count: 5,
            ingress_last_flush_duration_ms: 44,
            ingress_last_promoted_count: 8,
            cpu_load: 61.5,
            ram_load: 47.0,
            io_wait: 12.2,
            host_state: "constrained".to_string(),
            host_guidance_slots: 2,
            rss_bytes: 7_340,
            rss_anon_bytes: 5_120,
            rss_file_bytes: 1_920,
            rss_shmem_bytes: 300,
            db_file_bytes: 4_096,
            db_wal_bytes: 512,
            db_total_bytes: 4_608,
            duckdb_memory_bytes: 2_048,
            duckdb_temporary_bytes: 256,
        };

        let json = serde_json::to_string(&payload).expect("bridge event serializes");

        assert!(json.contains("\"RuntimeTelemetry\""));
        assert!(json.contains("\"budget_bytes\":1024"));
        assert!(json.contains("\"reserved_bytes\":256"));
        assert!(json.contains("\"exhaustion_ratio\":0.25"));
        assert!(json.contains("\"reserved_task_count\":2"));
        assert!(json.contains("\"anonymous_trace_reserved_tasks\":1"));
        assert!(json.contains("\"anonymous_trace_admissions_total\":9"));
        assert!(json.contains("\"reservation_release_misses_total\":3"));
        assert!(json.contains("\"queue_depth\":3"));
        assert!(json.contains("\"claim_mode\":\"guarded\""));
        assert!(json.contains("\"service_pressure\":\"degraded\""));
        assert!(json.contains("\"oversized_refusals_total\":7"));
        assert!(json.contains("\"degraded_mode_entries_total\":3"));
        assert!(json.contains("\"guard_hits\":9"));
        assert!(json.contains("\"guard_misses\":4"));
        assert!(json.contains("\"guard_bypassed_total\":2"));
        assert!(json.contains("\"guard_hydrated_entries\":512"));
        assert!(json.contains("\"guard_hydration_duration_ms\":18"));
        assert!(json.contains("\"ingress_enabled\":true"));
        assert!(json.contains("\"ingress_buffered_entries\":12"));
        assert!(json.contains("\"ingress_subtree_hints\":2"));
        assert!(json.contains("\"ingress_collapsed_total\":19"));
        assert!(json.contains("\"ingress_flush_count\":5"));
        assert!(json.contains("\"ingress_last_flush_duration_ms\":44"));
        assert!(json.contains("\"ingress_last_promoted_count\":8"));
        assert!(json.contains("\"cpu_load\":61.5"));
        assert!(json.contains("\"ram_load\":47.0"));
        assert!(json.contains("\"io_wait\":12.2"));
        assert!(json.contains("\"host_state\":\"constrained\""));
        assert!(json.contains("\"host_guidance_slots\":2"));
        assert!(json.contains("\"rss_bytes\":7340"));
        assert!(json.contains("\"rss_anon_bytes\":5120"));
        assert!(json.contains("\"rss_file_bytes\":1920"));
        assert!(json.contains("\"rss_shmem_bytes\":300"));
        assert!(json.contains("\"db_file_bytes\":4096"));
        assert!(json.contains("\"db_wal_bytes\":512"));
        assert!(json.contains("\"db_total_bytes\":4608"));
        assert!(json.contains("\"duckdb_memory_bytes\":2048"));
        assert!(json.contains("\"duckdb_temporary_bytes\":256"));
    }
}
