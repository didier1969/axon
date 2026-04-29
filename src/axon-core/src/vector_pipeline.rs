use crate::embedder::{
    PreparedVectorEmbedBatch, PreparedVectorPrepareOutcomeReply, VectorBatchLane,
    VectorPersistOutcomeReply, VectorPersistPlan,
};
use crate::graph_ingestion::{
    FileVectorizationLeaseSnapshot, FileVectorizationWork, VectorBatchRun,
};
use crate::service_guard;
use anyhow::Result as AnyhowResult;
use std::collections::{HashMap, HashSet, VecDeque};
use std::ops::{Deref, DerefMut};
use std::sync::Mutex;
use std::time::Instant;

#[derive(Debug, Clone)]
pub(crate) struct ClaimedLeaseSet {
    works: Vec<FileVectorizationWork>,
}

impl ClaimedLeaseSet {
    pub(crate) fn new(works: Vec<FileVectorizationWork>) -> Self {
        Self { works }
    }

    pub(crate) fn as_slice(&self) -> &[FileVectorizationWork] {
        &self.works
    }

    pub(crate) fn clone_works(&self) -> Vec<FileVectorizationWork> {
        self.works.clone()
    }

    pub(crate) fn into_inner(self) -> Vec<FileVectorizationWork> {
        self.works
    }
}

#[derive(Debug)]
pub(crate) struct FinalizeEnvelope {
    pub(crate) completed_works: Vec<FileVectorizationWork>,
    pub(crate) lease_snapshots: Vec<FileVectorizationLeaseSnapshot>,
    pub(crate) batch_runs: Vec<VectorBatchRun>,
}

#[derive(Debug)]
pub(crate) struct PersistEnvelope {
    pub(crate) persist_plan: VectorPersistPlan,
    pub(crate) batch_run: VectorBatchRun,
}

impl PersistEnvelope {
    pub(crate) fn sync_batch_run_counts_from_plan(&mut self) {
        let (chunk_count, file_count) = self.persist_plan.sync_batch_run_counts();
        self.batch_run.chunk_count = chunk_count;
        self.batch_run.file_count = file_count;
    }
}

#[derive(Debug, Clone)]
pub(crate) struct PreparedBatchEnvelope {
    prepared: PreparedVectorEmbedBatch,
}

impl PreparedBatchEnvelope {
    pub(crate) fn new(prepared: PreparedVectorEmbedBatch) -> Self {
        Self { prepared }
    }

    pub(crate) fn into_inner(self) -> PreparedVectorEmbedBatch {
        self.prepared
    }

    pub(crate) fn into_touched_works(self) -> Vec<FileVectorizationWork> {
        self.prepared.into_touched_works()
    }

    pub(crate) fn into_persist_envelope(
        self,
        embeddings: Vec<Vec<f32>>,
        batch_run: VectorBatchRun,
    ) -> AnyhowResult<PersistEnvelope> {
        Ok(PersistEnvelope {
            persist_plan: self
                .prepared
                .into_persist_plan(embeddings, batch_run.clone())?,
            batch_run,
        })
    }
}

impl Deref for PreparedBatchEnvelope {
    type Target = PreparedVectorEmbedBatch;

    fn deref(&self) -> &Self::Target {
        &self.prepared
    }
}

impl DerefMut for PreparedBatchEnvelope {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.prepared
    }
}

#[derive(Debug, Default)]
pub(crate) struct SharedPreparedBatchQueue {
    inner: Mutex<PreparedBatchQueueState>,
}

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct PreparedBatchQueueSummary {
    pub(crate) len: usize,
    pub(crate) chunk_count: usize,
    pub(crate) small_batch_count: usize,
    pub(crate) medium_batch_count: usize,
    pub(crate) large_batch_count: usize,
    pub(crate) mixed_batch_count: usize,
    pub(crate) small_chunk_count: usize,
    pub(crate) medium_chunk_count: usize,
    pub(crate) large_chunk_count: usize,
    pub(crate) touched_works_count: usize,
    pub(crate) oldest_prepared_at_ms: Option<i64>,
}

#[derive(Debug, Default)]
struct PreparedBatchQueueState {
    small: VecDeque<PreparedBatchEnvelope>,
    medium: VecDeque<PreparedBatchEnvelope>,
    large: VecDeque<PreparedBatchEnvelope>,
    mixed: VecDeque<PreparedBatchEnvelope>,
    chunk_count: usize,
    small_chunk_count: usize,
    medium_chunk_count: usize,
    large_chunk_count: usize,
    mixed_chunk_count: usize,
    touched_works_count: usize,
    touched_work_refcounts: HashMap<String, usize>,
    touched_work_index: HashMap<String, FileVectorizationWork>,
    oldest_prepared_at_ms: Option<i64>,
}

impl PreparedBatchQueueState {
    fn queue_mut_for_lane(
        &mut self,
        lane: VectorBatchLane,
    ) -> &mut VecDeque<PreparedBatchEnvelope> {
        match lane {
            VectorBatchLane::Small => &mut self.small,
            VectorBatchLane::Medium => &mut self.medium,
            VectorBatchLane::Large => &mut self.large,
            VectorBatchLane::Mixed => &mut self.mixed,
        }
    }

    fn queue_for_lane(&self, lane: VectorBatchLane) -> &VecDeque<PreparedBatchEnvelope> {
        match lane {
            VectorBatchLane::Small => &self.small,
            VectorBatchLane::Medium => &self.medium,
            VectorBatchLane::Large => &self.large,
            VectorBatchLane::Mixed => &self.mixed,
        }
    }

    fn all_queues(&self) -> [&VecDeque<PreparedBatchEnvelope>; 4] {
        [&self.small, &self.medium, &self.large, &self.mixed]
    }

    fn lane_chunk_count(&self, lane: VectorBatchLane) -> usize {
        match lane {
            VectorBatchLane::Small => self.small_chunk_count,
            VectorBatchLane::Medium => self.medium_chunk_count,
            VectorBatchLane::Large => self.large_chunk_count,
            VectorBatchLane::Mixed => self.mixed_chunk_count,
        }
    }

    fn adjust_lane_chunk_count(&mut self, lane: VectorBatchLane, delta: isize) {
        let value = match lane {
            VectorBatchLane::Small => &mut self.small_chunk_count,
            VectorBatchLane::Medium => &mut self.medium_chunk_count,
            VectorBatchLane::Large => &mut self.large_chunk_count,
            VectorBatchLane::Mixed => &mut self.mixed_chunk_count,
        };
        if delta >= 0 {
            *value = value.saturating_add(delta as usize);
        } else {
            *value = value.saturating_sub(delta.unsigned_abs());
        }
    }

    fn record_touched_works(&mut self, batch: &PreparedBatchEnvelope) {
        let mut seen = HashSet::new();
        for work in batch.touched_works_slice() {
            if !seen.insert(work.file_path.clone()) {
                continue;
            }
            let entry = self
                .touched_work_refcounts
                .entry(work.file_path.clone())
                .or_insert(0);
            *entry += 1;
            self.touched_work_index
                .entry(work.file_path.clone())
                .and_modify(|existing| {
                    existing.resumed_after_interactive_pause |=
                        work.resumed_after_interactive_pause;
                })
                .or_insert_with(|| work.clone());
        }
        self.touched_works_count = self.touched_work_refcounts.len();
    }

    fn release_touched_works(&mut self, batch: &PreparedBatchEnvelope) {
        let mut seen = HashSet::new();
        for work in batch.touched_works_slice() {
            if !seen.insert(work.file_path.clone()) {
                continue;
            }
            let should_remove = self
                .touched_work_refcounts
                .get_mut(&work.file_path)
                .map(|count| {
                    *count = count.saturating_sub(1);
                    *count == 0
                })
                .unwrap_or(false);
            if should_remove {
                self.touched_work_refcounts.remove(&work.file_path);
                self.touched_work_index.remove(&work.file_path);
            }
        }
        self.touched_works_count = self.touched_work_refcounts.len();
    }

    fn recompute_oldest_prepared_at_ms(&mut self) {
        self.oldest_prepared_at_ms = self
            .all_queues()
            .into_iter()
            .filter_map(|queue| queue.front().map(|batch| batch.prepared_at_ms()))
            .min();
    }

    fn total_len(&self) -> usize {
        self.small.len() + self.medium.len() + self.large.len() + self.mixed.len()
    }

    fn push_batch(
        &mut self,
        batch: PreparedBatchEnvelope,
        push: impl FnOnce(&mut VecDeque<PreparedBatchEnvelope>, PreparedBatchEnvelope),
    ) {
        let lane = batch.batch_lane();
        let chunk_count = batch.chunk_count() as usize;
        let prepared_at_ms = batch.prepared_at_ms();
        self.record_touched_works(&batch);
        self.chunk_count = self.chunk_count.saturating_add(chunk_count);
        self.adjust_lane_chunk_count(lane, chunk_count as isize);
        self.oldest_prepared_at_ms = Some(
            self.oldest_prepared_at_ms
                .map(|oldest| oldest.min(prepared_at_ms))
                .unwrap_or(prepared_at_ms),
        );
        push(self.queue_mut_for_lane(lane), batch);
    }

    fn push_front(&mut self, batch: PreparedBatchEnvelope) {
        self.push_batch(batch, |queue, batch| queue.push_front(batch));
    }

    fn push_back(&mut self, batch: PreparedBatchEnvelope) {
        self.push_batch(batch, |queue, batch| queue.push_back(batch));
    }

    fn pop_from_lane(&mut self, lane: VectorBatchLane) -> Option<PreparedBatchEnvelope> {
        let batch = self.queue_mut_for_lane(lane).pop_front()?;
        let chunk_count = batch.chunk_count() as usize;
        self.chunk_count = self.chunk_count.saturating_sub(chunk_count);
        self.adjust_lane_chunk_count(lane, -(chunk_count as isize));
        self.release_touched_works(&batch);
        self.recompute_oldest_prepared_at_ms();
        Some(batch)
    }

    fn pop_best(&mut self) -> Option<PreparedBatchEnvelope> {
        let best_lane = [
            VectorBatchLane::Large,
            VectorBatchLane::Medium,
            VectorBatchLane::Small,
            VectorBatchLane::Mixed,
        ]
        .into_iter()
        .filter_map(|lane| {
            self.queue_for_lane(lane)
                .front()
                .map(|batch| (lane, batch.density_score(), batch.prepared_at_ms()))
        })
        .max_by_key(|(lane, score, prepared_at_ms)| (*score, lane.priority(), -prepared_at_ms))?
        .0;
        self.pop_from_lane(best_lane)
    }

    fn summary(&self) -> PreparedBatchQueueSummary {
        PreparedBatchQueueSummary {
            len: self.total_len(),
            chunk_count: self.chunk_count,
            small_batch_count: self.small.len(),
            medium_batch_count: self.medium.len(),
            large_batch_count: self.large.len(),
            mixed_batch_count: self.mixed.len(),
            small_chunk_count: self.lane_chunk_count(VectorBatchLane::Small),
            medium_chunk_count: self.lane_chunk_count(VectorBatchLane::Medium),
            large_chunk_count: self.lane_chunk_count(VectorBatchLane::Large),
            touched_works_count: self.touched_works_count,
            oldest_prepared_at_ms: self.oldest_prepared_at_ms,
        }
    }
}

impl SharedPreparedBatchQueue {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn len(&self) -> usize {
        self.inner.lock().unwrap().total_len()
    }

    pub(crate) fn pop_best(&self) -> Option<PreparedBatchEnvelope> {
        self.inner.lock().unwrap().pop_best()
    }

    pub(crate) fn push_front(&self, batch: PreparedBatchEnvelope) -> usize {
        let mut guard = self.inner.lock().unwrap();
        guard.push_front(batch);
        let len = guard.total_len();
        drop(guard);
        service_guard::notify_vector_backlog_activity();
        len
    }

    pub(crate) fn push_back_many(&self, batches: Vec<PreparedBatchEnvelope>) -> usize {
        if batches.is_empty() {
            return self.len();
        }
        let mut guard = self.inner.lock().unwrap();
        for batch in batches {
            guard.push_back(batch);
        }
        let len = guard.total_len();
        drop(guard);
        service_guard::notify_vector_backlog_activity();
        len
    }

    pub(crate) fn drain(&self) -> Vec<PreparedBatchEnvelope> {
        let mut guard = self.inner.lock().unwrap();
        let mut drained = Vec::new();
        drained.extend(guard.large.drain(..));
        drained.extend(guard.medium.drain(..));
        drained.extend(guard.small.drain(..));
        drained.extend(guard.mixed.drain(..));
        *guard = PreparedBatchQueueState::default();
        drained
    }

    pub(crate) fn summary(&self) -> PreparedBatchQueueSummary {
        self.inner.lock().unwrap().summary()
    }

    pub(crate) fn touched_works_snapshot(&self) -> Vec<FileVectorizationWork> {
        let mut snapshot = self
            .inner
            .lock()
            .unwrap()
            .touched_work_index
            .values()
            .cloned()
            .collect::<Vec<_>>();
        snapshot.sort_by(|left, right| left.file_path.cmp(&right.file_path));
        snapshot
    }

    #[cfg(test)]
    pub(crate) fn snapshot(&self) -> Vec<PreparedBatchEnvelope> {
        let guard = self.inner.lock().unwrap();
        guard
            .all_queues()
            .into_iter()
            .flat_map(|queue| queue.iter().cloned())
            .collect()
    }

    pub(crate) fn retain<F>(&self, mut keep: F) -> Vec<PreparedBatchEnvelope>
    where
        F: FnMut(&PreparedBatchEnvelope) -> bool,
    {
        let mut guard = self.inner.lock().unwrap();
        let mut removed = Vec::new();
        for lane in [
            VectorBatchLane::Small,
            VectorBatchLane::Medium,
            VectorBatchLane::Large,
            VectorBatchLane::Mixed,
        ] {
            let mut retained = VecDeque::with_capacity(guard.queue_for_lane(lane).len());
            while let Some(batch) = guard.queue_mut_for_lane(lane).pop_front() {
                if keep(&batch) {
                    retained.push_back(batch);
                } else {
                    let chunk_count = batch.chunk_count() as usize;
                    guard.chunk_count = guard.chunk_count.saturating_sub(chunk_count);
                    guard.adjust_lane_chunk_count(lane, -(chunk_count as isize));
                    guard.release_touched_works(&batch);
                    removed.push(batch);
                }
            }
            *guard.queue_mut_for_lane(lane) = retained;
        }
        guard.recompute_oldest_prepared_at_ms();
        removed
    }
}

#[derive(Debug)]
pub(crate) struct InflightPersistRequest {
    pub(crate) reply_rx: VectorPersistOutcomeReply,
    pub(crate) claimed: ClaimedLeaseSet,
}

#[derive(Debug)]
pub(crate) struct InflightPrepareRequest {
    pub(crate) reply_rx: PreparedVectorPrepareOutcomeReply,
    pub(crate) claimed: ClaimedLeaseSet,
    pub(crate) planned_chunk_count: usize,
    pub(crate) dispatched_at: Instant,
}
