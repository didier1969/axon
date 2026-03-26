use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum BridgeEvent {
    SystemReady { start_time_utc: String },
    ScanStarted { total_files: usize },
    ProjectScanStarted { project: String, total_files: usize },
    FileIndexed { 
        path: String, 
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
    ScanComplete { total_files: usize, duration_ms: u64 },
    Heartbeat,
}
