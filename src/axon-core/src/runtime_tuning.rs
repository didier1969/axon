use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeTuningState {
    pub vector_workers: usize,
    pub chunk_batch_size: usize,
    pub file_vectorization_batch_size: usize,
    pub vector_ready_queue_depth: usize,
    pub vector_persist_queue_bound: usize,
    pub vector_max_inflight_persists: usize,
    pub embed_micro_batch_max_items: usize,
    pub embed_micro_batch_max_total_tokens: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeTuningSnapshot {
    pub version: u64,
    pub state: RuntimeTuningState,
}

static RUNTIME_TUNING_SNAPSHOT: OnceLock<Mutex<Option<RuntimeTuningSnapshot>>> = OnceLock::new();

fn runtime_tuning_snapshot_slot() -> &'static Mutex<Option<RuntimeTuningSnapshot>> {
    RUNTIME_TUNING_SNAPSHOT.get_or_init(|| Mutex::new(None))
}

pub fn current_runtime_tuning_snapshot(bootstrap: RuntimeTuningState) -> RuntimeTuningSnapshot {
    let slot = runtime_tuning_snapshot_slot();
    let mut guard = slot.lock().unwrap_or_else(|poison| poison.into_inner());
    let snapshot = guard.get_or_insert(RuntimeTuningSnapshot {
        version: 1,
        state: bootstrap,
    });
    *snapshot
}

pub fn current_runtime_tuning_state(bootstrap: RuntimeTuningState) -> RuntimeTuningState {
    current_runtime_tuning_snapshot(bootstrap).state
}

pub fn update_runtime_tuning_state(
    bootstrap: RuntimeTuningState,
    vector_workers: Option<usize>,
    chunk_batch_size: Option<usize>,
    file_vectorization_batch_size: Option<usize>,
    vector_ready_queue_depth: Option<usize>,
    vector_persist_queue_bound: Option<usize>,
    vector_max_inflight_persists: Option<usize>,
    embed_micro_batch_max_items: Option<usize>,
    embed_micro_batch_max_total_tokens: Option<usize>,
) -> RuntimeTuningSnapshot {
    let slot = runtime_tuning_snapshot_slot();
    let mut guard = slot.lock().unwrap_or_else(|poison| poison.into_inner());
    let current = guard.get_or_insert(RuntimeTuningSnapshot {
        version: 1,
        state: bootstrap,
    });
    let mut next = current.state;
    if let Some(value) = vector_workers {
        next.vector_workers = value.max(1);
    }
    if let Some(value) = chunk_batch_size {
        next.chunk_batch_size = value.max(1);
    }
    if let Some(value) = file_vectorization_batch_size {
        next.file_vectorization_batch_size = value.max(1);
    }
    if let Some(value) = vector_ready_queue_depth {
        next.vector_ready_queue_depth = value.max(1);
    }
    if let Some(value) = vector_persist_queue_bound {
        next.vector_persist_queue_bound = value.max(1);
    }
    if let Some(value) = vector_max_inflight_persists {
        next.vector_max_inflight_persists = value.max(1);
    }
    if let Some(value) = embed_micro_batch_max_items {
        next.embed_micro_batch_max_items = value.max(1);
    }
    if let Some(value) = embed_micro_batch_max_total_tokens {
        next.embed_micro_batch_max_total_tokens = value.max(1);
    }
    if next != current.state {
        current.version = current.version.saturating_add(1);
        current.state = next;
    }
    *current
}

#[cfg(test)]
pub fn reset_runtime_tuning_snapshot(bootstrap: RuntimeTuningState) -> RuntimeTuningSnapshot {
    let snapshot = RuntimeTuningSnapshot {
        version: 1,
        state: bootstrap,
    };
    let slot = runtime_tuning_snapshot_slot();
    *slot.lock().unwrap_or_else(|poison| poison.into_inner()) = Some(snapshot);
    snapshot
}
