use std::sync::Arc;

use axon_core::graph::GraphStore;
use axon_core::queue::QueueStore;
use tracing::{debug, error, info};

pub(crate) fn start_memory_watchdog() {
    std::thread::spawn(|| {
        let page_size = 4096;
        let limit_bytes: u64 = 14 * 1024 * 1024 * 1024;
        loop {
            if let Ok(content) = std::fs::read_to_string("/proc/self/statm") {
                if let Some(rss_pages) = parse_rss_from_statm(&content) {
                    let rss_bytes = rss_pages * page_size;
                    if rss_bytes > limit_bytes {
                        error!(
                            "CRITICAL: Memory threshold reached ({} GB). Suicide for recycling...",
                            rss_bytes / 1024 / 1024 / 1024
                        );
                        std::process::exit(0);
                    }
                }
            }
            std::thread::sleep(std::time::Duration::from_secs(10));
        }
    });
}

pub(crate) fn spawn_autonomous_ingestor(
    store: Arc<GraphStore>,
    queue: Arc<QueueStore>,
) {
    tokio::spawn(async move {
        info!("Autonomous Ingestor: Ignition. Monitoring DuckDB for work...");
        loop {
            if queue.len() < 5000 {
                if let Ok(files) = store.fetch_pending_batch(2000) {
                    if !files.is_empty() {
                        debug!("Autonomous Ingestor: Feeding {} tasks to workers.", files.len());
                        for f in files {
                            let _ = queue.push(&f.path, 0, &f.trace_id, 0, 0, false);
                        }
                    }
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    });
}

pub(crate) fn spawn_initial_scan(store: Arc<GraphStore>, projects_root: String) {
    std::thread::spawn(move || {
        info!("🚀 Auto-Ignition: Beginning initial workspace mapping...");
        axon_core::scanner::Scanner::new(&projects_root).scan(store);
        info!("✅ Auto-Ignition: Initial mapping sequence complete.");
    });
}

fn parse_rss_from_statm(content: &str) -> Option<u64> {
    content.split_whitespace().nth(1).and_then(|s| s.parse::<u64>().ok())
}
