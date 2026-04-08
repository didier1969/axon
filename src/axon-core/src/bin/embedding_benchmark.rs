use axon_core::embedding_benchmark::{
    run_real_embedding_benchmark, BenchmarkMeasurementLayer, RealEmbeddingBenchmarkConfig,
};
use axon_core::embedder::{EmbeddingExecutionBackend, EmbeddingProfileKey};
use anyhow::{bail, Context, Result};
use std::path::PathBuf;

fn main() -> Result<()> {
    let mut config = RealEmbeddingBenchmarkConfig::default();
    let mut args = std::env::args().skip(1);
    let mut json_out: Option<PathBuf> = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--repo-path" => {
                config.repo_path = PathBuf::from(next_arg(&mut args, "--repo-path")?);
            }
            "--profile" => {
                config.profile_key = parse_profile(&next_arg(&mut args, "--profile")?)?;
            }
            "--backend" => {
                config.backend = parse_backend(&next_arg(&mut args, "--backend")?)?;
            }
            "--measurement-layer" | "--mode" => {
                config.measurement_layer =
                    parse_measurement_layer(&next_arg(&mut args, "--measurement-layer")?)?;
            }
            "--warmup-batches" => {
                config.warmup_batches = next_arg(&mut args, "--warmup-batches")?
                    .parse()
                    .context("invalid --warmup-batches")?;
            }
            "--min-samples" => {
                config.min_samples_per_target = next_arg(&mut args, "--min-samples")?
                    .parse()
                    .context("invalid --min-samples")?;
            }
            "--max-files" => {
                config.limits.max_files = next_arg(&mut args, "--max-files")?
                    .parse()
                    .context("invalid --max-files")?;
            }
            "--max-file-chars" => {
                config.limits.max_file_chars = next_arg(&mut args, "--max-file-chars")?
                    .parse()
                    .context("invalid --max-file-chars")?;
            }
            "--max-symbol-chars" => {
                config.limits.max_symbol_chars = next_arg(&mut args, "--max-symbol-chars")?
                    .parse()
                    .context("invalid --max-symbol-chars")?;
            }
            "--max-samples-per-target" => {
                config.limits.max_samples_per_target =
                    next_arg(&mut args, "--max-samples-per-target")?
                        .parse()
                        .context("invalid --max-samples-per-target")?;
            }
            "--json-out" => {
                json_out = Some(PathBuf::from(next_arg(&mut args, "--json-out")?));
            }
            "--help" | "-h" => {
                print_help();
                return Ok(());
            }
            other => bail!("unsupported argument: {other}"),
        }
    }

    if config.repo_path.as_os_str().is_empty() {
        config.repo_path = PathBuf::from(".");
    }

    let report = run_real_embedding_benchmark(&config)?;
    let json = serde_json::to_string_pretty(&report)?;

    if let Some(path) = json_out {
        std::fs::write(&path, format!("{json}\n"))
            .with_context(|| format!("failed to write {}", path.display()))?;
    }

    println!("{json}");
    Ok(())
}

fn next_arg(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String> {
    args.next()
        .ok_or_else(|| anyhow::anyhow!("missing value for {flag}"))
}

fn parse_profile(value: &str) -> Result<EmbeddingProfileKey> {
    match value.trim().to_ascii_lowercase().as_str() {
        "jina" | "jina-code" | "jina-code-v2-base" => Ok(EmbeddingProfileKey::JinaCodeV2Base),
        "bge-base" | "bge-base-en-v1.5" => Ok(EmbeddingProfileKey::BgeBaseEnv15),
        "legacy-bge-small" | "bge-small" | "bge-small-en-v1.5" => {
            Ok(EmbeddingProfileKey::LegacyBgeSmallEnv15)
        }
        other => bail!("unsupported --profile value: {other}"),
    }
}

fn parse_backend(value: &str) -> Result<EmbeddingExecutionBackend> {
    match value.trim().to_ascii_lowercase().as_str() {
        "cpu" => Ok(EmbeddingExecutionBackend::Cpu),
        "cuda" | "gpu" => Ok(EmbeddingExecutionBackend::GpuCuda),
        "unspecified" => Ok(EmbeddingExecutionBackend::Unspecified),
        other => bail!("unsupported --backend value: {other}"),
    }
}

fn parse_measurement_layer(value: &str) -> Result<BenchmarkMeasurementLayer> {
    match value.trim().to_ascii_lowercase().as_str() {
        "model_only" | "model-only" | "model" => Ok(BenchmarkMeasurementLayer::ModelOnly),
        "prepare_embed" | "prepare-embed" | "prepare" => {
            Ok(BenchmarkMeasurementLayer::PrepareEmbed)
        }
        "full_pipeline" | "full-pipeline" | "pipeline" | "full" => {
            Ok(BenchmarkMeasurementLayer::FullPipeline)
        }
        other => bail!("unsupported --measurement-layer value: {other}"),
    }
}

fn print_help() {
    println!(
        "\
embedding_benchmark
  --repo-path <path>
  [--profile jina|bge-base|legacy-bge-small]
  [--backend cpu|cuda]
  [--measurement-layer model_only|prepare_embed|full_pipeline]
  [--warmup-batches <n>]
  [--min-samples <n>]
  [--max-files <n>]
  [--max-file-chars <n>]
  [--max-symbol-chars <n>]
  [--max-samples-per-target <n>]
  [--json-out <path>]"
    );
}
