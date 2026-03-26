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
    sender: Sender<Task>,
    receiver: Receiver<Task>,
}

impl QueueStore {
    pub fn new(capacity: usize) -> Self {
        let (sender, receiver) = bounded(capacity);
        Self { sender, receiver }
    }

    pub fn push(&self, path: &str, _mtime: i64, trace_id: &str, t0: i64, t1: i64) -> Result<(), String> {
        let t2 = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_micros() as i64;
        let task = Task {
            path: path.to_string(),
            trace_id: trace_id.to_string(),
            t0, t1, t2
        };
        self.sender.send(task).map_err(|e| format!("Channel full or dead: {}", e))
    }

    pub fn pop(&self) -> Option<Task> {
        let _span = info_span!("queue_pop").entered();
        self.receiver.recv().ok()
    }

    pub fn try_pop(&self) -> Option<Task> {
        self.receiver.try_recv().ok()
    }

    // Le statut est maintenant géré uniquement par Oban (Elixir)
    pub fn mark_done(&self, _path: &str) -> Result<(), String> {
        Ok(())
    }

    pub fn purge_all(&self) -> Result<(), String> {
        let _span = info_span!("queue_purge").entered();
        while self.receiver.try_recv().is_ok() {}
        info!("RAM Queue entirely purged for rescan.");
        Ok(())
    }
}