use crate::embedder::{
    PreparedVectorEmbedBatch, PreparedVectorEmbedSequenceReply, VectorPersistOutcomeReply,
    VectorPersistPlan,
};
use crate::graph_ingestion::{
    FileVectorizationLeaseSnapshot, FileVectorizationWork, VectorBatchRun,
};
use anyhow::Result as AnyhowResult;
use std::ops::{Deref, DerefMut};
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

#[derive(Debug)]
pub(crate) struct PreparedBatchEnvelope {
    prepared: PreparedVectorEmbedBatch,
}

impl PreparedBatchEnvelope {
    pub(crate) fn new(prepared: PreparedVectorEmbedBatch) -> Self {
        Self { prepared }
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

pub(crate) struct InflightPersistRequest {
    pub(crate) reply_rx: VectorPersistOutcomeReply,
    pub(crate) claimed: ClaimedLeaseSet,
}

pub(crate) struct InflightPrepareRequest {
    pub(crate) reply_rx: PreparedVectorEmbedSequenceReply,
    pub(crate) claimed: ClaimedLeaseSet,
    pub(crate) dispatched_at: Instant,
}
