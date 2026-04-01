// Copyright (c) Didier Stadelmann. All rights reserved.

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum BridgeEvent {
    SystemReady { start_time_utc: String },
    ScanStarted { total_files: usize },
    ProjectScanStarted { project: String, total_files: usize },
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
        queue_depth: usize,
        claim_mode: String,
        service_pressure: String,
        oversized_refusals_total: u64,
        degraded_mode_entries_total: u64,
    },
    ScanComplete { total_files: usize, duration_ms: u64 },
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
            queue_depth: 3,
            claim_mode: "guarded".to_string(),
            service_pressure: "degraded".to_string(),
            oversized_refusals_total: 7,
            degraded_mode_entries_total: 3,
        };

        let json = serde_json::to_string(&payload).expect("bridge event serializes");

        assert!(json.contains("\"RuntimeTelemetry\""));
        assert!(json.contains("\"budget_bytes\":1024"));
        assert!(json.contains("\"reserved_bytes\":256"));
        assert!(json.contains("\"exhaustion_ratio\":0.25"));
        assert!(json.contains("\"queue_depth\":3"));
        assert!(json.contains("\"claim_mode\":\"guarded\""));
        assert!(json.contains("\"service_pressure\":\"degraded\""));
        assert!(json.contains("\"oversized_refusals_total\":7"));
        assert!(json.contains("\"degraded_mode_entries_total\":3"));
    }
}
