use anyhow::Result as AnyhowResult;

use super::{
    embed_prepared_batch_with_breakdown_ort, embed_texts_with_breakdown_ort,
    gpu_backend::{EmbeddingBatchStats, OrtGpuFirstTextEmbedding},
    PreparedVectorEmbedBatch,
};

pub(super) enum VectorEmbeddingBackend {
    CpuInProcess(OrtGpuFirstTextEmbedding),
    CudaInProcess(OrtGpuFirstTextEmbedding),
}

impl VectorEmbeddingBackend {
    pub(super) fn cpu_in_process(model: OrtGpuFirstTextEmbedding) -> Self {
        Self::CpuInProcess(model)
    }

    pub(super) fn cuda_in_process(model: OrtGpuFirstTextEmbedding) -> Self {
        Self::CudaInProcess(model)
    }

    pub(super) fn embed_prepared_batch_with_breakdown(
        &mut self,
        prepared: &PreparedVectorEmbedBatch,
    ) -> AnyhowResult<(Vec<Vec<f32>>, u64, u64, u64, u64, EmbeddingBatchStats)> {
        match self {
            Self::CpuInProcess(model) | Self::CudaInProcess(model) => {
                embed_prepared_batch_with_breakdown_ort(model, prepared)
            }
        }
    }

    pub(super) fn embed_texts_with_breakdown(
        &mut self,
        texts: &[String],
    ) -> AnyhowResult<(Vec<Vec<f32>>, u64, u64, u64, u64)> {
        match self {
            Self::CpuInProcess(model) | Self::CudaInProcess(model) => {
                // REQ-AXO-176 — drop tokenize_ms here; the production hot path
                // tracks throughput at queue level. Bench facade
                // `run_embedder_throughput_bench` reads the 6-tuple directly.
                let (
                    embeddings,
                    _tokenize_ms,
                    host_prepare_ms,
                    input_copy_ms,
                    inference_ms,
                    output_extract_ms,
                ) = embed_texts_with_breakdown_ort(model, texts)?;
                Ok((
                    embeddings,
                    host_prepare_ms,
                    input_copy_ms,
                    inference_ms,
                    output_extract_ms,
                ))
            }
        }
    }
}
