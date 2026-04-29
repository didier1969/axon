use super::*;

#[derive(Debug)]
pub(super) struct VectorRefillProducerState {
    active_works: Vec<FileVectorizationWork>,
    inflight_prepares: VecDeque<InflightPrepareRequest>,
}

#[derive(Debug)]
pub(super) enum VectorRefillCommand {
    RequeueWorks(Vec<FileVectorizationWork>),
    BatchConsumed(usize),
}

impl VectorRefillProducerState {
    pub(super) fn new(active_works: Vec<FileVectorizationWork>) -> Self {
        Self {
            active_works,
            inflight_prepares: VecDeque::new(),
        }
    }

    pub(super) fn active_works(&self) -> &[FileVectorizationWork] {
        &self.active_works
    }

    pub(super) fn active_len(&self) -> usize {
        self.active_works.len()
    }

    pub(super) fn take_active_works(&mut self) -> Vec<FileVectorizationWork> {
        std::mem::take(&mut self.active_works)
    }

    pub(super) fn replace_active_works(&mut self, works: Vec<FileVectorizationWork>) {
        self.active_works = works;
    }

    pub(super) fn inflight_prepares(&self) -> &VecDeque<InflightPrepareRequest> {
        &self.inflight_prepares
    }

    pub(super) fn inflight_prepare_count(&self) -> usize {
        self.inflight_prepares.len()
    }

    pub(super) fn inflight_prepare_chunk_count(&self) -> usize {
        self.inflight_prepares
            .iter()
            .map(|request| request.planned_chunk_count)
            .sum()
    }

    pub(super) fn has_pending(&self, ready_depth: usize, inflight_persists: usize) -> bool {
        !self.active_works.is_empty()
            || !self.inflight_prepares.is_empty()
            || ready_depth > 0
            || inflight_persists > 0
    }

    pub(super) fn top_up_from_claimable_queue(
        &mut self,
        graph_store: &GraphStore,
        claim_target: usize,
        extra_owned_works: &[FileVectorizationWork],
        reserved_slots: usize,
    ) -> AnyhowResult<usize> {
        let locally_buffered_works = merge_unique_vectorization_work_sets([
            self.active_works.clone(),
            self.inflight_prepares
                .iter()
                .flat_map(|request| request.claimed.clone_works())
                .collect::<Vec<_>>(),
            extra_owned_works.to_vec(),
        ])
        .len()
            + reserved_slots;
        if locally_buffered_works >= claim_target {
            return Ok(0);
        }
        let top_up = graph_store.fetch_pending_file_vectorization_work(
            claim_target.saturating_sub(locally_buffered_works),
        )?;
        let added = top_up.len();
        let active = std::mem::take(&mut self.active_works);
        self.active_works = merge_vectorization_work(
            active,
            top_up,
            claim_target.saturating_sub(reserved_slots).max(1),
        );
        Ok(added)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn dispatch_prepare_request(
        &mut self,
        graph_store: &GraphStore,
        prepare_tx: &Sender<VectorPrepareRequest>,
        request_claim_target: usize,
        target_chunks: usize,
        per_file_fetch_limit: usize,
        batch_max_bytes: usize,
        target_ready_depth: usize,
        target_ready_chunks: usize,
    ) -> AnyhowResult<bool> {
        let claimed_works =
            split_claimed_vectorization_work(&mut self.active_works, request_claim_target);
        if claimed_works.is_empty() {
            return Ok(false);
        }
        let claimed = ClaimedLeaseSet::new(claimed_works);
        let _ = graph_store.refresh_inflight_file_vectorization_claims(claimed.as_slice());
        match dispatch_prepared_vector_embed_sequence(
            prepare_tx,
            claimed.clone(),
            target_chunks,
            per_file_fetch_limit,
            batch_max_bytes,
            target_ready_depth,
        ) {
            Ok(reply_rx) => {
                self.inflight_prepares.push_back(InflightPrepareRequest {
                    reply_rx,
                    claimed,
                    planned_chunk_count: target_ready_chunks,
                    dispatched_at: Instant::now(),
                });
                service_guard::record_vector_prepare_inflight_depth(
                    self.inflight_prepares.len() as u64
                );
                service_guard::record_vector_prepare_inflight_chunks(
                    self.inflight_prepare_chunk_count() as u64,
                );
                Ok(true)
            }
            Err(err) => {
                let merge_target = self
                    .active_works
                    .len()
                    .saturating_add(claimed.as_slice().len())
                    .max(1);
                let active = std::mem::take(&mut self.active_works);
                self.active_works =
                    merge_vectorization_work(claimed.into_inner(), active, merge_target);
                Err(err)
            }
        }
    }

    pub(super) fn poll_prepare_replies(&mut self, worker_idx: usize) {
        let pending = self.inflight_prepares.len();
        for _ in 0..pending {
            let Some(inflight) = self.inflight_prepares.pop_front() else {
                break;
            };
            match inflight.reply_rx.try_recv() {
                Ok(outcome) => {
                    service_guard::record_vector_prepare_reply_wait_ms(
                        inflight.dispatched_at.elapsed().as_millis() as u64,
                    );
                    apply_prepared_vector_prepare_outcome(outcome, &mut self.active_works);
                }
                Err(TryRecvError::Empty) => self.inflight_prepares.push_back(inflight),
                Err(TryRecvError::Disconnected) => {
                    error!(
                        "Semantic Vector Worker [{}]: prepare reply disconnected before completion",
                        worker_idx
                    );
                    self.merge_claimed_back(inflight.claimed);
                }
            }
        }
        service_guard::record_vector_prepare_inflight_depth(self.inflight_prepares.len() as u64);
        service_guard::record_vector_prepare_inflight_chunks(
            self.inflight_prepare_chunk_count() as u64
        );
    }

    fn merge_claimed_back(&mut self, claimed: ClaimedLeaseSet) {
        let merge_target = self
            .active_works
            .len()
            .saturating_add(claimed.as_slice().len())
            .max(1);
        let active = std::mem::take(&mut self.active_works);
        self.active_works = merge_vectorization_work(claimed.into_inner(), active, merge_target);
    }

    pub(super) fn merge_requeued_works(&mut self, works: Vec<FileVectorizationWork>) {
        if works.is_empty() {
            return;
        }
        let merge_target = self.active_len().saturating_add(works.len()).max(1);
        let current_active = self.take_active_works();
        self.replace_active_works(merge_vectorization_work(
            current_active,
            works,
            merge_target,
        ));
    }
}
