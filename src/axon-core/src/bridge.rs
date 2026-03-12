use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum BridgeEvent {
    ScanStarted { total_files: usize },
    FileIndexed { 
        path: String, 
        symbol_count: usize,
        security_score: usize,
        coverage_score: usize,
    },
    ScanComplete { total_files: usize, duration_ms: u64 },
    Heartbeat,
}
