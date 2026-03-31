use crossbeam_channel::{bounded, select_biased, Receiver, Sender, TrySendError};
use tracing::{info, info_span};

const TITAN_FILE_SIZE_BYTES: u64 = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskLane {
    Hot,
    Bulk,
    Titan,
}

#[derive(Debug, Clone)]
pub struct Task {
    pub path: String,
    pub trace_id: String,
    pub lane: TaskLane,
    pub t0: i64,
    pub t1: i64,
    pub t2: i64,
}

pub struct QueueStore {
    priority_sender: Sender<Task>,
    priority_receiver: Receiver<Task>,
    bulk_sender: Sender<Task>,
    bulk_receiver: Receiver<Task>,
    titan_sender: Sender<Task>,
    titan_receiver: Receiver<Task>,
}

impl QueueStore {
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(3);
        let prio_cap = (capacity / 5).max(1);
        let titan_cap = (capacity / 5).max(1);
        let bulk_cap = (capacity - prio_cap - titan_cap).max(1);
        let (ps, pr) = bounded(prio_cap);
        let (bs, br) = bounded(bulk_cap);
        let (ts, tr) = bounded(titan_cap);
        Self { 
            priority_sender: ps, 
            priority_receiver: pr,
            bulk_sender: bs,
            bulk_receiver: br,
            titan_sender: ts,
            titan_receiver: tr,
        }
    }

    pub fn push(&self, path: &str, _mtime: i64, trace_id: &str, t0: i64, t1: i64, priority: bool) -> Result<(), String> {
        let t2 = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_micros() as i64;
        let lane = if priority {
            TaskLane::Hot
        } else if std::fs::metadata(path)
            .map(|metadata| metadata.len() > TITAN_FILE_SIZE_BYTES)
            .unwrap_or(false)
        {
            TaskLane::Titan
        } else {
            TaskLane::Bulk
        };
        let task = Task {
            path: path.to_string(),
            trace_id: trace_id.to_string(),
            lane,
            t0, t1, t2
        };
        
        match lane {
            TaskLane::Hot => self
                .priority_sender
                .try_send(task)
                .map_err(|e| format_channel_send_error("Priority", e)),
            TaskLane::Bulk => self
                .bulk_sender
                .try_send(task)
                .map_err(|e| format_channel_send_error("Bulk", e)),
            TaskLane::Titan => self
                .titan_sender
                .try_send(task)
                .map_err(|e| format_channel_send_error("Titan", e)),
        }
    }

    pub fn pop(&self) -> Option<Task> {
        if let Ok(task) = self.priority_receiver.try_recv() {
            return Some(task);
        }
        if let Ok(task) = self.bulk_receiver.try_recv() {
            return Some(task);
        }
        if let Ok(task) = self.titan_receiver.try_recv() {
            return Some(task);
        }

        select_biased! {
            recv(self.priority_receiver) -> task => task.ok(),
            recv(self.bulk_receiver) -> task => task.ok(),
            recv(self.titan_receiver) -> task => task.ok(),
        }
    }

    pub fn try_pop(&self) -> Option<Task> {
        self.priority_receiver
            .try_recv()
            .or_else(|_| self.bulk_receiver.try_recv())
            .or_else(|_| self.titan_receiver.try_recv())
            .ok()
    }

    pub fn mark_done(&self, _path: &str) -> Result<(), String> {
        Ok(())
    }

    pub fn purge_all(&self) -> Result<(), String> {
        let _span = info_span!("queue_purge").entered();
        while self.priority_receiver.try_recv().is_ok() {}
        while self.bulk_receiver.try_recv().is_ok() {}
        while self.titan_receiver.try_recv().is_ok() {}
        info!("RAM Queues entirely purged for rescan.");
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.priority_sender.len() + self.bulk_sender.len() + self.titan_sender.len()
    }

    pub fn common_len(&self) -> usize {
        self.priority_sender.len() + self.bulk_sender.len()
    }
}

fn format_channel_send_error(channel: &str, err: TrySendError<Task>) -> String {
    match err {
        TrySendError::Full(_) => format!("{} Channel full", channel),
        TrySendError::Disconnected(_) => format!("{} Channel disconnected", channel),
    }
}

#[cfg(test)]
mod tests {
    use super::QueueStore;

    #[test]
    fn test_hot_lane_never_starves_behind_bulk_work() {
        let queue = QueueStore::new(10);
        queue.push("/tmp/bulk_a.ex", 0, "bulk-a", 0, 0, false).unwrap();
        queue.push("/tmp/bulk_b.ex", 0, "bulk-b", 0, 0, false).unwrap();
        queue.push("/tmp/hot.ex", 0, "hot", 0, 0, true).unwrap();

        let first = queue.pop().expect("hot lane should be served first");
        assert_eq!(first.trace_id, "hot");
    }

    #[test]
    fn test_bulk_lane_hits_backpressure_before_hot_lane() {
        let queue = QueueStore::new(10);

        for idx in 0..6 {
            queue
                .push(&format!("/tmp/bulk_{}.ex", idx), 0, &format!("bulk-{}", idx), 0, 0, false)
                .unwrap();
        }

        let overflow = queue.push("/tmp/bulk_overflow.ex", 0, "bulk-overflow", 0, 0, false);
        assert!(overflow.is_err(), "bulk lane should saturate before borrowing hot capacity");

        queue
            .push("/tmp/hot_reserved.ex", 0, "hot-reserved", 0, 0, true)
            .expect("hot lane must retain reserved capacity under bulk pressure");
    }

    #[test]
    fn test_titan_lane_isolated_from_common_lane() {
        let temp = tempfile::tempdir().unwrap();
        let titan_path = temp.path().join("titan.rs");
        std::fs::write(&titan_path, vec![b'x'; 300 * 1024]).unwrap();

        let queue = QueueStore::new(10);
        queue
            .push(titan_path.to_string_lossy().as_ref(), 0, "titan", 0, 0, false)
            .unwrap();
        queue.push("/tmp/bulk_regular.ex", 0, "bulk", 0, 0, false).unwrap();

        let first = queue.pop().expect("bulk lane should drain before titan");
        assert_eq!(first.trace_id, "bulk");

        let second = queue.pop().expect("titan lane should still be available after bulk");
        assert_eq!(second.trace_id, "titan");
    }
}
