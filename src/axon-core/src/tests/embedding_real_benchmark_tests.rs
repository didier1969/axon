use crate::embedding_benchmark::{
    benchmark_target_for_symbol_kind, collect_repo_benchmark_corpus, expand_benchmark_samples,
    BenchmarkMeasurementLayer, BenchmarkProfileBatchReport, BenchmarkSample,
    BenchmarkTargetKind, CorpusCollectionLimits, RealEmbeddingBenchmarkConfig,
    RealEmbeddingBenchmarkReport, RepoBenchmarkCorpus,
    BENCHMARK_TARGET_EMBEDDINGS_PER_HOUR,
};
use crate::embedder::{
    resolve_embedding_provider_truth, resolve_embedding_provider_truth_with_probe,
    EmbeddingExecutionBackend, EmbeddingProviderStartupProbe,
};
use tempfile::tempdir;

#[test]
fn test_benchmark_symbol_kind_maps_types_and_procedures() {
    assert_eq!(
        benchmark_target_for_symbol_kind("struct"),
        Some(BenchmarkTargetKind::Type)
    );
    assert_eq!(
        benchmark_target_for_symbol_kind("class"),
        Some(BenchmarkTargetKind::Type)
    );
    assert_eq!(
        benchmark_target_for_symbol_kind("function"),
        Some(BenchmarkTargetKind::Procedure)
    );
    assert_eq!(
        benchmark_target_for_symbol_kind("method"),
        Some(BenchmarkTargetKind::Procedure)
    );
    assert_eq!(benchmark_target_for_symbol_kind("TODO"), None);
}

#[test]
fn test_expand_benchmark_samples_repeats_until_target_count() {
    let base = vec![
        BenchmarkSample::new(
            BenchmarkTargetKind::Procedure,
            "alpha".to_string(),
            "/tmp/a.rs".into(),
            "fn alpha() {}".to_string(),
        ),
        BenchmarkSample::new(
            BenchmarkTargetKind::Procedure,
            "beta".to_string(),
            "/tmp/b.rs".into(),
            "fn beta() {}".to_string(),
        ),
    ];

    let expanded = expand_benchmark_samples(&base, 5);

    assert_eq!(expanded.len(), 5);
    assert_eq!(expanded[0].label, "alpha");
    assert_eq!(expanded[1].label, "beta");
    assert_eq!(expanded[2].label, "alpha");
    assert_eq!(expanded[3].label, "beta");
    assert_eq!(expanded[4].label, "alpha");
}

#[test]
fn test_collect_repo_benchmark_corpus_extracts_files_types_and_procedures() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let sample = root.join("sample.rs");
    std::fs::write(
        &sample,
        r#"
pub struct Greeter {
    message: String,
}

impl Greeter {
    pub fn hello(&self) -> String {
        self.message.clone()
    }
}
"#,
    )
    .unwrap();

    let corpus = collect_repo_benchmark_corpus(
        root,
        &CorpusCollectionLimits {
            max_files: 8,
            max_file_chars: 2_000,
            max_symbol_chars: 500,
            max_samples_per_target: 32,
        },
    )
    .unwrap();

    assert_eq!(BENCHMARK_TARGET_EMBEDDINGS_PER_HOUR, 300_000);
    assert_eq!(corpus.files.len(), 1);
    assert!(
        corpus.types.iter().any(|sample| sample.label == "Greeter"),
        "type snippets should include the parsed struct"
    );
    assert!(
        corpus.procedures.iter().any(|sample| sample.label == "hello"),
        "procedure snippets should include the parsed method"
    );
}

#[test]
fn test_real_benchmark_config_defaults_to_full_pipeline_mode() {
    let config = RealEmbeddingBenchmarkConfig::default();

    assert_eq!(config.measurement_layer, BenchmarkMeasurementLayer::FullPipeline);
}

#[test]
fn test_measurement_layer_labels_are_stable() {
    assert_eq!(BenchmarkMeasurementLayer::ModelOnly.label(), "model_only");
    assert_eq!(BenchmarkMeasurementLayer::PrepareEmbed.label(), "prepare_embed");
    assert_eq!(
        BenchmarkMeasurementLayer::FullPipeline.label(),
        "full_pipeline"
    );
}

#[test]
fn test_measurement_layers_expose_distinct_timing_semantics() {
    assert!(
        BenchmarkMeasurementLayer::FullPipeline
            .includes_corpus_collection_in_total_seconds()
    );
    assert!(
        !BenchmarkMeasurementLayer::PrepareEmbed
            .includes_corpus_collection_in_total_seconds()
    );
    assert!(
        !BenchmarkMeasurementLayer::ModelOnly
            .includes_corpus_collection_in_total_seconds()
    );

    assert!(BenchmarkMeasurementLayer::ModelOnly.prebuilds_batches());
    assert!(!BenchmarkMeasurementLayer::PrepareEmbed.prebuilds_batches());
    assert!(!BenchmarkMeasurementLayer::FullPipeline.prebuilds_batches());
}

#[test]
fn test_measurement_layers_expose_distinct_preparation_accounting() {
    assert!(!BenchmarkMeasurementLayer::ModelOnly.includes_prepare_seconds_in_total_seconds());
    assert!(BenchmarkMeasurementLayer::PrepareEmbed.includes_prepare_seconds_in_total_seconds());
    assert!(BenchmarkMeasurementLayer::FullPipeline.includes_prepare_seconds_in_total_seconds());
}

#[test]
fn test_provider_truth_contract_separates_requested_heuristic_and_effective_fields() {
    let truth = resolve_embedding_provider_truth(EmbeddingExecutionBackend::GpuCuda, false);

    assert_eq!(truth.requested_backend, "cuda");
    assert_eq!(truth.device_heuristic_backend, "cpu");
    assert_eq!(truth.provider_effective, None);
    assert_eq!(truth.provider_status, "unverified");
}

#[test]
fn test_provider_truth_contract_surfaces_registration_probe_outcome() {
    let truth = resolve_embedding_provider_truth_with_probe(
        EmbeddingExecutionBackend::GpuCuda,
        false,
        Some(&EmbeddingProviderStartupProbe::registration_succeeded()),
    );

    assert_eq!(truth.provider_effective, Some("cuda"));
    assert_eq!(truth.provider_provenance, "ort_registration_probe");
    assert_eq!(truth.provider_registration_outcome, Some("registered"));
}

#[test]
fn test_benchmark_report_does_not_treat_requested_cuda_fallback_as_valid_gpu_benchmark() {
    let report = benchmark_report_fixture(Some("cuda"), "fallback");

    assert!(!report.has_verified_gpu_backend());
}

#[test]
fn test_benchmark_report_treats_verified_cuda_as_valid_gpu_benchmark() {
    let report = benchmark_report_fixture(Some("cuda"), "verified");

    assert!(report.has_verified_gpu_backend());
}

fn benchmark_report_fixture(
    provider_effective: Option<&str>,
    provider_status: &'static str,
) -> RealEmbeddingBenchmarkReport {
    RealEmbeddingBenchmarkReport {
        mode: "real",
        measurement_layer: BenchmarkMeasurementLayer::ModelOnly,
        repo_path: "/tmp/repo".into(),
        model_name: "fixture".to_string(),
        dimension: 768,
        requested_backend: "cuda",
        gpu_present: true,
        device_heuristic_backend: "cuda",
        target_embeddings_per_hour: BENCHMARK_TARGET_EMBEDDINGS_PER_HOUR,
        target_embeddings_per_second: BENCHMARK_TARGET_EMBEDDINGS_PER_HOUR as f64 / 3600.0,
        warmup_batches: 1,
        min_samples_per_target: 1,
        batch_override_active: false,
        batch_override_source: "canonical",
        canonical_profile_batches: BenchmarkProfileBatchReport {
            chunk_batch_size: 16,
            symbol_batch_size: 32,
            file_vectorization_batch_size: 8,
            graph_batch_size: 6,
        },
        effective_profile_batches: BenchmarkProfileBatchReport {
            chunk_batch_size: 16,
            symbol_batch_size: 32,
            file_vectorization_batch_size: 8,
            graph_batch_size: 6,
        },
        provider_effective: provider_effective.map(str::to_string),
        provider_status,
        provider_provenance: "fixture",
        provider_registration_outcome: None,
        provider_note: "fixture".to_string(),
        corpus: RepoBenchmarkCorpus {
            root: "/tmp/repo".into(),
            scanned_files: 0,
            supported_files: 0,
            files: Vec::new(),
            types: Vec::new(),
            procedures: Vec::new(),
        },
        targets: Vec::new(),
    }
}
