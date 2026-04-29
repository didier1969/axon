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

pub(super) struct OrtGpuFirstTextEmbedding {
    pub(super) tokenizer: Tokenizer,
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
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct GpuEmbedSubprocessInit {
    pub(super) ok: bool,
    pub(super) error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct GpuEmbedSubprocessRequest {
    pub(super) texts: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct GpuEmbedSubprocessResponse {
    pub(super) ok: bool,
    pub(super) embeddings: Option<Vec<Vec<f32>>>,
    pub(super) host_prepare_ms: u64,
    pub(super) input_copy_ms: u64,
    pub(super) inference_ms: u64,
    pub(super) output_extract_ms: u64,
    pub(super) error: Option<String>,
}

struct GpuEmbedSubprocess {
    child: Child,
    stdin: BufWriter<ChildStdin>,
    stdout: BufReader<ChildStdout>,
}

pub(super) struct GpuEmbeddingServiceClient {
    subprocess: GpuEmbedSubprocess,
}

static GPU_EMBED_SERVICE_CLIENT: OnceLock<Arc<Mutex<GpuEmbeddingServiceClient>>> = OnceLock::new();

pub(super) fn gpu_embed_service_enabled() -> bool {
    std::env::var("AXON_GPU_EMBED_SERVICE_ENABLED")
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .or_else(|| {
            std::env::var("AXON_GPU_EMBED_SUBPROCESS")
                .ok()
                .map(|value| {
                    matches!(
                        value.trim().to_ascii_lowercase().as_str(),
                        "1" | "true" | "yes" | "on"
                    )
                })
        })
        .unwrap_or(false)
}

fn gpu_embed_service_recycle_every_batch_enabled() -> bool {
    std::env::var("AXON_GPU_EMBED_SERVICE_RECYCLE_EVERY_BATCH")
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .or_else(|| {
            std::env::var("AXON_GPU_EMBED_SUBPROCESS_RECYCLE_EVERY_BATCH")
                .ok()
                .map(|value| {
                    matches!(
                        value.trim().to_ascii_lowercase().as_str(),
                        "1" | "true" | "yes" | "on"
                    )
                })
        })
        .unwrap_or(false)
}

pub(super) fn gpu_embed_service_prefers_tensorrt() -> bool {
    std::env::var("AXON_GPU_EMBED_SERVICE_TENSORRT")
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

impl OrtGpuFirstTextEmbedding {
    pub(super) fn try_new(lane: &str, worker_idx: usize, use_cuda: bool) -> AnyhowResult<Self> {
        let snapshot_dir = runtime_embedding_snapshot_dir()?;
        let model_path = snapshot_dir.join("onnx").join("model.onnx");
        let tokenizer = load_runtime_embedding_tokenizer()?;
        let mut builder = Session::builder()
            .map_err(|err| anyhow!("failed to create ORT session builder: {err}"))?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|err| anyhow!("failed to set ORT optimization level: {err}"))?
            .with_memory_pattern(false)
            .map_err(|err| anyhow!("failed to disable ORT memory pattern: {err}"))?;

        if use_cuda {
            let providers = gpu_service_execution_providers()?;
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
        let io_binding = session
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
        info!(
            "✅ Semantic {} Worker [{}]: ORT GPU-first embedding runner loaded successfully (provider={})",
            lane,
            worker_idx,
            if use_cuda { "cuda" } else { "cpu" }
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
        })
    }

    fn encode_and_bind_inputs(
        &mut self,
        encodings: &[Encoding],
    ) -> AnyhowResult<(Vec<i64>, usize, usize, u64, u64, u64)> {
        let input_prepare_started = Instant::now();
        let batch_size = encodings.len();
        let sequence_len = encodings
            .first()
            .map(|encoding| encoding.len())
            .ok_or_else(|| anyhow!("expected at least one encoding"))?;
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
            for col in 0..sequence_len {
                self.input_ids_buffer[row_offset + col] = ids[col] as i64;
                let mask_value = mask[col] as i64;
                self.attention_mask_buffer[row_offset + col] = mask_value;
                if self.need_token_type_ids {
                    self.token_type_ids_buffer[row_offset + col] = type_ids[col] as i64;
                }
            }
        }
        let host_prepare_ms = input_prepare_started.elapsed().as_millis() as u64;
        let host_fill_ms = fill_started.elapsed().as_millis() as u64;
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
    ) -> AnyhowResult<(Vec<Vec<f32>>, u64, u64, u64, u64)> {
        if encodings.is_empty() {
            return Ok((Vec::new(), 0, 0, 0, 0));
        }

        let (
            attention_mask,
            batch_size,
            sequence_len,
            host_prepare_ms,
            host_fill_ms,
            input_copy_ms,
        ) = self.encode_and_bind_inputs(encodings)?;
        self.io_binding.clear_outputs();
        self.io_binding
            .bind_output_to_device(self.output_name.clone(), &self.output_memory_info)
            .map_err(|err| anyhow!("failed to bind output {}: {err}", self.output_name))?;

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
        ))
    }
}

impl GpuEmbedSubprocess {
    fn spawn_child() -> AnyhowResult<(Child, BufWriter<ChildStdin>, BufReader<ChildStdout>)> {
        let exe = std::env::current_exe().map_err(|err| {
            anyhow!("failed to resolve current executable for GPU subprocess: {err}")
        })?;
        let mut child = Command::new(&exe)
            .env("AXON_GPU_EMBED_SERVICE_CHILD", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|err| {
                anyhow!(
                    "failed to spawn GPU embed subprocess {}: {err}",
                    exe.display()
                )
            })?;
        let child_stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("GPU embed subprocess missing stdin"))?;
        let child_stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("GPU embed subprocess missing stdout"))?;
        Ok((
            child,
            BufWriter::new(child_stdin),
            BufReader::new(child_stdout),
        ))
    }

    fn spawn() -> AnyhowResult<Self> {
        let (child, stdin, mut stdout) = Self::spawn_child()?;
        let mut init_line = String::new();
        stdout
            .read_line(&mut init_line)
            .map_err(|err| anyhow!("failed to read GPU embed subprocess init: {err}"))?;
        if init_line.trim().is_empty() {
            return Err(anyhow!("GPU embed subprocess exited before init handshake"));
        }
        let init: GpuEmbedSubprocessInit = serde_json::from_str(init_line.trim())
            .map_err(|err| anyhow!("failed to parse GPU embed subprocess init: {err}"))?;
        if !init.ok {
            return Err(anyhow!(
                "GPU embed subprocess failed to initialize: {}",
                init.error.unwrap_or_else(|| "unknown error".to_string())
            ));
        }
        Ok(Self {
            child,
            stdin,
            stdout,
        })
    }

    fn recycle(&mut self) -> AnyhowResult<()> {
        let _ = self.stdin.flush();
        let _ = self.child.kill();
        let _ = self.child.wait();
        let (child, stdin, mut stdout) = Self::spawn_child()?;
        let mut init_line = String::new();
        stdout
            .read_line(&mut init_line)
            .map_err(|err| anyhow!("failed to read GPU embed subprocess recycle init: {err}"))?;
        if init_line.trim().is_empty() {
            return Err(anyhow!(
                "GPU embed subprocess exited before recycle init handshake"
            ));
        }
        let init: GpuEmbedSubprocessInit = serde_json::from_str(init_line.trim())
            .map_err(|err| anyhow!("failed to parse recycled GPU embed subprocess init: {err}"))?;
        if !init.ok {
            return Err(anyhow!(
                "recycled GPU embed subprocess failed to initialize: {}",
                init.error.unwrap_or_else(|| "unknown error".to_string())
            ));
        }
        self.child = child;
        self.stdin = stdin;
        self.stdout = stdout;
        Ok(())
    }

    pub(super) fn embed_texts(
        &mut self,
        texts: &[String],
    ) -> AnyhowResult<(Vec<Vec<f32>>, u64, u64, u64, u64)> {
        let request = GpuEmbedSubprocessRequest {
            texts: texts.to_vec(),
        };
        let payload = serde_json::to_string(&request)
            .map_err(|err| anyhow!("failed to serialize GPU embed request: {err}"))?;
        self.stdin
            .write_all(payload.as_bytes())
            .and_then(|_| self.stdin.write_all(b"\n"))
            .and_then(|_| self.stdin.flush())
            .map_err(|err| anyhow!("failed to send GPU embed request to subprocess: {err}"))?;

        let mut response_line = String::new();
        self.stdout
            .read_line(&mut response_line)
            .map_err(|err| anyhow!("failed to read GPU embed subprocess response: {err}"))?;
        if response_line.trim().is_empty() {
            return Err(anyhow!(
                "GPU embed subprocess exited before sending response"
            ));
        }
        let response: GpuEmbedSubprocessResponse = serde_json::from_str(response_line.trim())
            .map_err(|err| anyhow!("failed to parse GPU embed subprocess response: {err}"))?;
        if !response.ok {
            return Err(anyhow!(
                "{}",
                response
                    .error
                    .unwrap_or_else(|| "GPU embed subprocess returned unknown error".to_string())
            ));
        }
        let result = (
            response.embeddings.unwrap_or_default(),
            response.host_prepare_ms,
            response.input_copy_ms,
            response.inference_ms,
            response.output_extract_ms,
        );
        if gpu_embed_service_recycle_every_batch_enabled()
            || gpu_recycle_immediate_required(current_gpu_memory_snapshot(), 0)
        {
            info!("GPU embedding service: recycling child process after batch");
            self.recycle()?;
        }
        Ok(result)
    }
}

impl GpuEmbeddingServiceClient {
    fn connect() -> AnyhowResult<Self> {
        Ok(Self {
            subprocess: GpuEmbedSubprocess::spawn()?,
        })
    }

    fn recycle(&mut self) -> AnyhowResult<()> {
        self.subprocess.recycle()
    }

    pub(super) fn embed_texts(
        &mut self,
        texts: &[String],
    ) -> AnyhowResult<(Vec<Vec<f32>>, u64, u64, u64, u64)> {
        match self.subprocess.embed_texts(texts) {
            Ok(result) => Ok(result),
            Err(first_err) => {
                warn!(
                    "GPU embedding service request failed, recycling service process: {:?}",
                    first_err
                );
                self.recycle().map_err(|recycle_err| {
                    anyhow!(
                        "GPU embedding service request failed: {first_err:?}; recycle failed: {recycle_err:?}"
                    )
                })?;
                self.subprocess.embed_texts(texts).map_err(|retry_err| {
                    anyhow!(
                        "GPU embedding service retry failed after recycle: first={first_err:?}, retry={retry_err:?}"
                    )
                })
            }
        }
    }
}

pub(super) fn gpu_embedding_service_client() -> AnyhowResult<Arc<Mutex<GpuEmbeddingServiceClient>>>
{
    if let Some(existing) = GPU_EMBED_SERVICE_CLIENT.get() {
        return Ok(Arc::clone(existing));
    }

    let client = Arc::new(Mutex::new(GpuEmbeddingServiceClient::connect()?));
    match GPU_EMBED_SERVICE_CLIENT.set(Arc::clone(&client)) {
        Ok(()) => Ok(client),
        Err(_) => GPU_EMBED_SERVICE_CLIENT
            .get()
            .cloned()
            .ok_or_else(|| anyhow!("GPU embedding service singleton missing after initialization")),
    }
}

impl Drop for GpuEmbedSubprocess {
    fn drop(&mut self) {
        let _ = self.stdin.flush();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

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
        .build();

    Ok(ExecutionProviderDispatch::from(provider).error_on_failure())
}

pub(super) fn gpu_service_execution_providers() -> AnyhowResult<Vec<ExecutionProviderDispatch>> {
    let mut providers = Vec::new();
    if gpu_embed_service_prefers_tensorrt() {
        providers.push(tensorrt_execution_provider_dispatch()?);
    }
    providers.push(cuda_execution_provider_dispatch());
    Ok(providers)
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
    std::env::var("AXON_CUDA_ALLOW_TF32")
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes"
            )
        })
        .unwrap_or(false)
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
