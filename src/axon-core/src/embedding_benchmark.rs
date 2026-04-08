use crate::embedder::{
    apply_embedding_batch_overrides, calibrated_embedding_profile_without_overrides,
    configured_embedding_batch_overrides, embedding_execution_providers,
    embedding_profile_for_key, EmbeddingExecutionBackend, EmbeddingProfileKey,
};
use crate::parser::get_parser_for_file;
use crate::runtime_profile::RuntimeProfile;
use anyhow::{Context, Result};
use fastembed::{InitOptions, TextEmbedding};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::Instant;
use walkdir::WalkDir;

pub const BENCHMARK_TARGET_EMBEDDINGS_PER_HOUR: u64 = 300_000;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BenchmarkMeasurementLayer {
    ModelOnly,
    PrepareEmbed,
    FullPipeline,
}

impl BenchmarkMeasurementLayer {
    pub fn label(self) -> &'static str {
        match self {
            Self::ModelOnly => "model_only",
            Self::PrepareEmbed => "prepare_embed",
            Self::FullPipeline => "full_pipeline",
        }
    }

    pub fn includes_corpus_collection_in_total_seconds(self) -> bool {
        matches!(self, Self::FullPipeline)
    }

    pub fn includes_prepare_seconds_in_total_seconds(self) -> bool {
        matches!(self, Self::PrepareEmbed | Self::FullPipeline)
    }

    pub fn prebuilds_batches(self) -> bool {
        matches!(self, Self::ModelOnly)
    }
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BenchmarkTargetKind {
    File,
    Type,
    Procedure,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct BenchmarkSample {
    pub target: BenchmarkTargetKind,
    pub label: String,
    pub source_path: PathBuf,
    pub text: String,
}

impl BenchmarkSample {
    pub fn new(
        target: BenchmarkTargetKind,
        label: String,
        source_path: PathBuf,
        text: String,
    ) -> Self {
        Self {
            target,
            label,
            source_path,
            text,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CorpusCollectionLimits {
    pub max_files: usize,
    pub max_file_chars: usize,
    pub max_symbol_chars: usize,
    pub max_samples_per_target: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct RepoBenchmarkCorpus {
    pub root: PathBuf,
    pub scanned_files: usize,
    pub supported_files: usize,
    pub files: Vec<BenchmarkSample>,
    pub types: Vec<BenchmarkSample>,
    pub procedures: Vec<BenchmarkSample>,
}

#[derive(Debug, Clone)]
pub struct RealEmbeddingBenchmarkConfig {
    pub repo_path: PathBuf,
    pub profile_key: EmbeddingProfileKey,
    pub backend: EmbeddingExecutionBackend,
    pub measurement_layer: BenchmarkMeasurementLayer,
    pub warmup_batches: usize,
    pub min_samples_per_target: usize,
    pub limits: CorpusCollectionLimits,
}

#[derive(Debug, Clone, Serialize)]
pub struct BenchmarkTargetReport {
    pub target: BenchmarkTargetKind,
    pub unique_samples: usize,
    pub measured_samples: usize,
    pub batch_size: usize,
    pub measurement_layer: BenchmarkMeasurementLayer,
    pub corpus_collection_seconds_included: bool,
    pub corpus_collection_seconds: f64,
    pub prepare_seconds_included: bool,
    pub prepare_seconds: f64,
    pub total_embeddings: usize,
    pub total_seconds: f64,
    pub embeddings_per_second: f64,
    pub embeddings_per_hour: f64,
    pub avg_batch_latency_ms: f64,
    pub p95_batch_latency_ms: f64,
    pub target_met: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct RealEmbeddingBenchmarkReport {
    pub mode: &'static str,
    pub measurement_layer: BenchmarkMeasurementLayer,
    pub repo_path: PathBuf,
    pub model_name: String,
    pub dimension: usize,
    pub requested_backend: &'static str,
    pub gpu_present: bool,
    pub target_embeddings_per_hour: u64,
    pub target_embeddings_per_second: f64,
    pub warmup_batches: usize,
    pub min_samples_per_target: usize,
    pub batch_override_active: bool,
    pub batch_override_source: &'static str,
    pub canonical_profile_batches: BenchmarkProfileBatchReport,
    pub effective_profile_batches: BenchmarkProfileBatchReport,
    pub provider_effective: Option<String>,
    pub provider_note: String,
    pub corpus: RepoBenchmarkCorpus,
    pub targets: Vec<BenchmarkTargetReport>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BenchmarkProfileBatchReport {
    pub chunk_batch_size: usize,
    pub symbol_batch_size: usize,
    pub file_vectorization_batch_size: usize,
    pub graph_batch_size: usize,
}

impl Default for RealEmbeddingBenchmarkConfig {
    fn default() -> Self {
        Self {
            repo_path: PathBuf::from("."),
            profile_key: EmbeddingProfileKey::JinaCodeV2Base,
            backend: EmbeddingExecutionBackend::Cpu,
            measurement_layer: BenchmarkMeasurementLayer::FullPipeline,
            warmup_batches: 4,
            min_samples_per_target: 1_024,
            limits: CorpusCollectionLimits {
                max_files: 2_000,
                max_file_chars: 6_000,
                max_symbol_chars: 2_000,
                max_samples_per_target: 4_096,
            },
        }
    }
}

pub fn benchmark_target_for_symbol_kind(kind: &str) -> Option<BenchmarkTargetKind> {
    match kind.trim().to_ascii_lowercase().as_str() {
        "class" | "interface" | "trait" | "struct" | "enum" | "module" | "mod"
        | "type" | "type_alias" | "protocol" => Some(BenchmarkTargetKind::Type),
        "function" | "method" | "procedure" | "def" => Some(BenchmarkTargetKind::Procedure),
        _ => None,
    }
}

pub fn expand_benchmark_samples(
    samples: &[BenchmarkSample],
    min_count: usize,
) -> Vec<BenchmarkSample> {
    if samples.is_empty() || min_count == 0 {
        return Vec::new();
    }

    let mut expanded = Vec::with_capacity(min_count.max(samples.len()));
    while expanded.len() < min_count {
        for sample in samples {
            expanded.push(sample.clone());
            if expanded.len() == min_count {
                break;
            }
        }
    }
    expanded
}

pub fn collect_repo_benchmark_corpus(
    root: &Path,
    limits: &CorpusCollectionLimits,
) -> Result<RepoBenchmarkCorpus> {
    let root = root
        .canonicalize()
        .with_context(|| format!("failed to canonicalize repo root {}", root.display()))?;

    let mut scanned_files = 0usize;
    let mut supported_files = 0usize;
    let mut files = Vec::new();
    let mut types = Vec::new();
    let mut procedures = Vec::new();

    for entry in WalkDir::new(&root).into_iter() {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        if entry.path() != root && is_hidden_or_generated(relative_to_root(&root, entry.path())) {
            continue;
        }
        if !entry.file_type().is_file() {
            continue;
        }
        scanned_files += 1;
        if files.len() >= limits.max_files
            && types.len() >= limits.max_samples_per_target
            && procedures.len() >= limits.max_samples_per_target
        {
            break;
        }

        let path = entry.path();
        let Some(parser) = get_parser_for_file(path) else {
            continue;
        };
        supported_files += 1;

        let content = match std::fs::read_to_string(path) {
            Ok(content) if !content.trim().is_empty() => content,
            _ => continue,
        };

        if files.len() < limits.max_files {
            files.push(BenchmarkSample::new(
                BenchmarkTargetKind::File,
                display_relative(&root, path),
                path.to_path_buf(),
                truncate_chars(content.trim(), limits.max_file_chars),
            ));
        }

        let extraction = parser.parse(&content);
        for symbol in extraction.symbols {
            let Some(target) = benchmark_target_for_symbol_kind(&symbol.kind) else {
                continue;
            };
            let bucket = match target {
                BenchmarkTargetKind::File => continue,
                BenchmarkTargetKind::Type => &mut types,
                BenchmarkTargetKind::Procedure => &mut procedures,
            };
            if bucket.len() >= limits.max_samples_per_target {
                continue;
            }

            let snippet = symbol_snippet(&content, symbol.start_line, symbol.end_line);
            let trimmed = truncate_chars(snippet.trim(), limits.max_symbol_chars);
            if trimmed.is_empty() {
                continue;
            }
            bucket.push(BenchmarkSample::new(
                target,
                symbol.name,
                path.to_path_buf(),
                trimmed,
            ));
        }
    }

    Ok(RepoBenchmarkCorpus {
        root,
        scanned_files,
        supported_files,
        files,
        types,
        procedures,
    })
}

pub fn run_real_embedding_benchmark(
    config: &RealEmbeddingBenchmarkConfig,
) -> Result<RealEmbeddingBenchmarkReport> {
    let runtime_profile = RuntimeProfile::detect();
    let base_profile = embedding_profile_for_key(config.profile_key);
    let canonical_profile =
        calibrated_embedding_profile_without_overrides(&base_profile, config.backend);
    let overrides = configured_embedding_batch_overrides();
    let profile = apply_embedding_batch_overrides(&canonical_profile, overrides);
    let mut options = InitOptions::new(profile.runtime_model.fastembed_model());
    options.show_download_progress = true;
    options = options.with_execution_providers(embedding_execution_providers(config.backend));
    let mut model =
        TextEmbedding::try_new(options).context("failed to initialize embedding model")?;

    let corpus_started = Instant::now();
    let corpus = collect_repo_benchmark_corpus(&config.repo_path, &config.limits)?;
    let corpus_collection_seconds = corpus_started.elapsed().as_secs_f64();
    let targets = vec![
        benchmark_target_report(
            &mut model,
            BenchmarkTargetKind::File,
            &corpus.files,
            profile.file_vectorization_batch_size,
            config.measurement_layer,
            corpus_collection_seconds,
            config.warmup_batches,
            config.min_samples_per_target,
        )?,
        benchmark_target_report(
            &mut model,
            BenchmarkTargetKind::Type,
            &corpus.types,
            profile.symbol.batch_size,
            config.measurement_layer,
            corpus_collection_seconds,
            config.warmup_batches,
            config.min_samples_per_target,
        )?,
        benchmark_target_report(
            &mut model,
            BenchmarkTargetKind::Procedure,
            &corpus.procedures,
            profile.symbol.batch_size,
            config.measurement_layer,
            corpus_collection_seconds,
            config.warmup_batches,
            config.min_samples_per_target,
        )?,
    ];

    Ok(RealEmbeddingBenchmarkReport {
        mode: "real",
        measurement_layer: config.measurement_layer,
        repo_path: config.repo_path.clone(),
        model_name: profile.model_name.to_string(),
        dimension: profile.dimension,
        requested_backend: config.backend.name(),
        gpu_present: runtime_profile.gpu_present,
        target_embeddings_per_hour: BENCHMARK_TARGET_EMBEDDINGS_PER_HOUR,
        target_embeddings_per_second: BENCHMARK_TARGET_EMBEDDINGS_PER_HOUR as f64 / 3600.0,
        warmup_batches: config.warmup_batches,
        min_samples_per_target: config.min_samples_per_target,
        batch_override_active: overrides.is_active(),
        batch_override_source: if overrides.is_active() {
            "runtime_env"
        } else {
            "canonical"
        },
        canonical_profile_batches: BenchmarkProfileBatchReport::from_profile(&canonical_profile),
        effective_profile_batches: BenchmarkProfileBatchReport::from_profile(&profile),
        provider_effective: None,
        provider_note: "The harness proves real inference throughput, but fastembed/ort do not currently expose the effective execution provider strongly enough here; correlate with external GPU telemetry if required.".to_string(),
        corpus,
        targets,
    })
}

impl BenchmarkProfileBatchReport {
    fn from_profile(profile: &crate::embedder::EmbeddingProfile) -> Self {
        Self {
            chunk_batch_size: profile.chunk.batch_size,
            symbol_batch_size: profile.symbol.batch_size,
            file_vectorization_batch_size: profile.file_vectorization_batch_size,
            graph_batch_size: profile.graph.batch_size,
        }
    }
}

fn benchmark_target_report(
    model: &mut TextEmbedding,
    target: BenchmarkTargetKind,
    unique: &[BenchmarkSample],
    batch_size: usize,
    measurement_layer: BenchmarkMeasurementLayer,
    corpus_collection_seconds: f64,
    warmup_batches: usize,
    min_samples_per_target: usize,
) -> Result<BenchmarkTargetReport> {
    let measured = expand_benchmark_samples(unique, min_samples_per_target);
    if measured.is_empty() {
        return Ok(BenchmarkTargetReport {
            target,
            unique_samples: unique.len(),
            measured_samples: 0,
            batch_size,
            measurement_layer,
            corpus_collection_seconds_included: measurement_layer
                .includes_corpus_collection_in_total_seconds(),
            corpus_collection_seconds,
            prepare_seconds_included: measurement_layer
                .includes_prepare_seconds_in_total_seconds(),
            prepare_seconds: 0.0,
            total_embeddings: 0,
            total_seconds: 0.0,
            embeddings_per_second: 0.0,
            embeddings_per_hour: 0.0,
            avg_batch_latency_ms: 0.0,
            p95_batch_latency_ms: 0.0,
            target_met: false,
        });
    }

    let texts: Vec<String> = measured.iter().map(|sample| sample.text.clone()).collect();
    warmup_model(model, &texts, batch_size, warmup_batches)?;

    let prepare_started = Instant::now();
    let prebuilt_payloads = if measurement_layer.prebuilds_batches() {
        Some(
            texts.chunks(batch_size.max(1))
                .map(|chunk| chunk.to_vec())
                .collect::<Vec<_>>(),
        )
    } else {
        None
    };
    let prepare_seconds = prepare_started.elapsed().as_secs_f64();

    let mut latencies_ms = Vec::new();
    let mut total_embeddings = 0usize;
    let measurement_started = Instant::now();
    if measurement_layer.prebuilds_batches() {
        for payload in prebuilt_payloads.unwrap_or_default() {
            let t0 = Instant::now();
            let embeddings = model
                .embed(payload, None)
                .with_context(|| format!("embedding batch failed for {:?}", target))?;
            latencies_ms.push(t0.elapsed().as_secs_f64() * 1000.0);
            total_embeddings += embeddings.len();
        }
    } else {
        for chunk in texts.chunks(batch_size.max(1)) {
            let payload: Vec<String> = chunk.to_vec();
            let t0 = Instant::now();
            let embeddings = model
                .embed(payload, None)
                .with_context(|| format!("embedding batch failed for {:?}", target))?;
            latencies_ms.push(t0.elapsed().as_secs_f64() * 1000.0);
            total_embeddings += embeddings.len();
        }
    }
    let measured_seconds = measurement_started.elapsed().as_secs_f64();
    let mut total_seconds = measured_seconds;
    if measurement_layer.includes_prepare_seconds_in_total_seconds() {
        total_seconds += prepare_seconds;
    }
    if measurement_layer.includes_corpus_collection_in_total_seconds() {
        total_seconds += corpus_collection_seconds;
    }
    let embeddings_per_second = if total_seconds > 0.0 {
        total_embeddings as f64 / total_seconds
    } else {
        0.0
    };
    let embeddings_per_hour = embeddings_per_second * 3600.0;

    Ok(BenchmarkTargetReport {
        target,
        unique_samples: unique.len(),
        measured_samples: measured.len(),
        batch_size,
        measurement_layer,
        corpus_collection_seconds_included: measurement_layer
            .includes_corpus_collection_in_total_seconds(),
        corpus_collection_seconds,
        prepare_seconds_included: measurement_layer.includes_prepare_seconds_in_total_seconds(),
        prepare_seconds,
        total_embeddings,
        total_seconds,
        embeddings_per_second,
        embeddings_per_hour,
        avg_batch_latency_ms: average(&latencies_ms),
        p95_batch_latency_ms: percentile_95(&latencies_ms),
        target_met: embeddings_per_hour >= BENCHMARK_TARGET_EMBEDDINGS_PER_HOUR as f64,
    })
}

fn warmup_model(
    model: &mut TextEmbedding,
    texts: &[String],
    batch_size: usize,
    warmup_batches: usize,
) -> Result<()> {
    if texts.is_empty() || warmup_batches == 0 {
        return Ok(());
    }

    for chunk in texts
        .iter()
        .cycle()
        .take(batch_size.max(1) * warmup_batches)
        .cloned()
        .collect::<Vec<_>>()
        .chunks(batch_size.max(1))
    {
        model
            .embed(chunk.to_vec(), None)
            .context("warmup batch failed")?;
    }
    Ok(())
}

fn average(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.iter().sum::<f64>() / values.len() as f64
}

fn percentile_95(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let idx = ((sorted.len() - 1) as f64 * 0.95).round() as usize;
    sorted[idx]
}

fn symbol_snippet(content: &str, start_line: usize, end_line: usize) -> String {
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return String::new();
    }
    let start = start_line.saturating_sub(1).min(lines.len() - 1);
    let end = end_line.max(start_line).min(lines.len());
    lines[start..end].join("\n")
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    text.chars().take(max_chars).collect::<String>()
}

fn display_relative(root: &Path, path: &Path) -> String {
    relative_to_root(root, path).display().to_string()
}

fn relative_to_root<'a>(root: &'a Path, path: &'a Path) -> &'a Path {
    path.strip_prefix(root).unwrap_or(path)
}

fn is_hidden_or_generated(path: &Path) -> bool {
    path.components().any(|component| {
        let name = component.as_os_str().to_string_lossy();
        name.starts_with('.')
            || matches!(
                name.as_ref(),
                "target" | "node_modules" | "_build" | "deps" | "dist" | "build"
            )
            || name.starts_with("_build")
    })
}
