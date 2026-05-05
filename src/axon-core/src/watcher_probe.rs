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

    // REQ-AXO-185 #4: filtered/ignored_directory_event is high-volume noise.
    // Keep it queryable via `recent()` ring buffer, but log at debug level so
    // INFO-level capture remains useful for diagnosing the pipeline.
    if detail.contains("ignored_directory_event") || checkpoint == "watcher.filtered" {
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
