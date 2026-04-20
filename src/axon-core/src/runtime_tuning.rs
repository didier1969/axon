use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeTuningState {
    pub vector_workers: usize,
    pub graph_workers: usize,
    pub chunk_batch_size: usize,
    pub file_vectorization_batch_size: usize,
    pub vector_ready_queue_depth: usize,
    pub vector_persist_queue_bound: usize,
    pub vector_max_inflight_persists: usize,
    pub embed_micro_batch_max_items: usize,
    pub embed_micro_batch_max_total_tokens: usize,
    pub semantic_sleep_scale_pct: usize,
    pub semantic_idle_sleep_scale_pct: usize,
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

pub fn normalize_runtime_tuning_state(mut state: RuntimeTuningState) -> RuntimeTuningState {
    state.vector_workers = state.vector_workers.max(1);
    state.graph_workers = state.graph_workers.clamp(0, 64);
    state.chunk_batch_size = state.chunk_batch_size.clamp(16, 256);
    state.file_vectorization_batch_size = state.file_vectorization_batch_size.clamp(4, 64);
    state.vector_ready_queue_depth = state.vector_ready_queue_depth.clamp(1, 32);
    state.vector_persist_queue_bound = state.vector_persist_queue_bound.clamp(1, 12);
    state.vector_max_inflight_persists = state
        .vector_max_inflight_persists
        .clamp(1, state.vector_persist_queue_bound);
    state.embed_micro_batch_max_items = state.embed_micro_batch_max_items.clamp(8, 256);
    state.embed_micro_batch_max_total_tokens =
        state.embed_micro_batch_max_total_tokens.clamp(512, 65_536);
    state.semantic_sleep_scale_pct = state.semantic_sleep_scale_pct.clamp(25, 400);
    state.semantic_idle_sleep_scale_pct = state.semantic_idle_sleep_scale_pct.clamp(25, 400);
    state
}

pub fn current_runtime_tuning_snapshot(bootstrap: RuntimeTuningState) -> RuntimeTuningSnapshot {
    let slot = runtime_tuning_snapshot_slot();
    let mut guard = slot.lock().unwrap_or_else(|poison| poison.into_inner());
    let snapshot = guard.get_or_insert(RuntimeTuningSnapshot {
        version: 1,
        state: normalize_runtime_tuning_state(bootstrap),
    });
    *snapshot
}

pub fn current_runtime_tuning_state(bootstrap: RuntimeTuningState) -> RuntimeTuningState {
    current_runtime_tuning_snapshot(bootstrap).state
}

pub fn update_runtime_tuning_state(
    bootstrap: RuntimeTuningState,
    vector_workers: Option<usize>,
    graph_workers: Option<usize>,
    chunk_batch_size: Option<usize>,
    file_vectorization_batch_size: Option<usize>,
    vector_ready_queue_depth: Option<usize>,
    vector_persist_queue_bound: Option<usize>,
    vector_max_inflight_persists: Option<usize>,
    embed_micro_batch_max_items: Option<usize>,
    embed_micro_batch_max_total_tokens: Option<usize>,
    semantic_sleep_scale_pct: Option<usize>,
    semantic_idle_sleep_scale_pct: Option<usize>,
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
    if let Some(value) = graph_workers {
        next.graph_workers = value;
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
    if let Some(value) = semantic_sleep_scale_pct {
        next.semantic_sleep_scale_pct = value.max(1);
    }
    if let Some(value) = semantic_idle_sleep_scale_pct {
        next.semantic_idle_sleep_scale_pct = value.max(1);
    }
    next = normalize_runtime_tuning_state(next);
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
        state: normalize_runtime_tuning_state(bootstrap),
    };
    let slot = runtime_tuning_snapshot_slot();
    *slot.lock().unwrap_or_else(|poison| poison.into_inner()) = Some(snapshot);
    snapshot
}
