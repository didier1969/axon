use crossbeam_channel::{bounded, Sender, Receiver};
use tracing::{info, info_span};

#[derive(Debug, Clone)]
pub struct Task {
    pub path: String,
    pub trace_id: String,
    pub t0: i64,
    pub t1: i64,
    pub t2: i64,
}

pub struct QueueStore {
    priority_sender: Sender<Task>,
    priority_receiver: Receiver<Task>,
    bulk_sender: Sender<Task>,
    bulk_receiver: Receiver<Task>,
}

impl QueueStore {
    pub fn new(capacity: usize) -> Self {
        // We split capacity: 20% for priority, 80% for bulk
        let prio_cap = capacity / 5;
        let bulk_cap = capacity - prio_cap;
        let (ps, pr) = bounded(prio_cap);
        let (bs, br) = bounded(bulk_cap);
        Self { 
            priority_sender: ps, 
            priority_receiver: pr,
            bulk_sender: bs,
            bulk_receiver: br
        }
    }

    pub fn push(&self, path: &str, _mtime: i64, trace_id: &str, t0: i64, t1: i64, priority: bool) -> Result<(), String> {
        let t2 = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_micros() as i64;
        let task = Task {
            path: path.to_string(),
            trace_id: trace_id.to_string(),
            t0, t1, t2
        };
        
        if priority {
            self.priority_sender.send(task).map_err(|e| format!("Priority Channel full: {}", e))
        } else {
            self.bulk_sender.send(task).map_err(|e| format!("Bulk Channel full: {}", e))
        }
    }

    pub fn pop(&self) -> Option<Task> {
        // ALWAYS try priority first (Non-blocking check)
        if let Ok(task) = self.priority_receiver.try_recv() {
            return Some(task);
        }
        
        // If no priority, block on any channel
        // Since crossbeam-channel doesn't easily select-block with priority, 
        // we'll use a simple loop with a small backoff or just drain bulk.
        self.bulk_receiver.recv().ok()
    }

    pub fn try_pop(&self) -> Option<Task> {
        self.priority_receiver.try_recv().or_else(|_| self.bulk_receiver.try_recv()).ok()
    }

    pub fn mark_done(&self, _path: &str) -> Result<(), String> {
        Ok(())
    }

    pub fn purge_all(&self) -> Result<(), String> {
        let _span = info_span!("queue_purge").entered();
        while self.priority_receiver.try_recv().is_ok() {}
        while self.bulk_receiver.try_recv().is_ok() {}
        info!("RAM Queues entirely purged for rescan.");
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.priority_sender.len() + self.bulk_sender.len()
    }
}