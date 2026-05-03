use std::sync::{Arc, Mutex};

use anyhow::Result as AnyhowResult;

use super::{
    embed_prepared_batch_with_breakdown_ort, embed_texts_with_breakdown_ort,
    gpu_backend::{GpuEmbeddingServiceClient, OrtGpuFirstTextEmbedding},
    PreparedVectorEmbedBatch, ProviderStrategy,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum VectorExecutorStrategy {
    CpuInProcess,
    CudaInProcess,
    CudaService,
    TensorRtService,
}

impl VectorExecutorStrategy {
    pub(super) fn provider_strategy(self) -> ProviderStrategy {
        match self {
            Self::CpuInProcess => ProviderStrategy::Cpu,
            Self::CudaInProcess | Self::CudaService => ProviderStrategy::Cuda,
            Self::TensorRtService => ProviderStrategy::TensorRt,
        }
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::CpuInProcess => "cpu_in_process",
            Self::CudaInProcess => "cuda_in_process",
            Self::CudaService => "cuda_service",
            Self::TensorRtService => "tensorrt_service",
        }
    }
}

pub(super) enum VectorEmbeddingBackend {
    CpuInProcess(OrtGpuFirstTextEmbedding),
    CudaInProcess(OrtGpuFirstTextEmbedding),
    CudaService(Arc<Mutex<GpuEmbeddingServiceClient>>),
    TensorRtService(Arc<Mutex<GpuEmbeddingServiceClient>>),
}

impl VectorEmbeddingBackend {
    pub(super) fn cpu_in_process(model: OrtGpuFirstTextEmbedding) -> Self {
        Self::CpuInProcess(model)
    }

    pub(super) fn cuda_in_process(model: OrtGpuFirstTextEmbedding) -> Self {
        Self::CudaInProcess(model)
    }

    pub(super) fn gpu_service(
        client: Arc<Mutex<GpuEmbeddingServiceClient>>,
        tensorrt_requested: bool,
    ) -> Self {
        if tensorrt_requested {
            Self::TensorRtService(client)
        } else {
            Self::CudaService(client)
        }
    }

    fn executor_strategy(&self) -> VectorExecutorStrategy {
        match self {
            Self::CpuInProcess(_) => VectorExecutorStrategy::CpuInProcess,
            Self::CudaInProcess(_) => VectorExecutorStrategy::CudaInProcess,
            Self::CudaService(_) => VectorExecutorStrategy::CudaService,
            Self::TensorRtService(_) => VectorExecutorStrategy::TensorRtService,
        }
    }

    pub(super) fn embed_prepared_batch_with_breakdown(
        &mut self,
        prepared: &PreparedVectorEmbedBatch,
    ) -> AnyhowResult<(Vec<Vec<f32>>, u64, u64, u64, u64)> {
        let _provider_strategy = self.executor_strategy().provider_strategy();
        let _executor_label = self.executor_strategy().label();
        match self {
            Self::CpuInProcess(model) | Self::CudaInProcess(model) => {
                embed_prepared_batch_with_breakdown_ort(model, prepared)
            }
            Self::CudaService(client) | Self::TensorRtService(client) => client
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .embed_texts(&prepared.texts),
        }
    }

    pub(super) fn embed_texts_with_breakdown(
        &mut self,
        texts: &[String],
    ) -> AnyhowResult<(Vec<Vec<f32>>, u64, u64, u64, u64)> {
        let _provider_strategy = self.executor_strategy().provider_strategy();
        let _executor_label = self.executor_strategy().label();
        match self {
            Self::CpuInProcess(model) | Self::CudaInProcess(model) => {
                // REQ-AXO-176 — drop tokenize_ms here; the production
                // hot path tracks throughput at queue level. Bench
                // facade `run_embedder_throughput_bench` reads the
                // 6-tuple directly.
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
            Self::CudaService(client) | Self::TensorRtService(client) => client
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .embed_texts(texts),
        }
    }
}
