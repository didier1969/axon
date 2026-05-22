// Copyright (c) Didier Stadelmann. All rights reserved.

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct RuntimeTruthFeed {
    pub stale: bool,
    pub observed_age_ms: Option<u64>,
    pub stale_after_ms: u64,
    pub last_heartbeat_at_ms: Option<u64>,
    pub last_good_payload_at_ms: Option<u64>,
    pub degraded_reason: Option<String>,
}

impl RuntimeTruthFeed {
    pub const DEFAULT_STALE_AFTER_MS: u64 = 5_000;

    pub fn from_last_heartbeat_ms(now_ms: u64, last_heartbeat_at_ms: u64) -> Self {
        Self::from_observed_times(
            now_ms,
            Some(last_heartbeat_at_ms),
            Some(last_heartbeat_at_ms),
            Self::DEFAULT_STALE_AFTER_MS,
            None::<String>,
        )
    }

    pub fn from_observed_times(
        now_ms: u64,
        last_heartbeat_at_ms: Option<u64>,
        last_good_payload_at_ms: Option<u64>,
        stale_after_ms: u64,
        degraded_reason: Option<impl Into<String>>,
    ) -> Self {
        let observed_age_ms = last_heartbeat_at_ms.map(|at| now_ms.saturating_sub(at));
        let stale = match observed_age_ms {
            Some(age) => age > stale_after_ms,
            None => true,
        };
        let degraded_reason = if stale {
            Some(
                degraded_reason
                    .map(Into::into)
                    .unwrap_or_else(|| "missing_runtime_truth_heartbeat".to_string()),
            )
        } else {
            degraded_reason.map(Into::into)
        };

        Self {
            stale,
            observed_age_ms,
            stale_after_ms,
            last_heartbeat_at_ms,
            last_good_payload_at_ms,
            degraded_reason,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[allow(clippy::large_enum_variant)]
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
        telemetry_source: String,
        telemetry_process_role: String,
        telemetry_freshness_state: String,
        telemetry_observed_age_ms: Option<u64>,
        telemetry_degraded_reason: Option<String>,
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
        interactive_priority_active: bool,
        interactive_priority_level: String,
        interactive_requests_in_flight: u64,
        oversized_refusals_total: u64,
        degraded_mode_entries_total: u64,
        background_launches_suppressed_total: u64,
        vectorization_suppressed_due_to_interactive: u64,
        vectorization_interrupted_due_to_interactive: u64,
        vectorization_requeued_for_interactive: u64,
        vectorization_resumed_after_interactive: u64,
        projection_suppressed_due_to_interactive: u64,
        guard_hits: u64,
        guard_misses: u64,
        guard_bypassed_total: u64,
        guard_hydrated_entries: u64,
        guard_hydration_duration_ms: u64,
        ingress_enabled: bool,
        ingress_buffered_entries: usize,
        ingress_subtree_hints: usize,
        ingress_subtree_hint_in_flight: usize,
        ingress_subtree_hint_accepted_total: u64,
        ingress_subtree_hint_blocked_total: u64,
        ingress_subtree_hint_suppressed_total: u64,
        ingress_subtree_hint_productive_total: u64,
        ingress_subtree_hint_unproductive_total: u64,
        ingress_subtree_hint_dropped_total: u64,
        ingress_collapsed_total: u64,
        ingress_flush_count: u64,
        ingress_last_flush_duration_ms: u64,
        ingress_last_promoted_count: u64,
        ingress_promoted_total: u64,
        ingress_last_durably_persisted_count: u64,
        ingress_durably_persisted_total: u64,
        ingress_last_excluded_from_pending_count: u64,
        ingress_excluded_from_pending_total: u64,
        memory_trim_attempts_total: u64,
        memory_trim_successes_total: u64,
        cpu_load: f64,
        ram_load: f64,
        io_wait: f64,
        host_state: String,
        host_guidance_slots: usize,
        rss_bytes: u64,
        rss_anon_bytes: u64,
        rss_file_bytes: u64,
        rss_shmem_bytes: u64,
        // REQ-AXO-284 Slice 2 — PG health metrics. `Option` so a transient
        // catalog miss doesn't poison the payload.
        pg_database_bytes: Option<i64>,
        pg_chunkembedding_total_bytes: Option<i64>,
        pg_wal_bytes: Option<i64>,
        pg_buffer_hit_ratio: Option<f64>,
        vector_chunks_embedded_total: u64,
        chunk_embeddings_per_second: f64,
        chunk_embeddings_rate_window_ms: u64,
        prepare_inflight_chunks_current: u64,
        ready_queue_chunks_current: u64,
        ready_queue_chunks_small: u64,
        ready_queue_chunks_medium: u64,
        ready_queue_chunks_large: u64,
        ready_batches_small: u64,
        ready_batches_medium: u64,
        ready_batches_large: u64,
        mixed_fallback_batches_total: u64,
        homogeneous_batches_total: u64,
        last_consumed_batch_lane: String,
        active_small_max_tokens: u64,
        active_medium_max_tokens: u64,
        last_embed_attempt_wall_ms: u64,
        avg_embed_attempt_wall_ms: f64,
        max_embed_attempt_wall_ms: u64,
        last_embed_gap_ms: u64,
        avg_embed_gap_ms: f64,
        max_embed_gap_ms: u64,
        graph_workers_started_total: u64,
        graph_workers_active_current: u64,
        graph_worker_heartbeat_at_ms: u64,
        runtime_truth_feed: RuntimeTruthFeed,
        #[serde(skip_serializing_if = "Option::is_none")]
        projected_indexer_runtime: Option<serde_json::Value>,
    },
    ScanComplete {
        total_files: usize,
        duration_ms: u64,
    },
    Heartbeat,
}

#[cfg(test)]
mod tests {
    use super::{BridgeEvent, RuntimeTruthFeed};

    #[test]
    fn runtime_truth_feed_marks_missing_heartbeat_stale() {
        let feed = RuntimeTruthFeed::from_last_heartbeat_ms(10_000, 2_000);
        assert!(feed.stale);
    }

    #[test]
    fn runtime_telemetry_bridge_event_serializes_with_expected_shape() {
        let payload = BridgeEvent::RuntimeTelemetry {
            telemetry_source: "local_runtime".to_string(),
            telemetry_process_role: "indexer".to_string(),
            telemetry_freshness_state: "degraded".to_string(),
            telemetry_observed_age_ms: Some(500),
            telemetry_degraded_reason: Some("indexer_feed_degraded".to_string()),
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
            interactive_priority_active: true,
            interactive_priority_level: "interactive_priority".to_string(),
            interactive_requests_in_flight: 2,
            oversized_refusals_total: 7,
            degraded_mode_entries_total: 3,
            background_launches_suppressed_total: 4,
            vectorization_suppressed_due_to_interactive: 5,
            vectorization_interrupted_due_to_interactive: 7,
            vectorization_requeued_for_interactive: 7,
            vectorization_resumed_after_interactive: 3,
            projection_suppressed_due_to_interactive: 6,
            guard_hits: 9,
            guard_misses: 4,
            guard_bypassed_total: 2,
            guard_hydrated_entries: 512,
            guard_hydration_duration_ms: 18,
            ingress_enabled: true,
            ingress_buffered_entries: 12,
            ingress_subtree_hints: 2,
            ingress_subtree_hint_in_flight: 1,
            ingress_subtree_hint_accepted_total: 15,
            ingress_subtree_hint_blocked_total: 4,
            ingress_subtree_hint_suppressed_total: 2,
            ingress_subtree_hint_productive_total: 9,
            ingress_subtree_hint_unproductive_total: 6,
            ingress_subtree_hint_dropped_total: 3,
            ingress_collapsed_total: 19,
            ingress_flush_count: 5,
            ingress_last_flush_duration_ms: 44,
            ingress_last_promoted_count: 8,
            ingress_promoted_total: 64,
            ingress_last_durably_persisted_count: 3,
            ingress_durably_persisted_total: 58,
            ingress_last_excluded_from_pending_count: 1,
            ingress_excluded_from_pending_total: 7,
            memory_trim_attempts_total: 11,
            memory_trim_successes_total: 5,
            cpu_load: 61.5,
            ram_load: 47.0,
            io_wait: 12.2,
            host_state: "constrained".to_string(),
            host_guidance_slots: 2,
            rss_bytes: 7_340,
            rss_anon_bytes: 5_120,
            rss_file_bytes: 1_920,
            rss_shmem_bytes: 300,
            pg_database_bytes: Some(8_589_934_592),
            pg_chunkembedding_total_bytes: Some(2_147_483_648),
            pg_wal_bytes: Some(1_073_741_824),
            pg_buffer_hit_ratio: Some(0.987),
            vector_chunks_embedded_total: 96,
            chunk_embeddings_per_second: 32.0,
            chunk_embeddings_rate_window_ms: 5_000,
            prepare_inflight_chunks_current: 24,
            ready_queue_chunks_current: 72,
            ready_queue_chunks_small: 8,
            ready_queue_chunks_medium: 24,
            ready_queue_chunks_large: 40,
            ready_batches_small: 1,
            ready_batches_medium: 2,
            ready_batches_large: 3,
            mixed_fallback_batches_total: 2,
            homogeneous_batches_total: 14,
            last_consumed_batch_lane: "large".to_string(),
            active_small_max_tokens: 96,
            active_medium_max_tokens: 224,
            last_embed_attempt_wall_ms: 84,
            avg_embed_attempt_wall_ms: 52.5,
            max_embed_attempt_wall_ms: 120,
            last_embed_gap_ms: 640,
            avg_embed_gap_ms: 410.0,
            max_embed_gap_ms: 1_250,
            graph_workers_started_total: 2,
            graph_workers_active_current: 2,
            graph_worker_heartbeat_at_ms: 9_750,
            runtime_truth_feed: RuntimeTruthFeed::from_observed_times(
                10_000,
                Some(9_500),
                Some(9_400),
                RuntimeTruthFeed::DEFAULT_STALE_AFTER_MS,
                Some("indexer_feed_degraded"),
            ),
            projected_indexer_runtime: Some(serde_json::json!({
                "available": true,
                "telemetry_source": "indexer_peer_heartbeat",
                "process_role": "indexer",
                "freshness_state": "fresh",
                "observed_age_ms": 125,
                "telemetry": {
                    "ingress_buffered_entries": 33
                }
            })),
        };

        let json = serde_json::to_string(&payload).expect("bridge event serializes");

        assert!(json.contains("\"RuntimeTelemetry\""));
        assert!(json.contains("\"telemetry_source\":\"local_runtime\""));
        assert!(json.contains("\"telemetry_process_role\":\"indexer\""));
        assert!(json.contains("\"telemetry_freshness_state\":\"degraded\""));
        assert!(json.contains("\"telemetry_observed_age_ms\":500"));
        assert!(json.contains("\"telemetry_degraded_reason\":\"indexer_feed_degraded\""));
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
        assert!(json.contains("\"interactive_priority_active\":true"));
        assert!(json.contains("\"interactive_priority_level\":\"interactive_priority\""));
        assert!(json.contains("\"interactive_requests_in_flight\":2"));
        assert!(json.contains("\"oversized_refusals_total\":7"));
        assert!(json.contains("\"degraded_mode_entries_total\":3"));
        assert!(json.contains("\"background_launches_suppressed_total\":4"));
        assert!(json.contains("\"vectorization_suppressed_due_to_interactive\":5"));
        assert!(json.contains("\"vectorization_interrupted_due_to_interactive\":7"));
        assert!(json.contains("\"vectorization_requeued_for_interactive\":7"));
        assert!(json.contains("\"vectorization_resumed_after_interactive\":3"));
        assert!(json.contains("\"projection_suppressed_due_to_interactive\":6"));
        assert!(json.contains("\"guard_hits\":9"));
        assert!(json.contains("\"guard_misses\":4"));
        assert!(json.contains("\"guard_bypassed_total\":2"));
        assert!(json.contains("\"guard_hydrated_entries\":512"));
        assert!(json.contains("\"guard_hydration_duration_ms\":18"));
        assert!(json.contains("\"ingress_enabled\":true"));
        assert!(json.contains("\"ingress_buffered_entries\":12"));
        assert!(json.contains("\"ingress_subtree_hints\":2"));
        assert!(json.contains("\"ingress_subtree_hint_in_flight\":1"));
        assert!(json.contains("\"ingress_subtree_hint_accepted_total\":15"));
        assert!(json.contains("\"ingress_subtree_hint_blocked_total\":4"));
        assert!(json.contains("\"ingress_subtree_hint_suppressed_total\":2"));
        assert!(json.contains("\"ingress_subtree_hint_productive_total\":9"));
        assert!(json.contains("\"ingress_subtree_hint_unproductive_total\":6"));
        assert!(json.contains("\"ingress_subtree_hint_dropped_total\":3"));
        assert!(json.contains("\"ingress_collapsed_total\":19"));
        assert!(json.contains("\"ingress_flush_count\":5"));
        assert!(json.contains("\"ingress_last_flush_duration_ms\":44"));
        assert!(json.contains("\"ingress_last_promoted_count\":8"));
        assert!(json.contains("\"ingress_promoted_total\":64"));
        assert!(json.contains("\"ingress_last_durably_persisted_count\":3"));
        assert!(json.contains("\"ingress_durably_persisted_total\":58"));
        assert!(json.contains("\"ingress_last_excluded_from_pending_count\":1"));
        assert!(json.contains("\"ingress_excluded_from_pending_total\":7"));
        assert!(json.contains("\"memory_trim_attempts_total\":11"));
        assert!(json.contains("\"memory_trim_successes_total\":5"));
        assert!(json.contains("\"cpu_load\":61.5"));
        assert!(json.contains("\"ram_load\":47.0"));
        assert!(json.contains("\"io_wait\":12.2"));
        assert!(json.contains("\"host_state\":\"constrained\""));
        assert!(json.contains("\"host_guidance_slots\":2"));
        assert!(json.contains("\"rss_bytes\":7340"));
        assert!(json.contains("\"rss_anon_bytes\":5120"));
        assert!(json.contains("\"rss_file_bytes\":1920"));
        assert!(json.contains("\"rss_shmem_bytes\":300"));
        assert!(json.contains("\"pg_database_bytes\":8589934592"));
        assert!(json.contains("\"pg_chunkembedding_total_bytes\":2147483648"));
        assert!(json.contains("\"pg_wal_bytes\":1073741824"));
        assert!(json.contains("\"pg_buffer_hit_ratio\":0.987"));
        assert!(json.contains("\"vector_chunks_embedded_total\":96"));
        assert!(json.contains("\"chunk_embeddings_per_second\":32.0"));
        assert!(json.contains("\"chunk_embeddings_rate_window_ms\":5000"));
        assert!(json.contains("\"ready_queue_chunks_small\":8"));
        assert!(json.contains("\"ready_batches_large\":3"));
        assert!(json.contains("\"mixed_fallback_batches_total\":2"));
        assert!(json.contains("\"last_consumed_batch_lane\":\"large\""));
        assert!(json.contains("\"graph_workers_started_total\":2"));
        assert!(json.contains("\"graph_workers_active_current\":2"));
        assert!(json.contains("\"runtime_truth_feed\""));
        assert!(json.contains("\"last_good_payload_at_ms\":9400"));
        assert!(json.contains("\"projected_indexer_runtime\""));
        assert!(json.contains("\"telemetry_source\":\"indexer_peer_heartbeat\""));
    }
}
