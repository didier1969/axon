use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use tracing::{debug, info};

const MAX_EVENTS: usize = 512;

fn buffer() -> &'static Mutex<VecDeque<String>> {
    static BUFFER: OnceLock<Mutex<VecDeque<String>>> = OnceLock::new();
    BUFFER.get_or_init(|| Mutex::new(VecDeque::with_capacity(MAX_EVENTS)))
}

pub fn record(checkpoint: &str, path: Option<&Path>, detail: impl Into<String>) {
    let detail = detail.into();
    let path = path
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "-".to_string());
    let line = format!("checkpoint={} path={} detail={}", checkpoint, path, detail);

    if let Ok(mut guard) = buffer().lock() {
        if guard.len() >= MAX_EVENTS {
            guard.pop_front();
        }
        guard.push_back(line.clone());
    }

    // REQ-AXO-185 #4 + REQ-AXO-331: high-volume checkpoints are kept in the
    // `recent()` ring buffer (operator-queryable) but emitted at DEBUG so a
    // running indexer does not flood the log file. INFO is reserved for
    // checkpoints that signal actual work (staging, reconcile, rescan, errors).
    let is_high_volume = matches!(
        checkpoint,
        "watcher.filtered"
            | "watcher.received"
            | "watcher.buffered_batch"
            | "watcher.buffered_none"
            | "watcher.buffered_subtree_hint"
            | "watcher.buffered_tombstone"
            | "watcher.control_file"
    ) || detail.contains("ignored_directory_event");

    if is_high_volume {
        debug!("WatcherProbe {}", line);
    } else {
        info!("WatcherProbe {}", line);
    }
}

#[allow(dead_code)]
pub fn clear() {
    if let Ok(mut guard) = buffer().lock() {
        guard.clear();
    }
}

#[allow(dead_code)]
pub fn recent() -> Vec<String> {
    buffer()
        .lock()
        .map(|guard| guard.iter().cloned().collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{clear, recent, record};
    use std::path::Path;

    #[test]
    fn test_record_keeps_recent_probe_events() {
        clear();
        record(
            "watcher.received",
            Some(Path::new("/tmp/probe.ex")),
            "count=1",
        );
        let events = recent();
        assert_eq!(events.len(), 1);
        assert!(events[0].contains("checkpoint=watcher.received"));
        assert!(events[0].contains("/tmp/probe.ex"));
    }
}
