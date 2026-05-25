use anyhow::{anyhow, Result as AnyhowResult};
use fastembed::{OutputKey, Pooling};
use ort::ep;
use ort::execution_providers::ExecutionProviderDispatch;
use ort::io_binding::IoBinding;
use ort::memory::{AllocationDevice, AllocatorType, MemoryInfo, MemoryType};
use ort::session::{builder::GraphOptimizationLevel, run_options::RunOptions, Session};
use ort::value::TensorRef;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;
use tokenizers::{Encoding, Tokenizer};
use tracing::{info, warn};

use super::{
    current_gpu_memory_snapshot, embedding_model_cache_dir, gpu_memory_soft_limit_mb,
    gpu_recycle_immediate_required, load_runtime_embedding_tokenizer, normalize_embedding,
    ort_pooling_cls, ort_pooling_mean, runtime_embedding_snapshot_dir, FASTEMBED_OUTPUT_PRECEDENCE,
};

pub(crate) struct OrtGpuFirstTextEmbedding {
    pub(super) tokenizer: Arc<Tokenizer>,
    pub(super) session: Session,
    pub(super) io_binding: IoBinding,
    pub(super) run_options: RunOptions,
    pub(super) need_token_type_ids: bool,
    pub(super) pooling: Pooling,
    pub(super) output_name: String,
    pub(super) output_memory_info: MemoryInfo,
    pub(super) input_ids_buffer: Vec<i64>,
    pub(super) attention_mask_buffer: Vec<i64>,
    pub(super) token_type_ids_buffer: Vec<i64>,
    /// REQ-AXO-262 / VAL-AXO-054 — when false (default), the output
    /// is bound once at session init and reused across runs (avoids
    /// per-iter `clear_outputs` + `bind_output_to_device` which
    /// appears to trigger periodic allocator scrub). Toggle via
    /// `AXON_ORT_BIND_OUTPUT_PER_ITER=1` for A/B comparison.
    pub(super) bind_output_per_iter: bool,
    /// REQ-AXO-262 — sequence-length buckets the embedder pads to before
    /// inference. With `PaddingStrategy::BatchLongest` upstream, each batch
    /// arrives with an arbitrary `seq_len` (1..=512) which causes TRT to
    /// pick a different optimized kernel per shape and the cudaMallocAsync
    /// allocator to churn. Padding up to a small fixed set of buckets
    /// {128,256,384,512} collapses production into ≤4 stable shapes while
    /// keeping wasted compute bounded.
    /// Override via `AXON_EMBEDDER_SEQ_BUCKETS=128,256,384,512`. Empty list
    /// disables bucketing (legacy BatchLongest-only behavior).
    pub(super) seq_buckets: Vec<usize>,
}

/// REQ-AXO-262 — batch-level stats emitted alongside timings so the
/// pipeline trace can correlate slow iters with shape variance.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct EmbeddingBatchStats {
    /// Max raw sequence length across encodings in the micro-batch
    /// (= BatchLongest pad length BEFORE bucket-up).
    pub seq_len_raw_max: usize,
    /// Padded sequence length after bucket-up (= shape fed to the GPU).
    pub seq_len_padded_max: usize,
    /// Total NON-PAD tokens in the batch (sum of encoding lengths).
    pub tokens_total: usize,
    /// Total batch size = sum of micro-batch sizes.
    pub batch_size_total: usize,
}

impl EmbeddingBatchStats {
    pub(super) fn merge(&mut self, other: EmbeddingBatchStats) {
        self.seq_len_raw_max = self.seq_len_raw_max.max(other.seq_len_raw_max);
        self.seq_len_padded_max = self.seq_len_padded_max.max(other.seq_len_padded_max);
        self.tokens_total = self.tokens_total.saturating_add(other.tokens_total);
        self.batch_size_total = self.batch_size_total.saturating_add(other.batch_size_total);
    }
}










impl OrtGpuFirstTextEmbedding {
    pub(crate) fn try_new(lane: &str, worker_idx: usize, use_cuda: bool) -> AnyhowResult<Self> {
        let snapshot_dir = runtime_embedding_snapshot_dir()?;
        let model_path = snapshot_dir.join("onnx").join("model.onnx");
        let tokenizer = load_runtime_embedding_tokenizer()?;
        // REQ-AXO-262 trial — operator authorized 2026-05-10. Memory
        // pattern previously disabled (likely workaround for dynamic
        // batch sizes that pre-dated the IoBinding-with-fixed-shape
        // path). Sustained bench shows periodic slow iterations
        // every 3-5 fast iters (allocator scrub hypothesis). Enable
        // memory pattern to let ORT pre-allocate output buffers and
        // reduce allocator churn.
        let memory_pattern_enabled = ort_memory_pattern_enabled_from_env(
            std::env::var("AXON_ORT_MEMORY_PATTERN").ok().as_deref(),
        );
        let mut builder = Session::builder()
            .map_err(|err| anyhow!("failed to create ORT session builder: {err}"))?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|err| anyhow!("failed to set ORT optimization level: {err}"))?
            .with_memory_pattern(memory_pattern_enabled)
            .map_err(|err| anyhow!("failed to set ORT memory pattern={memory_pattern_enabled}: {err}"))?;

        if use_cuda {
            // Default: TensorRT EP first, fall back to CUDA EP if TensorRT init fails.
            let mut providers = Vec::new();
            match tensorrt_execution_provider_dispatch() {
                Ok(tensorrt) => providers.push(tensorrt),
                Err(err) => warn!("TensorRT EP unavailable, using CUDA EP: {err}"),
            }
            providers.push(cuda_execution_provider_dispatch());
            builder = builder.with_execution_providers(providers).map_err(|err| {
                anyhow!("failed to configure GPU execution providers for ORT session: {err}")
            })?;
        }

        let session = builder
            .commit_from_file(&model_path)
            .map_err(|err| anyhow!("failed to load ORT session {}: {err}", model_path.display()))?;
        let need_token_type_ids = session
            .inputs()
            .iter()
            .any(|input| input.name() == "token_type_ids");
        let output_name = session
            .outputs()
            .iter()
            .find_map(|output| {
                FASTEMBED_OUTPUT_PRECEDENCE
                    .iter()
                    .find_map(|candidate| match candidate {
                        OutputKey::OnlyOne => {
                            (session.outputs().len() == 1).then(|| output.name().to_string())
                        }
                        OutputKey::ByOrder(index) => session
                            .outputs()
                            .get(*index)
                            .map(|selected| selected.name().to_string()),
                        OutputKey::ByName(name) => {
                            (output.name() == *name).then(|| name.to_string())
                        }
                    })
            })
            .ok_or_else(|| anyhow!("failed to determine ORT embedding output name"))?;
        let mut io_binding = session
            .create_binding()
            .map_err(|err| anyhow!("failed to create ORT I/O binding: {err}"))?;
        let run_options =
            RunOptions::new().map_err(|err| anyhow!("failed to create ORT run options: {err}"))?;
        let output_memory_info = if use_cuda {
            MemoryInfo::new(
                AllocationDevice::CPU,
                0,
                AllocatorType::Device,
                MemoryType::CPUOutput,
            )
            .map_err(|err| anyhow!("failed to create CPU output memory info: {err}"))?
        } else {
            MemoryInfo::new(
                AllocationDevice::CPU,
                0,
                AllocatorType::Arena,
                MemoryType::Default,
            )
            .map_err(|err| anyhow!("failed to create CPU output memory info: {err}"))?
        };
        // REQ-AXO-262 / VAL-AXO-054 follow-up — keep the per-iter
        // re-bind by default (proven baseline 118-134 ch/s @ batch=64).
        // The bind-once experiment regressed throughput (78 ch/s) by
        // aggregating allocator stress into bigger slow-iter bursts.
        // Path retained behind AXON_ORT_BIND_OUTPUT_PER_ITER=0 for
        // future A/B work. See VAL-AXO-055.
        let bind_output_per_iter = ort_bind_output_per_iter_from_env(
            std::env::var("AXON_ORT_BIND_OUTPUT_PER_ITER").ok().as_deref(),
        );
        if !bind_output_per_iter {
            io_binding
                .bind_output_to_device(output_name.clone(), &output_memory_info)
                .map_err(|err| anyhow!("failed to pre-bind output {}: {err}", output_name))?;
        }

        let seq_buckets = parse_seq_buckets_from_env(
            std::env::var("AXON_EMBEDDER_SEQ_BUCKETS").ok().as_deref(),
        );

        info!(
            "✅ Semantic {} Worker [{}]: ORT GPU-first embedding runner loaded (provider={}, bind_output_per_iter={}, seq_buckets={:?})",
            lane,
            worker_idx,
            if use_cuda { "cuda" } else { "cpu" },
            bind_output_per_iter,
            seq_buckets
        );

        Ok(Self {
            tokenizer,
            session,
            io_binding,
            run_options,
            need_token_type_ids,
            pooling: Pooling::Cls,
            output_name,
            output_memory_info,
            input_ids_buffer: Vec::new(),
            attention_mask_buffer: Vec::new(),
            token_type_ids_buffer: Vec::new(),
            bind_output_per_iter,
            seq_buckets,
        })
    }

    fn encode_and_bind_inputs(
        &mut self,
        encodings: &[Encoding],
    ) -> AnyhowResult<(Vec<i64>, usize, usize, u64, u64, u64, EmbeddingBatchStats)> {
        let input_prepare_started = Instant::now();
        let batch_size = encodings.len();
        // BatchLongest upstream guarantees all encodings have equal length,
        // but we conservatively scan to compute the true max and total tokens.
        let mut raw_seq = 0usize;
        let mut tokens_total = 0usize;
        for enc in encodings {
            let len = enc.len();
            if len > raw_seq {
                raw_seq = len;
            }
            tokens_total = tokens_total.saturating_add(len);
        }
        if raw_seq == 0 {
            return Err(anyhow!("expected at least one non-empty encoding"));
        }
        // REQ-AXO-262 — pad the batch up to a fixed bucket so the GPU sees a
        // small set of stable shapes. Eliminates TRT kernel reselection and
        // cudaMallocAsync churn observed under BatchLongest variance.
        let sequence_len = bucket_up(raw_seq, &self.seq_buckets);
        let element_count = batch_size.saturating_mul(sequence_len);
        self.input_ids_buffer.resize(element_count, 0);
        self.attention_mask_buffer.resize(element_count, 0);
        if self.need_token_type_ids {
            self.token_type_ids_buffer.resize(element_count, 0);
        } else {
            self.token_type_ids_buffer.clear();
        }

        let fill_started = Instant::now();
        for (row, encoding) in encodings.iter().enumerate() {
            let ids = encoding.get_ids();
            let mask = encoding.get_attention_mask();
            let type_ids = encoding.get_type_ids();
            let row_offset = row * sequence_len;
            let real_len = ids.len().min(sequence_len);
            for col in 0..real_len {
                self.input_ids_buffer[row_offset + col] = ids[col] as i64;
                self.attention_mask_buffer[row_offset + col] = mask[col] as i64;
                if self.need_token_type_ids {
                    self.token_type_ids_buffer[row_offset + col] = type_ids[col] as i64;
                }
            }
            // Pad tail (PAD_ID=0, mask=0, type=0). Resize already zeroed
            // freshly-grown positions but stale data may remain when batch
            // size shrunk and grew within the same buffer, so explicitly
            // zero the tail every iter.
            for col in real_len..sequence_len {
                self.input_ids_buffer[row_offset + col] = 0;
                self.attention_mask_buffer[row_offset + col] = 0;
                if self.need_token_type_ids {
                    self.token_type_ids_buffer[row_offset + col] = 0;
                }
            }
        }
        let host_prepare_ms = input_prepare_started.elapsed().as_millis() as u64;
        let host_fill_ms = fill_started.elapsed().as_millis() as u64;
        let stats = EmbeddingBatchStats {
            seq_len_raw_max: raw_seq,
            seq_len_padded_max: sequence_len,
            tokens_total,
            batch_size_total: batch_size,
        };
        let shape = [batch_size, sequence_len];
        let input_ids = TensorRef::from_array_view((shape, self.input_ids_buffer.as_slice()))
            .map_err(|err| anyhow!("failed to create input_ids tensor view: {err}"))?;
        let attention_mask =
            TensorRef::from_array_view((shape, self.attention_mask_buffer.as_slice()))
                .map_err(|err| anyhow!("failed to create attention_mask tensor view: {err}"))?;
        let token_type_ids = if self.need_token_type_ids {
            Some(
                TensorRef::from_array_view((shape, self.token_type_ids_buffer.as_slice()))
                    .map_err(|err| anyhow!("failed to create token_type_ids tensor view: {err}"))?,
            )
        } else {
            None
        };

        self.io_binding.clear_inputs();
        self.io_binding
            .bind_input("input_ids", &input_ids)
            .map_err(|err| anyhow!("failed to bind input_ids: {err}"))?;
        self.io_binding
            .bind_input("attention_mask", &attention_mask)
            .map_err(|err| anyhow!("failed to bind attention_mask: {err}"))?;
        if let Some(token_type_ids) = token_type_ids.as_ref() {
            self.io_binding
                .bind_input("token_type_ids", token_type_ids)
                .map_err(|err| anyhow!("failed to bind token_type_ids: {err}"))?;
        }

        Ok((
            self.attention_mask_buffer.clone(),
            batch_size,
            sequence_len,
            host_prepare_ms,
            host_fill_ms,
            0,
            stats,
        ))
    }

    fn pool_output(
        output_name: &str,
        pooling: Pooling,
        outputs: &ort::session::SessionOutputs<'_>,
        attention_mask: &[i64],
        batch_size: usize,
        sequence_len: usize,
    ) -> AnyhowResult<Vec<Vec<f32>>> {
        let (shape, tensor) = outputs
            .get(output_name)
            .ok_or_else(|| anyhow!("missing output {}", output_name))?
            .try_extract_tensor::<f32>()
            .map_err(|err| anyhow!("failed to extract output tensor {}: {err}", output_name))?;

        let pooled = match pooling {
            Pooling::Cls => ort_pooling_cls(shape.as_ref(), tensor, batch_size)?,
            Pooling::Mean => ort_pooling_mean(
                shape.as_ref(),
                tensor,
                attention_mask,
                batch_size,
                sequence_len,
            )?,
        };
        Ok(pooled.into_iter().map(normalize_embedding).collect())
    }

    pub(super) fn transform_encoded_with_breakdown(
        &mut self,
        encodings: &[Encoding],
    ) -> AnyhowResult<(Vec<Vec<f32>>, u64, u64, u64, u64, EmbeddingBatchStats)> {
        if encodings.is_empty() {
            return Ok((Vec::new(), 0, 0, 0, 0, EmbeddingBatchStats::default()));
        }

        let (
            attention_mask,
            batch_size,
            sequence_len,
            host_prepare_ms,
            host_fill_ms,
            input_copy_ms,
            stats,
        ) = self.encode_and_bind_inputs(encodings)?;
        // REQ-AXO-262 — skip per-iter clear+rebind by default;
        // output was bound once at session init.
        if self.bind_output_per_iter {
            self.io_binding.clear_outputs();
            self.io_binding
                .bind_output_to_device(self.output_name.clone(), &self.output_memory_info)
                .map_err(|err| anyhow!("failed to bind output {}: {err}", self.output_name))?;
        }

        let run_started = Instant::now();
        let outputs = self
            .session
            .run_binding_with_options(&self.io_binding, &self.run_options)
            .map_err(|err| anyhow!("failed ORT run_binding for embedding batch: {err}"))?;
        self.io_binding
            .synchronize()
            .map_err(|err| anyhow!("failed to synchronize ORT I/O binding: {err}"))?;
        let inference_ms = run_started.elapsed().as_millis() as u64;

        let extract_started = Instant::now();
        let output_name = self.output_name.clone();
        let pooling = self.pooling.clone();
        let embeddings = Self::pool_output(
            &output_name,
            pooling,
            &outputs,
            &attention_mask,
            batch_size,
            sequence_len,
        )?;
        let output_extract_ms = extract_started.elapsed().as_millis() as u64;

        Ok((
            embeddings,
            host_prepare_ms.saturating_add(host_fill_ms),
            input_copy_ms,
            inference_ms,
            output_extract_ms,
            stats,
        ))
    }
}






/// REQ-AXO-262 — pure helper to parse
/// `AXON_ORT_BIND_OUTPUT_PER_ITER` env override.
///
/// **Default = true** (re-bind output per iteration, the legacy
/// behaviour). Empirical measurement 2026-05-10 (test-bind-once-b64)
/// showed that binding-once **regressed** throughput from 118-134 ch/s
/// to 78 ch/s @ batch=64: slow-iter frequency dropped (1/8-15 vs
/// 1/3-5) but each slow iter became 2-3x more expensive (~5-7s vs ~3s).
/// Net: aggregated allocator stress hurts more than it helps. Path
/// kept behind the env knob for further A/B experimentation but the
/// default reverts to the proven baseline.
///
/// Accepts `0`, `false`, `False`, `FALSE` (case-insensitive) as the
/// explicit-disable marker (i.e. opt-in to bind-once). All other
/// values keep the default true (re-bind per iter).
pub(super) fn ort_bind_output_per_iter_from_env(raw: Option<&str>) -> bool {
    match raw {
        Some(v) => {
            let trimmed = v.trim();
            !(trimmed == "0" || trimmed.eq_ignore_ascii_case("false"))
        }
        None => true,
    }
}

/// REQ-AXO-262 — pure helper to parse `AXON_ORT_MEMORY_PATTERN` env
/// override. Default = true (memory pattern enabled). Accepts `0`,
/// `false`, `False`, `FALSE` (any case) as the disabled marker.
/// Sibling-tested in `gpu_backend_tests.rs` per GUI-PRO-001.
pub(super) fn ort_memory_pattern_enabled_from_env(raw: Option<&str>) -> bool {
    match raw {
        Some(v) => {
            let trimmed = v.trim();
            !(trimmed == "0" || trimmed.eq_ignore_ascii_case("false"))
        }
        None => true,
    }
}

/// REQ-AXO-262 — default sequence-length buckets for the GPU embedder.
/// Choices reflect BGE-Large `max_length=512`:
/// `128, 256, 384, 512` keep wasted compute bounded (≤4× for small chunks)
/// while collapsing production into 4 stable GPU shapes.
pub(crate) const DEFAULT_SEQ_BUCKETS: &[usize] = &[128, 256, 384, 512];

/// REQ-AXO-262 — parse a comma-separated bucket list. Empty input
/// (None, empty string, "0", or "off") disables bucketing (legacy
/// BatchLongest-only behavior). Buckets are deduplicated and sorted
/// ascending; non-numeric entries are skipped.
pub(super) fn parse_seq_buckets_from_env(raw: Option<&str>) -> Vec<usize> {
    let raw = match raw {
        Some(v) => v.trim(),
        None => return DEFAULT_SEQ_BUCKETS.to_vec(),
    };
    if raw.is_empty()
        || raw == "0"
        || raw.eq_ignore_ascii_case("off")
        || raw.eq_ignore_ascii_case("none")
    {
        return Vec::new();
    }
    let mut out: Vec<usize> = raw
        .split(',')
        .filter_map(|tok| tok.trim().parse::<usize>().ok())
        .filter(|v| *v > 0)
        .collect();
    out.sort_unstable();
    out.dedup();
    if out.is_empty() {
        DEFAULT_SEQ_BUCKETS.to_vec()
    } else {
        out
    }
}

/// REQ-AXO-262 — round `raw` up to the next bucket. When the input
/// exceeds the largest bucket, the largest bucket is returned (callers
/// should ensure their model max_length and TRT max profile cover that
/// value). When the bucket list is empty, returns `raw` unchanged
/// (bucketing disabled, falls back to BatchLongest seq_len).
pub(crate) fn bucket_up(raw: usize, buckets: &[usize]) -> usize {
    if buckets.is_empty() {
        return raw;
    }
    for &b in buckets {
        if raw <= b {
            return b;
        }
    }
    *buckets.last().unwrap()
}

#[cfg(test)]
#[path = "gpu_backend_tests.rs"]
mod gpu_backend_tests;

pub(super) fn abort_gpu_embed_if_vram_summit_reached() -> AnyhowResult<()> {
    if gpu_recycle_immediate_required(current_gpu_memory_snapshot(), 0) {
        let vram_used_mb = current_gpu_memory_snapshot()
            .map(|snapshot| snapshot.used_mb)
            .unwrap_or(0);
        return Err(anyhow!(
            "gpu_recycle_immediate_after_vram_summit vram={}",
            vram_used_mb
        ));
    }
    Ok(())
}

pub(crate) fn cuda_execution_provider_dispatch() -> ExecutionProviderDispatch {
    let mut cuda = ep::CUDA::default()
        .with_device_id(0)
        .with_memory_limit(cuda_memory_limit_bytes())
        .with_arena_extend_strategy(ort::ep::ArenaExtendStrategy::SameAsRequested)
        .with_conv_max_workspace(false)
        .with_conv_algorithm_search(ort::ep::cuda::ConvAlgorithmSearch::Heuristic);
    if cuda_tf32_enabled() {
        cuda = cuda.with_tf32(true);
    }
    ExecutionProviderDispatch::from(cuda.build()).error_on_failure()
}

fn tensorrt_cache_dir() -> PathBuf {
    embedding_model_cache_dir().join("tensorrt")
}

pub(super) fn tensorrt_execution_provider_dispatch() -> AnyhowResult<ExecutionProviderDispatch> {
    let provider_path = ort_tensorrt_provider_library_path()
        .ok_or_else(|| anyhow!("ORT_DYLIB_PATH missing for TensorRT provider discovery"))?;
    if !provider_path.is_file() {
        return Err(anyhow!(
            "TensorRT provider library missing: {}",
            provider_path.display()
        ));
    }
    let cache_dir = tensorrt_cache_dir();
    let engine_cache_dir = cache_dir.join("engine-cache");
    let timing_cache_dir = cache_dir.join("timing-cache");
    fs::create_dir_all(&engine_cache_dir).map_err(|err| {
        anyhow!(
            "failed to create TensorRT engine cache dir {}: {err}",
            engine_cache_dir.display()
        )
    })?;
    fs::create_dir_all(&timing_cache_dir).map_err(|err| {
        anyhow!(
            "failed to create TensorRT timing cache dir {}: {err}",
            timing_cache_dir.display()
        )
    })?;

    let workspace_size = cuda_memory_limit_bytes();
    // REQ-AXO-262 (operator 2026-05-10) + REQ-AXO-91570 (operator
    // 2026-05-17) — explicit dynamic-shape profile so a single TRT
    // engine covers the full bench / production range. Without these,
    // every batch-size or seq-len change triggers an engine rebuild.
    //
    // Format per ORT TRT EP docs: `"name:DxD,name:DxD,..."`.
    // BGE-Large inputs: input_ids[batch, seq], attention_mask[batch, seq],
    // token_type_ids[batch, seq] (when present in the model graph).
    //
    // Range chosen 2026-05-17 (REQ-AXO-91570 — operator directive
    // 2026-05-17 to cap VRAM at production batch headroom) :
    //   min  = (1, 1)        // smallest legal shape
    //   opt  = (64, 256)     // matches AXON_CHUNK_BATCH_SIZE=64 production point
    //   max  = (64, 512)     // capped at production batch (was 256 → -1-2GB VRAM)
    //
    // Production batch is 24 (operator 2026-05-17), so max=64 gives 2.6×
    // headroom while shaving ~1-2GB VRAM versus the previous 256-wide max.
    // seq=512 stays at BGE-Large's max_length.
    //
    // Override via AXON_TRT_PROFILE_{MIN,OPT,MAX}_SHAPES if a different
    // range is required (e.g. for a smaller VRAM budget).
    let trt_profile_min = std::env::var("AXON_TRT_PROFILE_MIN_SHAPES").unwrap_or_else(|_| {
        "input_ids:1x1,attention_mask:1x1,token_type_ids:1x1".to_string()
    });
    let trt_profile_opt = std::env::var("AXON_TRT_PROFILE_OPT_SHAPES").unwrap_or_else(|_| {
        "input_ids:64x256,attention_mask:64x256,token_type_ids:64x256".to_string()
    });
    let trt_profile_max = std::env::var("AXON_TRT_PROFILE_MAX_SHAPES").unwrap_or_else(|_| {
        "input_ids:64x512,attention_mask:64x512,token_type_ids:64x512".to_string()
    });
    let provider = ep::TensorRT::default()
        .with_device_id(0)
        .with_max_workspace_size(workspace_size)
        .with_fp16(true)
        .with_engine_cache(true)
        .with_engine_cache_path(engine_cache_dir.display().to_string())
        .with_engine_cache_prefix("axon-bge-large")
        .with_timing_cache(true)
        .with_timing_cache_path(timing_cache_dir.display().to_string())
        .with_force_timing_cache(true)
        .with_builder_optimization_level(5)
        .with_build_heuristics(true)
        .with_auxiliary_streams(1)
        .with_profile_min_shapes(trt_profile_min)
        .with_profile_opt_shapes(trt_profile_opt)
        .with_profile_max_shapes(trt_profile_max)
        .build();

    Ok(ExecutionProviderDispatch::from(provider).error_on_failure())
}


pub(crate) fn cuda_memory_limit_bytes() -> usize {
    (std::env::var("AXON_CUDA_MEMORY_LIMIT_MB")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value >= 512)
        .map(|value| value as u64)
        .unwrap_or_else(gpu_memory_soft_limit_mb)
        .max(512) as usize)
        .saturating_mul(1024 * 1024)
}

pub(super) fn cuda_tf32_enabled() -> bool {
    // REQ-AXO-262 — default ON for Ampere+ GPUs. TF32 matmul gives a
    // 1.5–2× speedup vs FP32 on tensor cores with negligible accuracy loss
    // for embedding inference. Set AXON_CUDA_ALLOW_TF32=0 (or false) to
    // disable for accuracy-sensitive experiments.
    cuda_tf32_enabled_from_env(std::env::var("AXON_CUDA_ALLOW_TF32").ok().as_deref())
}

/// REQ-AXO-262 — pure helper for the TF32 env parser. Default = true.
/// Accepts `0`, `false`, `no`, `off` (case-insensitive) as explicit disable.
pub(super) fn cuda_tf32_enabled_from_env(raw: Option<&str>) -> bool {
    match raw {
        Some(v) => {
            let trimmed = v.trim();
            !(trimmed == "0"
                || trimmed.eq_ignore_ascii_case("false")
                || trimmed.eq_ignore_ascii_case("no")
                || trimmed.eq_ignore_ascii_case("off"))
        }
        None => true,
    }
}

pub(crate) fn ort_cuda_provider_library_path() -> Option<PathBuf> {
    let ort_dylib_path = std::env::var("ORT_DYLIB_PATH")
        .ok()
        .filter(|value| !value.trim().is_empty())?;
    let ort_dir = Path::new(&ort_dylib_path).parent()?;
    Some(ort_dir.join("libonnxruntime_providers_cuda.so"))
}

pub(crate) fn ort_cuda_provider_library_available() -> bool {
    ort_cuda_provider_library_path()
        .map(|path| path.is_file())
        .unwrap_or(false)
}

pub(super) fn ort_tensorrt_provider_library_path() -> Option<PathBuf> {
    let ort_dylib_path = std::env::var("ORT_DYLIB_PATH")
        .ok()
        .filter(|value| !value.trim().is_empty())?;
    let ort_dir = Path::new(&ort_dylib_path).parent()?;
    Some(ort_dir.join("libonnxruntime_providers_tensorrt.so"))
}
